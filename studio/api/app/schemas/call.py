"""Pydantic schemas for call logs."""

from __future__ import annotations

import uuid
from datetime import datetime
from typing import Any

from pydantic import BaseModel


class CallResponse(BaseModel):
    """Response for a single call log."""

    id: uuid.UUID
    agent_id: uuid.UUID
    agent_name: str | None = None
    direction: str  # inbound | outbound | webrtc
    caller_number: str | None
    callee_number: str | None
    status: str
    duration_seconds: int | None
    transcript_json: dict[str, Any] | None
    recording_url: str | None
    variables_json: dict[str, Any] | None
    outcome: str | None
    sentiment_score: float | None
    agent_version_used: int | None
    started_at: datetime | None
    ended_at: datetime | None
    created_at: datetime

    model_config = {"from_attributes": True}


class CallListResponse(BaseModel):
    """Paginated list of calls."""

    calls: list[CallResponse]
    total: int


class CallEventResponse(BaseModel):
    id: uuid.UUID
    call_id: uuid.UUID
    session_id: str
    seq: int
    event_type: str
    event_category: str
    occurred_at: datetime
    payload_json: dict[str, Any]

    model_config = {"from_attributes": True}


class CallEventListResponse(BaseModel):
    events: list[CallEventResponse]
    total: int
    skip: int
    limit: int


class CallExternalLink(BaseModel):
    adapter: str
    label: str
    url: str


class CallLogCapabilitiesResponse(BaseModel):
    has_internal_logs: bool
    active_adapters: list[str]
    external_links: list[CallExternalLink]
