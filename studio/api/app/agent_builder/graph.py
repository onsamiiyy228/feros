"""Agent graph configuration schema — the new data contract.

Replaces v2_scene with a simpler graph language:
- Nodes (not scenes) with system_prompt, tools, edges
- Tools are always JavaScript (ES2020) scripts
- State transitions via graph edges

The graph JSON is stored directly in AgentVersion.config_json.
Rust runtime deserializes it into AgentGraphDef.
"""

from __future__ import annotations

import asyncio
import json
import re
from typing import Any

from loguru import logger
from pydantic_ai import Agent
from pydantic_ai.models import Model

# ── Graph Validation ─────────────────────────────────────────────


def validate_graph(graph: dict[str, Any]) -> list[str]:
    """Validate an agent graph JSON structure.

    Checks:
    1. 'entry' field exists and references a valid node
    2. 'nodes' is a non-empty dict
    3. Every node has a 'system_prompt' string
    4. Every node's 'edges' reference valid node IDs
    5. Every node's 'tools' reference tools defined in 'tools' map
    6. 'tools' entries have 'description' and 'script' fields
    7. Tool params have 'name' fields

    Returns a list of error messages (empty = valid).
    """
    errors: list[str] = []

    # Basic structure
    if not graph:
        return ["Graph must be empty"]

    from google.protobuf.json_format import ParseDict, ParseError

    from app.schemas.agent_pb2 import AgentGraphDef

    try:
        ParseDict(graph, AgentGraphDef(), ignore_unknown_fields=False)
    except ParseError as e:
        errors.append(f"Format compilation error: {str(e)}")
        return errors

    entry = graph.get("entry")
    if not entry or not isinstance(entry, str):
        errors.append("Missing or invalid 'entry' field (must be a non-empty string)")

    nodes = graph.get("nodes")
    if not isinstance(nodes, dict) or len(nodes) == 0:
        errors.append("'nodes' must be a non-empty object")
        return errors  # Can't validate further without nodes

    tools = graph.get("tools", {})
    if not isinstance(tools, dict):
        errors.append("'tools' must be an object")
        tools = {}

    node_ids = set(nodes.keys())
    tool_ids = set(tools.keys())

    # ── Entry must reference a valid node ────────────────────
    if entry and entry not in node_ids:
        errors.append(
            f"Entry '{entry}' does not reference a valid node. "
            f"Available nodes: {', '.join(sorted(node_ids))}"
        )

    # ── Validate each node ───────────────────────────────────
    for node_id, node_def in nodes.items():
        if not isinstance(node_def, dict):
            errors.append(f"Node '{node_id}' must be an object")
            continue

        # system_prompt is required
        prompt = node_def.get("system_prompt", "")
        if not isinstance(prompt, str) or not prompt.strip():
            errors.append(f"Node '{node_id}' is missing a non-empty 'system_prompt'")

        # edges must reference valid nodes
        edges = node_def.get("edges", [])
        if not isinstance(edges, list):
            errors.append(f"Node '{node_id}'.edges must be an array")
        else:
            for edge in edges:
                if edge not in node_ids:
                    errors.append(
                        f"Node '{node_id}' has edge to '{edge}' "
                        f"which is not a valid node"
                    )

        # tools must reference valid tool definitions
        node_tools = node_def.get("tools", [])
        if not isinstance(node_tools, list):
            errors.append(f"Node '{node_id}'.tools must be an array")
        else:
            for tool_ref in node_tools:
                if tool_ref not in tool_ids:
                    errors.append(
                        f"Node '{node_id}' references tool '{tool_ref}' "
                        f"which is not defined in 'tools'"
                    )

    # ── Validate each tool ───────────────────────────────────
    for tool_id, tool_def in tools.items():
        if not isinstance(tool_def, dict):
            errors.append(f"Tool '{tool_id}' must be an object")
            continue

        if not tool_def.get("description"):
            errors.append(f"Tool '{tool_id}' is missing 'description'")

        if not tool_def.get("script"):
            errors.append(f"Tool '{tool_id}' is missing 'script'")

        # Validate params structure
        params = tool_def.get("params", [])
        if not isinstance(params, list):
            errors.append(f"Tool '{tool_id}'.params must be an array")
        else:
            for i, param in enumerate(params):
                if not isinstance(param, dict):
                    errors.append(f"Tool '{tool_id}'.params[{i}] must be an object")
                elif not param.get("name"):
                    errors.append(f"Tool '{tool_id}'.params[{i}] is missing 'name'")

    # ── Check for unreachable nodes ──────────────────────────
    if entry and entry in node_ids and len(node_ids) > 1:
        reachable: set[str] = set()
        frontier = [entry]
        while frontier:
            current = frontier.pop()
            if current in reachable:
                continue
            reachable.add(current)
            node_def = nodes.get(current, {})
            for edge in node_def.get("edges", []):
                if edge not in reachable:
                    frontier.append(edge)

        unreachable = node_ids - reachable
        if unreachable:
            errors.append(
                f"Unreachable nodes (not connected from entry): "
                f"{', '.join(sorted(unreachable))}"
            )

    # ── Topology check: dispatcher star ──────────────────────
    # Multi-node graphs must be a star: every non-entry node
    # must have an edge back to the entry node (dispatcher),
    # and must NOT have edges to other non-entry nodes.
    if entry and entry in node_ids and len(node_ids) > 1:
        missing_back_edge: list[str] = []
        specialist_to_specialist: list[str] = []
        for node_id, node_def in nodes.items():
            if node_id == entry:
                continue
            if not isinstance(node_def, dict):
                continue
            edges = node_def.get("edges", [])
            if entry not in edges:
                missing_back_edge.append(node_id)
            # Specialist nodes must ONLY edge back to dispatcher
            bad_targets = [e for e in edges if e != entry and e in node_ids]
            if bad_targets:
                specialist_to_specialist.append(
                    f"'{node_id}' → {', '.join(repr(t) for t in bad_targets)}"
                )
        if missing_back_edge:
            errors.append(
                f"Dispatcher star topology violated: nodes "
                f"{', '.join(sorted(missing_back_edge))} must have an edge "
                f"back to the entry node '{entry}'. "
                f"Multi-node graphs must be star-shaped — every specialist "
                f"node needs an edge back to the dispatcher."
            )
        if specialist_to_specialist:
            errors.append(
                f"Specialist-to-specialist edges are FORBIDDEN in a star "
                f"topology: {'; '.join(specialist_to_specialist)}. "
                f"Specialist nodes may ONLY edge back to the dispatcher "
                f"'{entry}'. If your flow is linear (e.g. greet → ask → "
                f"confirm), use a SINGLE node instead."
            )

    return errors


