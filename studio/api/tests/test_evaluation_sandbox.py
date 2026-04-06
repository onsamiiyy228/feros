from __future__ import annotations

from app.schemas.evaluation import (
    ScenarioProfile,
    ToolMockOutcome,
    ToolSandboxOverride,
    ToolSandboxResolutionInput,
    ToolSandboxRule,
)
from app.services.evaluations.sandbox import SeededDeterministicSandboxResolver


def _base_input() -> ToolSandboxResolutionInput:
    return ToolSandboxResolutionInput(
        scenario_profile=ScenarioProfile.BALANCED,
        seed=123,
        turn_id=1,
        tool_call_index=0,
        tool_id="calendar.check_availability",
        args_json={"date": "2026-04-01"},
    )


def test_seeded_resolution_is_deterministic() -> None:
    resolver = SeededDeterministicSandboxResolver()
    payload = _base_input()

    first = resolver.resolve(payload)
    second = resolver.resolve(payload)

    assert first == second


def test_override_has_highest_precedence() -> None:
    resolver = SeededDeterministicSandboxResolver()
    payload = _base_input()
    payload.rules = [
        ToolSandboxRule(
            tool_id=payload.tool_id,
            condition="always",
            outcome=ToolMockOutcome.SUCCESS,
        )
    ]
    payload.overrides = [
        ToolSandboxOverride(
            turn_id=payload.turn_id,
            tool_id=payload.tool_id,
            outcome=ToolMockOutcome.TIMEOUT,
        )
    ]

    result = resolver.resolve(payload)
    assert result.outcome == ToolMockOutcome.TIMEOUT
    assert result.source == "override"


def test_rule_precedes_profile_draw() -> None:
    resolver = SeededDeterministicSandboxResolver()
    payload = _base_input()
    payload.rules = [
        ToolSandboxRule(
            tool_id=payload.tool_id,
            condition="args.date==2026-04-01",
            outcome=ToolMockOutcome.HTTP_4XX,
        )
    ]

    result = resolver.resolve(payload)
    assert result.outcome == ToolMockOutcome.HTTP_4XX
    assert result.source == "rule"
