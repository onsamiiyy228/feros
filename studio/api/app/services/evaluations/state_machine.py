"""Run lifecycle transitions for evaluation execution."""

from __future__ import annotations

from app.schemas.evaluation import EvaluationRunStatus

_ALLOWED_TRANSITIONS: dict[EvaluationRunStatus, set[EvaluationRunStatus]] = {
    EvaluationRunStatus.QUEUED: {
        EvaluationRunStatus.RUNNING,
        EvaluationRunStatus.CANCELLED,
    },
    EvaluationRunStatus.RUNNING: {
        EvaluationRunStatus.COMPLETED,
        EvaluationRunStatus.FAILED,
        EvaluationRunStatus.CANCELLED,
    },
    EvaluationRunStatus.COMPLETED: set(),
    EvaluationRunStatus.FAILED: set(),
    EvaluationRunStatus.CANCELLED: set(),
}


class InvalidRunTransitionError(ValueError):
    pass


class EvaluationRunStateMachine:
    @staticmethod
    def can_transition(
        current: EvaluationRunStatus, target: EvaluationRunStatus
    ) -> bool:
        return target in _ALLOWED_TRANSITIONS[current]

    @staticmethod
    def assert_transition(
        current: EvaluationRunStatus, target: EvaluationRunStatus
    ) -> None:
        if not EvaluationRunStateMachine.can_transition(current, target):
            raise InvalidRunTransitionError(
                f"Invalid transition: {current.value} -> {target.value}"
            )
