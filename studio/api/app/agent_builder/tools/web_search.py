"""Web search tool — grounded search via Gemini's google_search.

Lets the builder LLM research APIs, services, and integrations
that aren't covered by pre-authored skills.

Uses Gemini's built-in google_search tool for grounded results.
Requires GEMINI_API_KEY environment variable.
"""

from typing import Any

import httpx
from loguru import logger
from pydantic_ai import Agent, RunContext

from app.lib import get_settings

_GEMINI_URL = (
    "https://generativelanguage.googleapis.com/v1beta/"
    "models/gemini-3-flash-preview:generateContent"
)
_TIMEOUT = 30.0


async def _google_search(query: str) -> str:
    """Call Gemini with google_search tool and return the grounded answer."""
    api_key = get_settings().gemini.api_key
    if not api_key:
        return "Web search unavailable: GEMINI__API_KEY not configured."

    payload = {
        "contents": [{"parts": [{"text": query}]}],
        "tools": [{"google_search": {}}],
    }

    async with httpx.AsyncClient(timeout=_TIMEOUT) as client:
        resp = await client.post(
            _GEMINI_URL,
            headers={
                "x-goog-api-key": api_key,
                "Content-Type": "application/json",
            },
            json=payload,
        )

    if resp.status_code != 200:
        logger.warning("Gemini search failed: {} {}", resp.status_code, resp.text[:200])
        return f"Web search failed (HTTP {resp.status_code}). Try again or proceed without."

    try:
        data: dict[str, Any] = resp.json()
    except (ValueError, UnicodeDecodeError):
        logger.warning("Gemini returned non-JSON response: {}", resp.text[:200])
        return "Web search returned an invalid response. Try again or proceed without."

    # Extract text from response
    try:
        candidates = data.get("candidates", [])
        if not candidates:
            return "No search results found."
        parts = candidates[0].get("content", {}).get("parts", [])
        text_parts: list[str] = []
        for part in parts:
            if "text" in part:
                text_parts.append(part["text"])
        if not text_parts:
            return "No search results found."
        answer = "\n".join(text_parts)
    except (KeyError, IndexError):
        return "Failed to parse search results."

    # Extract grounding sources if available
    grounding = candidates[0].get("groundingMetadata", {}).get("groundingChunks", [])
    if grounding:
        sources: list[str] = []
        for chunk in grounding[:5]:  # Top 5 sources
            web = chunk.get("web", {})
            title = web.get("title", "")
            uri = web.get("uri", "")
            if uri:
                sources.append(f"- [{title}]({uri})" if title else f"- {uri}")
        if sources:
            answer += "\n\nSources:\n" + "\n".join(sources)

    return answer


def register_web_search_tools(agent: Agent[Any, Any]) -> None:
    """Register web_search tool on the builder agent."""

    @agent.tool
    async def web_search(ctx: RunContext[Any], query: str) -> str:
        """Search the web for information to help build a better agent.

        Use this when you need external knowledge, such as:
        - API documentation, endpoints, and auth patterns
        - Industry best practices for the agent's domain
        - Competitor workflows and conversation patterns
        - Regulatory or compliance requirements
        - Technical specifications for integrations
        - Pricing, rate limits, or service capabilities

        Keep queries focused and specific, e.g.:
        - "Calendly API authentication and list events endpoint"
        - "best practices for restaurant order taking chatbot"
        - "HIPAA compliance requirements for healthcare voice agents"
        - "Stripe payment intent flow for phone orders"
        """
        logger.info("Builder web search: '{}'", query)
        result = await _google_search(query)
        return result
