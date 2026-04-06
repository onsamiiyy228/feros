"""Builder tools — Pydantic AI tools the builder LLM can call.

Each sub-module defines tool functions and a register_*() function
that attaches them to the agent.
"""

from typing import Any

from app.agent_builder.tools.artifacts import register_artifact_tools
from app.agent_builder.tools.connections import register_connection_tools
from app.agent_builder.tools.skills import register_skill_tools
from app.agent_builder.tools.web_search import register_web_search_tools


def register_all_tools(agent: Any) -> None:
    """Register all builder tools on the given agent."""
    register_skill_tools(agent)
    register_artifact_tools(agent)
    register_web_search_tools(agent)
    register_connection_tools(agent)
