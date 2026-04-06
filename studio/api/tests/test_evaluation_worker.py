from __future__ import annotations

import asyncio
import uuid

import pytest

from app.services.evaluations.worker import InlineEvaluationWorker


@pytest.mark.asyncio
async def test_inline_worker_submit_and_complete() -> None:
    worker = InlineEvaluationWorker()
    run_id = uuid.uuid4()
    done = asyncio.Event()

    async def runner() -> None:
        await asyncio.sleep(0.01)
        done.set()

    worker.submit(run_id, runner)
    assert worker.is_running(run_id) is True
    await asyncio.wait_for(done.wait(), timeout=1.0)
    await asyncio.sleep(0)
    assert worker.is_running(run_id) is False


@pytest.mark.asyncio
async def test_inline_worker_cancel() -> None:
    worker = InlineEvaluationWorker()
    run_id = uuid.uuid4()
    started = asyncio.Event()

    async def runner() -> None:
        started.set()
        await asyncio.sleep(10)

    worker.submit(run_id, runner)
    await asyncio.wait_for(started.wait(), timeout=1.0)
    assert worker.cancel(run_id) is True
