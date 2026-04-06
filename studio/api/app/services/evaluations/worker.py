"""Inline background worker strategy for evaluation runs.

Phase 1 chooses in-process asyncio tasks for simplicity.
Can be swapped to a queue worker later without changing callers.
"""

from __future__ import annotations

import asyncio
import multiprocessing
import uuid
from collections.abc import Callable, Coroutine
from typing import Any

RunCoroutineFactory = Callable[[], Coroutine[Any, Any, None]]


class InlineEvaluationWorker:
    def __init__(self) -> None:
        self._tasks: dict[uuid.UUID, asyncio.Task[None]] = {}
        self._processes: dict[uuid.UUID, multiprocessing.Process] = {}
        self._mp_ctx = multiprocessing.get_context("spawn")

    def submit(self, run_id: uuid.UUID, runner: RunCoroutineFactory) -> None:
        if run_id in self._tasks and not self._tasks[run_id].done():
            return
        task: asyncio.Task[None] = asyncio.create_task(runner())
        self._tasks[run_id] = task
        task.add_done_callback(lambda _: self._tasks.pop(run_id, None))

    def submit_process(
        self,
        run_id: uuid.UUID,
        target: Callable[..., None],
        *args: object,
    ) -> None:
        self._reap_processes()
        proc = self._processes.get(run_id)
        if proc and proc.is_alive():
            return
        new_proc = self._mp_ctx.Process(
            target=target,
            args=args,
            daemon=True,
        )
        new_proc.start()
        # Safe cast: SpawnProcess is a subclass of Process

        self._processes[run_id] = new_proc  # type: ignore

    def _reap_processes(self) -> None:
        stale: list[uuid.UUID] = []
        for run_id, proc in self._processes.items():
            if proc.is_alive():
                continue
            proc.join(timeout=0)
            stale.append(run_id)
        for run_id in stale:
            self._processes.pop(run_id, None)

    def cancel(self, run_id: uuid.UUID) -> bool:
        task = self._tasks.get(run_id)
        cancelled = False
        if task and not task.done():
            task.cancel()
            cancelled = True

        self._reap_processes()
        proc = self._processes.get(run_id)
        if proc and proc.is_alive():
            proc.terminate()
            proc.join(timeout=1.0)
            self._processes.pop(run_id, None)
            cancelled = True
        return cancelled

    def is_running(self, run_id: uuid.UUID) -> bool:
        self._reap_processes()
        task = self._tasks.get(run_id)
        if task and not task.done():
            return True
        proc = self._processes.get(run_id)
        return bool(proc and proc.is_alive())


evaluation_worker = InlineEvaluationWorker()