def _mermaid_id(raw: str) -> str:
    """Sanitize a string for use as a Mermaid node ID."""
    return re.sub(r"[^a-zA-Z0-9_]", "_", raw)


_MERMAID_SUBGRAPH_RE = re.compile(r"^\s*subgraph\s+([^\s\[]+)")
_MERMAID_NODE_DEF_RE = re.compile(r"^\s*([A-Za-z0-9_]+)\s*[\[\(\{]")
_MERMAID_EDGE_OP_RE = re.compile(r"<==>|<-->|<-.->|==>|-->|-.->|===|---|-.-")


def _strip_entry_anchor_block(mermaid: str) -> str:
    """Remove a pre-existing ``__entry_anchor`` block so it can be re-injected.

    Deletes up to 4 lines:
        __entry_anchor(( ))
        style __entry_anchor fill:none,stroke:none
        __entry_anchor --- <target>
        linkStyle N stroke:none,stroke-width:0px   ← companion, only if immediately after anchor edge
    """
    lines = mermaid.splitlines()
    result: list[str] = []
    skip_next_linkstyle = False
    for line in lines:
        if "__entry_anchor" in line:
            if "---" in line:
                skip_next_linkstyle = True
            continue
        if skip_next_linkstyle:
            skip_next_linkstyle = False
            if re.match(
                r"^\s*linkStyle\s+\d+\s+stroke:none,stroke-width:0px\s*$", line
            ):
                continue
        result.append(line)
    return "\n".join(result)


