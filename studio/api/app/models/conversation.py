from __future__ import annotations

import uuid
from datetime import datetime
from typing import TYPE_CHECKING, Any

from sqlalchemy import DateTime, ForeignKey, String, Uuid, func
from sqlalchemy.dialects.postgresql import JSON
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.lib.database import Base

if TYPE_CHECKING:
    from app.models.agent import Agent, AgentVersion


class BuilderConversation(Base):
    """The vibe-code conversation that produces/refines an agent."""

    __tablename__ = "builder_conversations"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    agent_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("agents.id", ondelete="CASCADE"),
        nullable=False,
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )
    # Compressed LLM message history for replay (serialized ModelMessage list).
    # Grows with each turn but bounded by compression.
    persisted_context: Mapped[list[dict[str, Any]] | None] = mapped_column(
        JSON, nullable=True, default=None
    )

    # Relationships
    agent: Mapped[Agent] = relationship("Agent", back_populates="builder_conversations")
    messages: Mapped[list[BuilderMessage]] = relationship(
        back_populates="conversation",
        cascade="all, delete-orphan",
        order_by="BuilderMessage.created_at",
    )

    def __repr__(self) -> str:
        return f"<BuilderConversation agent={self.agent_id}>"


class BuilderMessage(Base):
    """A single message in a builder conversation."""

    __tablename__ = "builder_messages"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    conversation_id: Mapped[uuid.UUID] = mapped_column(
        Uuid,
        ForeignKey("builder_conversations.id", ondelete="CASCADE"),
        nullable=False,
    )
    role: Mapped[str] = mapped_column(String(20), nullable=False)  # user | assistant
    parts: Mapped[list[dict[str, Any]] | None] = mapped_column(
        JSON, nullable=True, default=None
    )
    agent_version_id: Mapped[uuid.UUID | None] = mapped_column(
        Uuid,
        ForeignKey("agent_versions.id", ondelete="SET NULL"),
        nullable=True,
    )
    metadata_json: Mapped[dict[str, Any] | None] = mapped_column(
        JSON, nullable=True, default=None
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )

    # Relationships
    conversation: Mapped[BuilderConversation] = relationship(back_populates="messages")
    agent_version: Mapped[AgentVersion | None] = relationship(
        "AgentVersion", back_populates="builder_messages"
    )

    def __repr__(self) -> str:
        return f"<BuilderMessage {self.role} in {self.conversation_id}>"
