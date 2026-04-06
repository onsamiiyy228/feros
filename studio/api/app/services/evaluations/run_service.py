"""Run orchestration primitives for evaluation execution lifecycle."""

from __future__ import annotations

import uuid
from datetime import UTC, datetime

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.agent import Agent
from app.models.evaluation import (
    EvaluationConfig,
    EvaluationConfigVersion,
    EvaluationRun,
    EvaluationRunStatus,
)
from app.schemas.evaluation import EvaluationRunStatus as SchemaStatus
from app.services.evaluations.state_machine import EvaluationRunStateMachine
from app.services.evaluations.worker import evaluation_worker


class EvaluationRunService:
    async def create_run(
        self,
        db: AsyncSession,
        *,
        agent_id: uuid.UUID,
        config_id: uuid.UUID,
        version: int | None = None,
        seed_override: int | None = None,
    ) -> EvaluationRun | None:
        cfg_stmt = select(EvaluationConfig).where(
            EvaluationConfig.id == config_id,
            EvaluationConfig.agent_id == agent_id,
        )
        config = await db.scalar(cfg_stmt)
        if not config:
            return None

        selected_version = version or config.latest_version
        version_stmt = select(EvaluationConfigVersion).where(
            EvaluationConfigVersion.config_id == config_id,
            EvaluationConfigVersion.version == selected_version,
        )
        config_version = await db.scalar(version_stmt)
        if not config_version:
            return None

        agent_stmt = select(Agent).where(Agent.id == agent_id)
        agent = await db.scalar(agent_stmt)
        target_agent_version = agent.active_version if agent else None
        payload_seed = int(config_version.config_json.get("seed", 42))
        run = EvaluationRun(
            agent_id=agent_id,
            config_id=config_id,
            config_version_id=config_version.id,
            target_agent_version=target_agent_version,
            seed=seed_override if seed_override is not None else payload_seed,
            status=EvaluationRunStatus.QUEUED,
        )
        db.add(run)
        await db.flush()
        return run

    async def mark_running(self, db: AsyncSession, run: EvaluationRun) -> EvaluationRun:
        EvaluationRunStateMachine.assert_transition(
            SchemaStatus(run.status), SchemaStatus.RUNNING
        )
        run.status = EvaluationRunStatus.RUNNING
        run.started_at = datetime.now(UTC)
        await db.flush()
        return run

    async def mark_completed(
        self,
        db: AsyncSession,
        run: EvaluationRun,
        *,
        aggregate_score: float | None = None,
        summary: str | None = None,
    ) -> EvaluationRun:
        EvaluationRunStateMachine.assert_transition(
            SchemaStatus(run.status), SchemaStatus.COMPLETED
        )
        run.status = EvaluationRunStatus.COMPLETED
        run.aggregate_score = aggregate_score
        run.summary = summary
        run.ended_at = datetime.now(UTC)
        await db.flush()
        return run

    async def mark_failed(
        self, db: AsyncSession, run: EvaluationRun, *, summary: str
    ) -> EvaluationRun:
        EvaluationRunStateMachine.assert_transition(
            SchemaStatus(run.status), SchemaStatus.FAILED
        )
        run.status = EvaluationRunStatus.FAILED
        run.summary = summary
        run.ended_at = datetime.now(UTC)
        await db.flush()
        return run

    async def cancel_run(self, db: AsyncSession, run: EvaluationRun) -> EvaluationRun:
        target = (
            SchemaStatus.CANCELLED
            if SchemaStatus(run.status) == SchemaStatus.QUEUED
            else SchemaStatus.CANCELLED
        )
        EvaluationRunStateMachine.assert_transition(SchemaStatus(run.status), target)
        run.status = EvaluationRunStatus.CANCELLED
        run.ended_at = datetime.now(UTC)
        evaluation_worker.cancel(run.id)
        await db.flush()
        return run
