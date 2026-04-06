from __future__ import annotations

from app.lib.tool_activity import tool_activity_from_runner_event


def test_tool_activity_maps_successful_completion() -> None:
    payload = tool_activity_from_runner_event(
        {"id": "call-1", "name": "lookup", "success": True, "error_message": None}
    )

    assert payload == {
        "type": "tool_activity",
        "tool_call_id": "call-1",
        "tool_name": "lookup",
        "status": "completed",
    }


def test_tool_activity_maps_failed_completion() -> None:
    payload = tool_activity_from_runner_event(
        {
            "id": "call-2",
            "name": "lookup",
            "success": False,
            "error_message": "Tool 'lookup' returned 500",
        }
    )

    assert payload == {
        "type": "tool_activity",
        "tool_call_id": "call-2",
        "tool_name": "lookup",
        "status": "error",
        "error_message": "Tool 'lookup' returned 500",
    }


def test_tool_activity_falls_back_to_error_message_when_success_missing() -> None:
    payload = tool_activity_from_runner_event(
        {
            "id": "call-3",
            "name": "lookup",
            "error_message": "Tool 'lookup' timed out",
        }
    )

    assert payload["status"] == "error"
    assert payload["error_message"] == "Tool 'lookup' timed out"
