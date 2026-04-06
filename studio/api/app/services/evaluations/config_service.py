"""Config/version persistence operations for evaluation templates."""

from __future__ import annotations

import uuid

from sqlalchemy import Select, func, select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.models.evaluation import (
    EvaluationConfig,
    EvaluationConfigStatus,
    EvaluationConfigVersion,
)
from app.schemas.evaluation import EvaluationConfigPayload


class EvaluationConfigService:
    async def create_config(
        self,
        db: AsyncSession,
        *,
        agent_id: uuid.UUID,
        name: str,
        payload: EvaluationConfigPayload,
    ) -> tuple[EvaluationConfig, EvaluationConfigVersion]:
        config = EvaluationConfig(
            agent_id=agent_id,
            name=name,
            status=EvaluationConfigStatus.ACTIVE,
            latest_version=1,
        )
        db.add(config)
        await db.flush()

        version = EvaluationConfigVersion(
            config_id=config.id,
            version=1,
            config_json=payload.model_dump(mode="json"),
        )
        db.add(version)
        await db.flush()
        return config, version

    async def create_version(
        self,
        db: AsyncSession,
        *,
        agent_id: uuid.UUID,
        config_id: uuid.UUID,
        payload: EvaluationConfigPayload,
    ) -> EvaluationConfigVersion | None:
        config = await self.get_config(db, agent_id=agent_id, config_id=config_id)
        if not config:
            return None

        next_version = config.latest_version + 1
        version = EvaluationConfigVersion(
            config_id=config.id,
            version=next_version,
            config_json=payload.model_dump(mode="json"),
        )
        db.add(version)
        config.latest_version = next_version
        await db.flush()
        return version

    async def list_configs(
        self,
        db: AsyncSession,
        *,
        agent_id: uuid.UUID,
        include_archived: bool = False,
        limit: int = 50,
        offset: int = 0,
    ) -> tuple[list[EvaluationConfig], int]:
        filters = [EvaluationConfig.agent_id == agent_id]
        if not include_archived:
            filters.append(EvaluationConfig.status == EvaluationConfigStatus.ACTIVE)

        stmt: Select[tuple[EvaluationConfig]] = (
            select(EvaluationConfig)
            .where(*filters)
            .order_by(EvaluationConfig.updated_at.desc())
            .limit(limit)
            .offset(offset)
        )
        items = list((await db.scalars(stmt)).all())

        count_stmt = select(func.count(EvaluationConfig.id)).where(*filters)
        total = int((await db.scalar(count_stmt)) or 0)
        return items, total

    async def get_config(
        self,
        db: AsyncSession,
        *,
        agent_id: uuid.UUID,
        config_id: uuid.UUID,
        include_versions: bool = False,
    ) -> EvaluationConfig | None:
        stmt: Select[tuple[EvaluationConfig]] = select(EvaluationConfig).where(
            EvaluationConfig.id == config_id,
            EvaluationConfig.agent_id == agent_id,
        )
        if include_versions:
            stmt = stmt.options(selectinload(EvaluationConfig.versions))
        return await db.scalar(stmt)  # type: ignore[no-any-return]

    async def get_version(
        self,
        db: AsyncSession,
        *,
        agent_id: uuid.UUID,
        config_id: uuid.UUID,
        version: int,
    ) -> EvaluationConfigVersion | None:
        stmt: Select[tuple[EvaluationConfigVersion]] = (
            select(EvaluationConfigVersion)
            .join(
                EvaluationConfig,
                EvaluationConfig.id == EvaluationConfigVersion.config_id,
            )
            .where(
                EvaluationConfig.agent_id == agent_id,
                EvaluationConfig.id == config_id,
                EvaluationConfigVersion.version == version,
            )
        )
        return await db.scalar(stmt)  # type: ignore[no-any-return]

    async def archive_config(
        self,
        db: AsyncSession,
        *,
        agent_id: uuid.UUID,
        config_id: uuid.UUID,
    ) -> bool:
        config = await self.get_config(db, agent_id=agent_id, config_id=config_id)
        if not config:
            return False
        config.status = EvaluationConfigStatus.ARCHIVED
        await db.flush()
        return True