def _pin_entry_to_top(mermaid: str, entry: str) -> str:
    """Bias Mermaid layout so the entry node starts at the top.

    Mermaid/ELK can place the logical first step in the visual center when the
    graph has branching or back-edges.  We inject a hidden anchor node connected
    to the entry target via a hidden ``---`` edge so the layout engine keeps
    that target at the top without showing an extra visible arrow.

    The anchor is inserted **after** the entry subgraph's ``end`` line (or after
    the target node's definition in non-subgraph diagrams) so that ``target_id``
    is already defined with its label before the anchor references it.  This
    avoids beautiful-mermaid's "first-seen wins" parser behaviour where a bare
    reference would lock in a label-less node.
    """
    if not mermaid.strip().startswith("graph"):
        return mermaid

    # Strip any pre-existing anchor block (e.g. LLM-copied with wrong linkStyle)
    mermaid = _strip_entry_anchor_block(mermaid)

    entry_ids = {entry, _mermaid_id(entry)}
    target_id = _mermaid_id(entry)
    lines = mermaid.splitlines()
    subgraph_end_idx: int | None = None
    target_def_idx: int | None = None

    # Scan for: first node inside entry subgraph (→ target_id) and the
    # subgraph's closing ``end`` line (→ subgraph_end_idx).
    in_entry_subgraph = False
    found_target = False
    for idx, line in enumerate(lines):
        stripped = line.strip()
        subgraph_match = _MERMAID_SUBGRAPH_RE.match(line)
        if subgraph_match:
            in_entry_subgraph = subgraph_match.group(1) in entry_ids
            continue
        if in_entry_subgraph and stripped == "end":
            subgraph_end_idx = idx
            in_entry_subgraph = False
            continue
        if in_entry_subgraph and not found_target:
            node_match = _MERMAID_NODE_DEF_RE.match(line)
            if node_match:
                target_id = node_match.group(1)
                found_target = True

    if not lines:
        return mermaid

    # Decide insertion point.
    if subgraph_end_idx is not None:
        # Rich (LLM) diagram — insert right after the entry subgraph's ``end``.
        insert_at = subgraph_end_idx
    else:
        # No subgraph (deterministic diagram) — insert after target's definition
        # so the label is already established before the anchor references it.
        for idx, line in enumerate(lines):
            node_match = _MERMAID_NODE_DEF_RE.match(line)
            if node_match and node_match.group(1) == target_id:
                target_def_idx = idx
                break
        # Extreme fallback: graph TD line.  Should almost never happen — both
        # the subgraph and target-definition paths cover normal diagrams.
        insert_at = target_def_idx if target_def_idx is not None else 0

    hidden_edge_index = sum(
        len(_MERMAID_EDGE_OP_RE.findall(line)) for line in lines[: insert_at + 1]
    )
    anchor_lines = [
        "  __entry_anchor(( ))",
        "  style __entry_anchor fill:none,stroke:none",
        f"  __entry_anchor --- {target_id}",
        f"  linkStyle {hidden_edge_index} stroke:none,stroke-width:0px",
    ]
    return "\n".join([*lines[: insert_at + 1], *anchor_lines, *lines[insert_at + 1 :]])


def generate_graph_mermaid(graph: dict[str, Any]) -> str:
    """Generate a basic Mermaid flowchart from an agent graph (deterministic fallback)."""
    lines = ["graph TD"]

    nodes = graph.get("nodes", {})
    tools = graph.get("tools", {})
    entry = graph.get("entry", "")

    # Define nodes
    for node_id in nodes:
        mid = _mermaid_id(node_id)
        label = node_id.replace("_", " ").title()
        lines.append(f'  {mid}["{label}"]')

    # Define tools
    for tool_id in tools:
        mid = _mermaid_id(tool_id)
        lines.append(f'  {mid}["{tool_id}"]')

    # Edges between nodes
    for node_id, node_def in nodes.items():
        src = _mermaid_id(node_id)
        for edge in node_def.get("edges", []):
            lines.append(f'  {src} -->|"transfer"| {_mermaid_id(edge)}')

        for tool_ref in node_def.get("tools", []):
            lines.append(f'  {src} -.->|"uses"| {_mermaid_id(tool_ref)}')

    return _pin_entry_to_top("\n".join(lines), entry)


# ── LLM-based Mermaid Generation ────────────────────────────────

