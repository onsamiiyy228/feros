from __future__ import annotations

import pytest

from app.schemas.evaluation import EvaluationRunStatus
from app.services.evaluations.state_machine import (
    EvaluationRunStateMachine,
    InvalidRunTransitionError,
)


def test_valid_transitions() -> None:
    assert EvaluationRunStateMachine.can_transition(
        EvaluationRunStatus.QUEUED, EvaluationRunStatus.RUNNING
    )
    assert EvaluationRunStateMachine.can_transition(
        EvaluationRunStatus.RUNNING, EvaluationRunStatus.COMPLETED
    )
    assert EvaluationRunStateMachine.can_transition(
        EvaluationRunStatus.RUNNING, EvaluationRunStatus.CANCELLED
    )


def test_invalid_transition_raises() -> None:
    with pytest.raises(InvalidRunTransitionError):
        EvaluationRunStateMachine.assert_transition(
            EvaluationRunStatus.COMPLETED, EvaluationRunStatus.RUNNING
        )
