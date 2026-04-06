"""Chat compression — LLM-based history summarization.

When conversation history gets long, compress older messages into a
structured state snapshot instead of hard-cutting them. This preserves
context that would otherwise be lost.

Strategy:
  1. When messages exceed COMPRESSION_THRESHOLD, split into OLD + RECENT
  2. Send OLD messages to the LLM with a compression prompt
  3. Get back a structured summary (goal, requirements, agent state, plan)
  4. Replace OLD messages with the summary, keep RECENT verbatim
"""

from loguru import logger
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    TextPart,
    ThinkingPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)
from pydantic_ai.models import Model

# Compress when we have more than this many messages
# (includes tool call/result pairs, so a single exchange can be ~4 messages)
COMPRESSION_THRESHOLD = 40

# Keep this many recent messages verbatim after compression
KEEP_RECENT = 12

_COMPRESSION_PROMPT = """\
You are a context compression assistant. Your job is to distill a conversation \
between a user and an AI agent builder into a concise state snapshot.

The conversation is about building a voice agent. The AI helps the user \
design and configure agent behavior — nodes, edges, tools, system prompts, etc.

Read the entire conversation and produce a structured summary that captures \
ALL essential information. This summary will be the agent's ONLY memory of \
the earlier conversation — nothing else survives.

Output format (use plain text, no XML/markdown fences):

OVERALL GOAL:
<one sentence describing what the user wants to build>

REQUIREMENTS:
<bullet list of collected requirements, constraints, preferences>

AGENT STATE:
<current state of the agent configuration — nodes created, tools configured, \
system prompt details, any credentials or integrations discussed>

DECISIONS MADE:
<key architecture/design decisions and why>

CURRENT PLAN:
<what's been done and what's next, use [DONE] / [TODO] markers>

IMPORTANT DETAILS:
<any specific names, URLs, IDs, or technical details that must not be lost>

Rules:
- Be extremely concise but miss NOTHING important
- Preserve specific values: names, URLs, API endpoints, credential types
- Preserve the user's exact preferences and corrections
- If the user rejected something, note that
- Do NOT add information that wasn't in the conversation
"""


def _find_safe_split(
    messages: list[ModelMessage],
    desired: int,
) -> int:
    """Find a split index where "recent" starts at a clean turn boundary.

    Only `ModelRequest` messages with a `UserPromptPart` are valid split points.
    This guarantees the full assistant→tool-result group is never torn
    apart across the old/recent boundary.

    Returns the adjusted split index (always >= 1 so we compress at
    least something).
    """
    n = len(messages)
    idx = max(1, min(desired, n - 1))

    # Walk backward until "recent" starts with a user request
    # that contains a UserPromptPart (not just a ToolReturnPart).
    while idx > 0 and not (
        isinstance(messages[idx], ModelRequest)
        and any(isinstance(p, UserPromptPart) for p in messages[idx].parts)
    ):
        idx -= 1

    # Always compress at least 1 message.
    return max(idx, 1)


async def compress_history(
    model: Model,
    messages: list[ModelMessage],
) -> list[ModelMessage]:
    """Compress native ModelMessage history if it exceeds the threshold.

    Returns the (possibly compressed) history ready for use as message_history.
    If history is short enough, returns it unchanged.
    """
    if len(messages) <= COMPRESSION_THRESHOLD:
        return messages

    # Split: compress old messages, keep recent ones verbatim.
    desired_split = len(messages) - KEEP_RECENT
    split_point = _find_safe_split(messages, desired_split)
    old_messages = messages[:split_point]
    recent_messages = messages[split_point:]

    # Build the conversation text to summarize
    conversation_text = _format_messages_for_compression(old_messages)

    from pydantic_ai import Agent

    agent = Agent(model, system_prompt=_COMPRESSION_PROMPT)

    try:
        run_res = await agent.run(conversation_text, message_history=[])
        summary = run_res.output
    except Exception:
        logger.exception("Chat compression failed, falling back to hard trim")
        return recent_messages

    if not summary or not summary.strip():
        logger.warning("Chat compression returned empty summary, falling back")
        return recent_messages

    logger.info(
        "Compressed {} messages into summary ({} chars), keeping {} recent",
        len(old_messages),
        len(summary),
        len(recent_messages),
    )

    # Build compressed history: summary as context + recent messages
    compressed: list[ModelMessage] = [
        ModelRequest(
            parts=[
                UserPromptPart(
                    content=(
                        "[Context from earlier in our conversation]\n\n"
                        + summary.strip()
                    )
                )
            ]
        ),
        ModelResponse(
            parts=[
                TextPart(
                    content=(
                        "Got it — I have the full context from our earlier "
                        "conversation. Let's continue."
                    )
                )
            ]
        ),
        *recent_messages,
    ]

    return compressed


def _format_messages_for_compression(
    messages: list[ModelMessage],
) -> str:
    """Format ModelMessages into a readable transcript for the compression LLM."""
    lines: list[str] = []
    for msg in messages:
        if isinstance(msg, ModelRequest):
            for req_part in msg.parts:
                if isinstance(req_part, UserPromptPart):
                    lines.append(f"USER: {req_part.content}")
                elif isinstance(req_part, ToolReturnPart):
                    content = str(req_part.content)[:500]
                    lines.append(f"TOOL_RESULT ({req_part.tool_name}): {content}")
        elif isinstance(msg, ModelResponse):
            for res_part in msg.parts:
                if isinstance(res_part, TextPart):
                    lines.append(f"ASSISTANT: {res_part.content}")
                elif isinstance(res_part, ThinkingPart) and res_part.content:
                    lines.append(f"THINKING: {res_part.content}")
                elif isinstance(res_part, ToolCallPart):
                    args_str = str(res_part.args)[:300]
                    lines.append(f"TOOL_CALL ({res_part.tool_name}): {args_str}")
    return "\n\n".join(lines)
