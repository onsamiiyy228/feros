from __future__ import annotations

from collections.abc import Mapping
from typing import Any


def tool_activity_from_runner_event(event: Mapping[str, Any]) -> dict[str, Any]:
    """Map a runner tool completion event to the frontend websocket shape."""
    success = event.get("success")
    raw_error = event.get("error_message")
    error_message = raw_error if isinstance(raw_error, str) and raw_error else None

    if isinstance(success, bool):
        status = "completed" if success else "error"
    else:
        status = "error" if error_message else "completed"

    payload: dict[str, Any] = {
        "type": "tool_activity",
        "tool_call_id": event.get("id"),
        "tool_name": str(event.get("name", "")),
        "status": status,
    }
    if error_message is not None:
        payload["error_message"] = error_message
    return payload