_MERMAID_SYSTEM_PROMPT = """\
You are a diagram specialist. Given an agent configuration JSON, generate a \
Mermaid flowchart that visualizes the agent's conversation flow.

RULES:
- Output ONLY valid Mermaid code, no markdown fences, no explanation.
- Start with `graph TD`.
- Use `subgraph` blocks for each node to show internal conversation flow.
- Extract conversation steps from each node's `system_prompt` (numbered lists, \
  sequential instructions, etc.) and render them as connected steps inside \
  the subgraph.
- Show edges between nodes (from `edges` field) outside the subgraphs.
- Keep labels short (3-5 words max per step).
- Use descriptive edge labels where helpful.
- Do NOT use emoji in any node label or ID.
- TOOL NODE IDs: every tool node's Mermaid ID MUST be the exact tool key from \
  the config's `tools` map (replacing non-alphanumeric chars with `_`). \
  The visible label MUST also be the raw tool key. \
  Example: `check_availability["check_availability"]`. \
  NEVER invent synthetic IDs like t1, t2 for tools.
- Connect tool nodes with dotted lines from the subgraph that uses them.
- When a previous diagram is provided, you MUST start from the previous diagram \
  and make MINIMAL modifications. Keep existing node IDs, labels, and connections \
  unchanged unless the updated config clearly requires a local modification. \
  Use the change_summary as a hint for what changed, not as the sole source of truth. \
  Only add, remove, or relabel the nodes/edges that the change directly affects. \
  Do NOT rearrange, renumber, or restyle existing elements.
- For small validations or constraints, prefer annotating or locally refining the \
  nearest existing step instead of introducing a new standalone step, unless the \
  updated config clearly implies a separate stage.

SINGLE-NODE EXAMPLE (input has 1 node with tool "check_availability"):
graph TD
  subgraph booking["Booking"]
    b1[Greet caller] --> b2[Ask name]
    b2 --> b3[Ask date & time]
    b3 --> b4[Ask guest count]
    b4 --> b5[Confirm & goodbye]
  end
  check_availability["check_availability"]
  booking -.-> check_availability

MULTI-NODE EXAMPLE (dispatcher + children):
graph TD
  subgraph dispatcher["Dispatcher"]
    d1[Route by intent]
  end
  subgraph booking["Booking"]
    b1[Ask name] --> b2[Ask date] --> b3[Confirm]
  end
  subgraph faq["FAQ"]
    f1[Answer question] --> f2[Offer more help]
  end
  dispatcher --> booking
  dispatcher --> faq
"""


async def generate_graph_mermaid_llm(
    graph: dict[str, Any],
    model: Model | str | None,
    *,
    previous_mermaid: str | None = None,
    change_summary: str | None = None,
) -> str:
    """Generate a rich Mermaid diagram using an LLM.

    When either *previous_mermaid* or *change_summary* is provided, the LLM
    receives them as independent optional context and is instructed to evolve
    the existing diagram with minimal modifications rather than creating from
    scratch.

    Falls back to the deterministic ``generate_graph_mermaid`` on failure.
    """
    try:
        agent: Agent[None, str] = Agent(
            model=model,
            output_type=str,
            system_prompt=_MERMAID_SYSTEM_PROMPT,
        )
        config_json = json.dumps(graph, indent=2)

        # Build user prompt with optional continuity context (independent, not gated on each other)
        parts = [config_json]
        if previous_mermaid:
            parts.append(f"\nPrevious Mermaid:\n{previous_mermaid}")
        if change_summary:
            parts.append(f"\nChange Summary:\n{change_summary}")
        user_prompt = "\n".join(parts)

        result = await asyncio.wait_for(agent.run(user_prompt), timeout=30)
        text = result.output.strip()

        # Strip markdown fences if the LLM wraps them
        if text.startswith("```"):
            first_newline = text.index("\n")
            text = text[first_newline + 1 :]
        if text.endswith("```"):
            text = text[:-3].rstrip()

        # Sanity check: must start with "graph"
        if not text.startswith("graph"):
            logger.warning(
                "LLM mermaid output doesn't start with 'graph', using fallback"
            )
            return generate_graph_mermaid(graph)

        return _pin_entry_to_top(text, graph.get("entry", ""))

    except Exception as e:
        logger.warning(
            "LLM mermaid generation failed ({}: {}), using fallback",
            type(e).__name__,
            e,
        )
        return generate_graph_mermaid(graph)
