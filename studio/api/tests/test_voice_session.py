from __future__ import annotations

import sys
from types import ModuleType

integrations_stub = ModuleType("integrations")
voice_engine_stub = ModuleType("voice_engine")
voice_engine_stub.AgentRunner = object

sys.modules.setdefault("integrations", integrations_stub)
sys.modules.setdefault("voice_engine", voice_engine_stub)

from app.api.voice_session import (  # noqa: E402
    _normalize_tool_call_id,
    _orphaned_tool_activity,
    _resolve_pending_tool,
    _track_pending_tool,
)


def test_track_and_resolve_pending_tool_by_id() -> None:
    pending_tools: list[tuple[str | None, str]] = []

    _track_pending_tool(pending_tools, "call-1", "save_artifact")
    _resolve_pending_tool(pending_tools, "call-1", "save_artifact")

    assert pending_tools == []


def test_resolve_pending_tool_falls_back_to_name() -> None:
    pending_tools = [(None, "notify_make_webhook")]

    _resolve_pending_tool(pending_tools, None, "notify_make_webhook")

    assert pending_tools == []


def test_resolve_pending_tool_falls_back_to_unique_name_after_id_mismatch() -> None:
    pending_tools = [("call-1", "notify_make_webhook")]

    _resolve_pending_tool(pending_tools, "call_1", "notify_make_webhook")

    assert pending_tools == []


def test_resolve_pending_tool_keeps_ambiguous_name_matches_pending() -> None:
    pending_tools = [
        ("call-1", "notify_make_webhook"),
        ("call-2", "notify_make_webhook"),
    ]

    _resolve_pending_tool(pending_tools, "call_1", "notify_make_webhook")

    assert pending_tools == [
        ("call-1", "notify_make_webhook"),
        ("call-2", "notify_make_webhook"),
    ]


def test_orphaned_tool_activity_omits_empty_tool_call_id() -> None:
    payload = _orphaned_tool_activity(
        _normalize_tool_call_id(""),
        "save_artifact",
    )

    assert payload == {
        "type": "tool_activity",
        "tool_name": "save_artifact",
        "status": "orphaned",
    }
