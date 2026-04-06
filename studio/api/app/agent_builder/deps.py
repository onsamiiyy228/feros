from __future__ import annotations

from typing import Any

from pydantic import BaseModel

from app.schemas.agent import ActionCardSchema


class BuilderResult(BaseModel):
    """The output of the builder LLM.

    The LLM decides what to populate:
      - config → generated or modified agent config
      - message_history → compressed message list for next turn
    """

    config: dict[str, Any] | None = None  # Agent graph JSON (if changed)
    change_summary: str | None = None  # What changed
    action_cards: list[ActionCardSchema] = []  # Credential/auth prompts for the UI
    mermaid_diagram: str | None = None  # Visual flow of the agent's logic
    # Full compressed message history for DB persistence (serialized ModelMessage list)
    message_history: list[Any] | None = None
    # Metadata for the API layer
    used_edit_path: bool = False  # True if apply_agent_edits was used
    base_version: int | None = None  # Version the edits were based on


class BuilderDeps(BaseModel):
    """Dependencies injected into the builder agent."""

    agent_id: str
    agent_name: str
    current_config: dict[str, Any] | None
    current_version: int | None = None
    # Pre-fetched connection status for system prompt injection
    connection_status: str = ""
    # Mutable slots filled by the save_agent_config / apply_agent_edits tools
    emitted_config: dict[str, Any] | None = None
    emitted_change_summary: str | None = None
    # Track which output path was used (for mermaid routing)
    used_edit_path: bool = False
    # Set by ask_user tool to break the agent loop after the current round
    pause_requested: bool = False
