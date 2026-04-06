"""Persistence helper for ordered evaluation run events."""

from __future__ import annotations

import uuid
from datetime import UTC, datetime
from typing import Any

from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.evaluation import EvaluationRunEvent
from app.schemas.evaluation import EvaluationRunEventType


class EvaluationRunEventRecorder:
    """Appends events with monotonic seq_no per run."""

    def __init__(self, db: AsyncSession, run_id: uuid.UUID) -> None:
        self._db = db
        self._run_id = run_id

    async def append(
        self,
        event_type: EvaluationRunEventType,
        payload_json: dict[str, Any],
        *,
        event_timestamp: datetime | None = None,
    ) -> EvaluationRunEvent:
        max_seq_stmt = select(
            func.coalesce(func.max(EvaluationRunEvent.seq_no), 0)
        ).where(EvaluationRunEvent.run_id == self._run_id)
        next_seq = int((await self._db.scalar(max_seq_stmt)) or 0) + 1

        model = EvaluationRunEvent(
            run_id=self._run_id,
            seq_no=next_seq,
            event_type=event_type.value,
            event_timestamp=event_timestamp or datetime.now(UTC),
            payload_json=payload_json,
        )
        self._db.add(model)
        await self._db.flush()
        return model
