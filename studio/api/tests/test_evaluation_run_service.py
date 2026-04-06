from __future__ import annotations

import uuid

import pytest

from app.models.evaluation import EvaluationRun, EvaluationRunStatus
from app.services.evaluations.run_service import EvaluationRunService


class _FakeSession:
    async def flush(self) -> None:
        return


@pytest.mark.asyncio
async def test_run_service_mark_running_and_completed() -> None:
    db = _FakeSession()
    svc = EvaluationRunService()
    run = EvaluationRun(
        id=uuid.uuid4(),
        agent_id=uuid.uuid4(),
        config_id=uuid.uuid4(),
        config_version_id=uuid.uuid4(),
        status=EvaluationRunStatus.QUEUED,
        seed=42,
    )
    await svc.mark_running(db, run)
    assert run.status == EvaluationRunStatus.RUNNING
    await svc.mark_completed(db, run, aggregate_score=91.2, summary="ok")
    assert run.status == EvaluationRunStatus.COMPLETED
    assert run.aggregate_score == 91.2


@pytest.mark.asyncio
async def test_run_service_cancel_from_running() -> None:
    db = _FakeSession()
    svc = EvaluationRunService()
    run = EvaluationRun(
        id=uuid.uuid4(),
        agent_id=uuid.uuid4(),
        config_id=uuid.uuid4(),
        config_version_id=uuid.uuid4(),
        status=EvaluationRunStatus.RUNNING,
        seed=42,
    )
    await svc.cancel_run(db, run)
    assert run.status == EvaluationRunStatus.CANCELLED
