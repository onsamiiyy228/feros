"""Deterministic tool-call sandbox resolution for evaluation runs."""

from __future__ import annotations

import hashlib
import random
from dataclasses import dataclass
from typing import Any

from app.schemas.evaluation import (
    ToolMockOutcome,
    ToolSandboxResolutionInput,
    ToolSandboxResolutionResult,
)
from app.services.evaluations.contracts import DeterministicSandboxResolver
from app.services.evaluations.profiles import default_profile_weights


def _stable_seed_int(seed_material: str) -> int:
    digest = hashlib.sha256(seed_material.encode("utf-8")).digest()
    return int.from_bytes(digest[:8], "big")


def _coerce_value(raw: str) -> object:
    lowered = raw.strip().lower()
    if lowered in {"true", "false"}:
        return lowered == "true"
    try:
        if "." in raw:
            return float(raw)
        return int(raw)
    except ValueError:
        return raw.strip().strip('"').strip("'")


def _rule_matches(condition: str, args_json: dict[str, Any]) -> bool:
    normalized = condition.strip().lower()
    if normalized in {"*", "always", "true"}:
        return True
    if not normalized.startswith("args.") or "==" not in condition:
        return False
    lhs, rhs = condition.split("==", maxsplit=1)
    key = lhs.strip()[len("args.") :]
    expected = _coerce_value(rhs)
    return args_json.get(key) == expected


def _default_outcome_payload(outcome: ToolMockOutcome) -> ToolSandboxResolutionResult:
    if outcome == ToolMockOutcome.SUCCESS:
        return ToolSandboxResolutionResult(
            outcome=outcome,
            status_code=200,
            body_json={"ok": True},
            source="profile",
        )
    if outcome == ToolMockOutcome.EMPTY:
        return ToolSandboxResolutionResult(
            outcome=outcome,
            status_code=200,
            body_json={},
            source="profile",
        )
    if outcome == ToolMockOutcome.PARTIAL:
        return ToolSandboxResolutionResult(
            outcome=outcome,
            status_code=206,
            body_json={"partial": True},
            source="profile",
        )
    if outcome == ToolMockOutcome.TIMEOUT:
        return ToolSandboxResolutionResult(
            outcome=outcome,
            status_code=504,
            error_message="Mock timeout",
            source="profile",
        )
    if outcome == ToolMockOutcome.HTTP_4XX:
        return ToolSandboxResolutionResult(
            outcome=outcome,
            status_code=400,
            error_message="Mock client error",
            source="profile",
        )
    if outcome == ToolMockOutcome.HTTP_5XX:
        return ToolSandboxResolutionResult(
            outcome=outcome,
            status_code=500,
            error_message="Mock server error",
            source="profile",
        )
    return ToolSandboxResolutionResult(
        outcome=ToolMockOutcome.MALFORMED,
        status_code=200,
        body_json={"malformed": "payload"},
        error_message="Malformed mock response",
        source="profile",
    )


@dataclass(slots=True)
class SeededDeterministicSandboxResolver(DeterministicSandboxResolver):
    """Reference resolver implementing precedence contract.

    Precedence:
    1) Per-turn override
    2) Rule match
    3) Seeded profile draw
    """

    def resolve(
        self, payload: ToolSandboxResolutionInput
    ) -> ToolSandboxResolutionResult:
        for override in payload.overrides:
            if (
                override.turn_id == payload.turn_id
                and override.tool_id == payload.tool_id
            ):
                result = _default_outcome_payload(override.outcome)
                result.source = "override"
                return result

        for rule in payload.rules:
            if rule.tool_id != payload.tool_id:
                continue
            if _rule_matches(rule.condition, payload.args_json):
                result = _default_outcome_payload(rule.outcome)
                result.source = "rule"
                return result

        weights = payload.profile_weights
        if weights == type(weights)():
            # Use profile-specific defaults when caller did not override weights.
            weights = default_profile_weights(payload.scenario_profile)

        weighted = [
            (ToolMockOutcome.SUCCESS, weights.success),
            (ToolMockOutcome.EMPTY, weights.empty),
            (ToolMockOutcome.PARTIAL, weights.partial),
            (ToolMockOutcome.TIMEOUT, weights.timeout),
            (ToolMockOutcome.HTTP_4XX, weights.http_4xx),
            (ToolMockOutcome.HTTP_5XX, weights.http_5xx),
            (ToolMockOutcome.MALFORMED, weights.malformed),
        ]

        material = (
            f"{payload.seed}:{payload.scenario_profile.value}:{payload.turn_id}:"
            f"{payload.tool_call_index}:{payload.tool_id}"
        )
        rng = random.Random(_stable_seed_int(material))
        threshold = rng.random() * sum(weight for _, weight in weighted)
        cumulative = 0.0
        for outcome, weight in weighted:
            cumulative += weight
            if threshold <= cumulative:
                return _default_outcome_payload(outcome)

        return _default_outcome_payload(ToolMockOutcome.SUCCESS)
