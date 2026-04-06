"""Contract tests for Auto E2E evaluation schemas (Phase 0)."""

from __future__ import annotations

import uuid
from datetime import UTC, datetime

import pytest
from pydantic import TypeAdapter, ValidationError

from app.schemas.evaluation import (
    EvaluationConfigPayload,
    EvaluationRunEvent,
    EvaluationRunEventEnvelope,
    EvaluationRunEventType,
    PersonaPreset,
    ScenarioProfile,
    ToolMockOutcome,
    ToolSandboxProfileWeights,
)


def test_evaluation_config_payload_defaults() -> None:
    payload = EvaluationConfigPayload()

    assert payload.persona_preset == PersonaPreset.COOPERATIVE
    assert payload.scenario_profile == ScenarioProfile.BALANCED
    assert payload.max_turns == 12
    assert payload.timeout_seconds == 180
    assert payload.seed == 42
    assert payload.run_count == 1
    assert payload.judge.enabled is True


def test_evaluation_run_event_discriminator_tool_mock_result() -> None:
    adapter = TypeAdapter(EvaluationRunEvent)

    raw = {
        "event_type": EvaluationRunEventType.TOOL_MOCK_RESULT,
        "seq_no": 5,
        "timestamp": datetime.now(UTC).isoformat(),
        "turn_id": 2,
        "tool_call_id": "call_1",
        "tool_id": "calendar.check_availability",
        "outcome": ToolMockOutcome.TIMEOUT,
        "error_message": "Timed out after 10s",
    }

    event = adapter.validate_python(raw)
    assert event.event_type == EvaluationRunEventType.TOOL_MOCK_RESULT
    assert event.turn_id == 2
    assert event.outcome == ToolMockOutcome.TIMEOUT


def test_event_envelope_round_trip() -> None:
    run_id = uuid.uuid4()
    raw = {
        "run_id": str(run_id),
        "event": {
            "event_type": EvaluationRunEventType.CALLER_UTTERANCE,
            "seq_no": 2,
            "timestamp": datetime.now(UTC).isoformat(),
            "turn_id": 1,
            "text": "I need help booking an appointment",
        },
    }

    envelope = EvaluationRunEventEnvelope.model_validate(raw)
    assert envelope.run_id == run_id
    assert envelope.event.event_type == EvaluationRunEventType.CALLER_UTTERANCE


def test_tool_sandbox_profile_weights_total_must_be_positive() -> None:
    with pytest.raises(ValidationError):
        ToolSandboxProfileWeights(
            success=0,
            empty=0,
            partial=0,
            timeout=0,
            http_4xx=0,
            http_5xx=0,
            malformed=0,
        )
