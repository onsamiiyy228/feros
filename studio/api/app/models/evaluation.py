from __future__ import annotations

import uuid
from datetime import datetime
from enum import StrEnum
from typing import TYPE_CHECKING, Any

from sqlalchemy import (
    DateTime,
    Float,
    ForeignKey,
    Integer,
    String,
    Text,
    UniqueConstraint,
    Uuid,
    func,
    text,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.lib.database import Base

if TYPE_CHECKING:
    from app.models.agent import Agent


class EvaluationConfigStatus(StrEnum):
    ACTIVE = "active"
    ARCHIVED = "archived"


class EvaluationRunStatus(StrEnum):
    QUEUED = "queued"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


class EvaluationConfig(Base):
    __tablename__ = "evaluation_configs"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    agent_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("agents.id", ondelete="CASCADE"),
        nullable=False,
    )
    name: Mapped[str] = mapped_column(String(255), nullable=False)
    status: Mapped[EvaluationConfigStatus] = mapped_column(
        String(20), nullable=False, default=EvaluationConfigStatus.ACTIVE
    )
    latest_version: Mapped[int] = mapped_column(Integer, nullable=False, default=1)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now()
    )

    agent: Mapped[Agent] = relationship("Agent", back_populates="evaluation_configs")
    versions: Mapped[list[EvaluationConfigVersion]] = relationship(
        back_populates="config",
        cascade="all, delete-orphan",
        order_by="EvaluationConfigVersion.version",
    )
    runs: Mapped[list[EvaluationRun]] = relationship(
        back_populates="config",
        cascade="all, delete-orphan",
    )


class EvaluationConfigVersion(Base):
    __tablename__ = "evaluation_config_versions"
    __table_args__ = (UniqueConstraint("config_id", "version"),)

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    config_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("evaluation_configs.id", ondelete="CASCADE"),
        nullable=False,
    )
    version: Mapped[int] = mapped_column(Integer, nullable=False)
    config_json: Mapped[dict[str, Any]] = mapped_column(JSONB, nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )

    config: Mapped[EvaluationConfig] = relationship(back_populates="versions")
    runs: Mapped[list[EvaluationRun]] = relationship(back_populates="config_version")


class EvaluationRun(Base):
    __tablename__ = "evaluation_runs"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    agent_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("agents.id", ondelete="CASCADE"),
        nullable=False,
    )
    config_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("evaluation_configs.id", ondelete="CASCADE"),
        nullable=False,
    )
    config_version_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("evaluation_config_versions.id", ondelete="RESTRICT"),
        nullable=False,
    )
    target_agent_version: Mapped[int | None] = mapped_column(Integer, nullable=True)
    seed: Mapped[int] = mapped_column(Integer, nullable=False, default=42)
    status: Mapped[EvaluationRunStatus] = mapped_column(
        String(20), nullable=False, default=EvaluationRunStatus.QUEUED
    )
    aggregate_score: Mapped[float | None] = mapped_column(Float, nullable=True)
    summary: Mapped[str | None] = mapped_column(Text, nullable=True)
    started_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    ended_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now()
    )

    agent: Mapped[Agent] = relationship("Agent", back_populates="evaluation_runs")
    config: Mapped[EvaluationConfig] = relationship(back_populates="runs")
    config_version: Mapped[EvaluationConfigVersion] = relationship(
        back_populates="runs"
    )
    events: Mapped[list[EvaluationRunEvent]] = relationship(
        back_populates="run",
        cascade="all, delete-orphan",
        order_by="EvaluationRunEvent.seq_no",
    )
    judgments: Mapped[list[EvaluationJudgment]] = relationship(
        back_populates="run",
        cascade="all, delete-orphan",
    )


class EvaluationRunEvent(Base):
    __tablename__ = "evaluation_run_events"
    __table_args__ = (UniqueConstraint("run_id", "seq_no"),)

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    run_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("evaluation_runs.id", ondelete="CASCADE"),
        nullable=False,
    )
    seq_no: Mapped[int] = mapped_column(Integer, nullable=False)
    event_type: Mapped[str] = mapped_column(String(64), nullable=False)
    event_timestamp: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False
    )
    payload_json: Mapped[dict[str, Any]] = mapped_column(JSONB, nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )

    run: Mapped[EvaluationRun] = relationship(back_populates="events")


class EvaluationJudgment(Base):
    __tablename__ = "evaluation_judgments"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    run_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("evaluation_runs.id", ondelete="CASCADE"),
        nullable=False,
    )
    hard_checks: Mapped[dict[str, Any]] = mapped_column(
        JSONB, nullable=False, server_default=text("'{}'::jsonb")
    )
    rubric_scores: Mapped[dict[str, Any]] = mapped_column(
        JSONB, nullable=False, server_default=text("'{}'::jsonb")
    )
    summary: Mapped[str | None] = mapped_column(Text, nullable=True)
    failure_highlights: Mapped[list[str]] = mapped_column(
        JSONB, nullable=False, server_default=text("'[]'::jsonb")
    )
    recommendations: Mapped[list[str]] = mapped_column(
        JSONB, nullable=False, server_default=text("'[]'::jsonb")
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )

    run: Mapped[EvaluationRun] = relationship(back_populates="judgments")
