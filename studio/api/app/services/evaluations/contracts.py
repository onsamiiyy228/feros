"""Protocol contracts for evaluation sandbox and judging components.

These interfaces intentionally avoid implementation details so
Phase 1+ can plug in concrete engines without changing call sites.
"""

from __future__ import annotations

from typing import Protocol

from app.schemas.evaluation import (
    EvaluationJudgeRequest,
    EvaluationJudgeResponse,
    ToolSandboxResolutionInput,
    ToolSandboxResolutionResult,
)


class DeterministicSandboxResolver(Protocol):
    """Resolve mocked tool results deterministically.

    Precedence is fixed by the contract:
    override > rule > seeded profile.
    """

    def resolve(
        self, payload: ToolSandboxResolutionInput
    ) -> ToolSandboxResolutionResult:
        """Return deterministic tool outcome for a single tool call."""


class EvaluationJudge(Protocol):
    """Score completed run traces and produce actionable feedback."""

    async def judge(self, payload: EvaluationJudgeRequest) -> EvaluationJudgeResponse:
        """Return rubric scores + qualitative summary + recommendations."""
