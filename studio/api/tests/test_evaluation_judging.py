from __future__ import annotations

import uuid
from datetime import UTC, datetime

import pytest

from app.schemas.evaluation import (
    AssistantReplyEvent,
    EvaluationJudgeRequest,
    EvaluationRunEventType,
    RunFinishedEvent,
    ToolCallEvent,
    ToolMockOutcome,
    ToolMockResultEvent,
)
from app.services.evaluations.judging import (
    BaselineEvaluationJudge,
    evaluate_hard_checks,
)


def test_hard_checks_pass_with_minimal_successful_timeline() -> None:
    now = datetime.now(UTC)
    events = [
        AssistantReplyEvent(
            event_type=EvaluationRunEventType.ASSISTANT_REPLY,
            seq_no=1,
            timestamp=now,
            turn_id=1,
            text="Hello",
        ),
        ToolCallEvent(
            event_type=EvaluationRunEventType.TOOL_CALL,
            seq_no=2,
            timestamp=now,
            turn_id=1,
            tool_call_id="call_1",
            tool_id="calendar.check_availability",
            args_json={},
        ),
        ToolMockResultEvent(
            event_type=EvaluationRunEventType.TOOL_MOCK_RESULT,
            seq_no=3,
            timestamp=now,
            turn_id=1,
            tool_call_id="call_1",
            tool_id="calendar.check_availability",
            outcome=ToolMockOutcome.SUCCESS,
        ),
        RunFinishedEvent(
            event_type=EvaluationRunEventType.RUN_FINISHED,
            seq_no=4,
            timestamp=now,
            aggregate_score=90,
        ),
    ]
    checks = evaluate_hard_checks(events)
    assert checks["has_assistant_reply"] is True
    assert checks["tools_resolved"] is True
    assert checks["run_not_failed"] is True
    assert checks["run_finished"] is True


@pytest.mark.asyncio
async def test_baseline_judge_returns_scores() -> None:
    judge = BaselineEvaluationJudge()
    result = await judge.judge(
        EvaluationJudgeRequest(
            run_id=uuid.uuid4(),
            config={},
            transcript=[],
            tool_timeline=[],
            hard_check_results={
                "has_assistant_reply": True,
                "tools_resolved": False,
                "run_not_failed": True,
                "run_finished": True,
            },
        )
    )
    assert "task_completion" in result.rubric_scores
    assert result.summary
