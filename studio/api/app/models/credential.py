from __future__ import annotations

import uuid
from datetime import datetime
from typing import TYPE_CHECKING

from sqlalchemy import Boolean, DateTime, ForeignKey, Integer, String, Text, Uuid, func
from sqlalchemy.orm import Mapped, mapped_column

from app.lib.database import Base

if TYPE_CHECKING:
    pass  # agent relationship kept optional; agent_id may be NULL for defaults

# AES-256-GCM via the Rust `integrations` crate.
# Bump this when the encryption scheme changes and update the
# corresponding Rust EncryptionEngine implementation.
CURRENT_ENCRYPTION_VERSION = 2


class Credential(Base):
    """An encrypted credential for an external integration.

    Encryption is handled by the Rust ``integrations.EncryptionEngine``
    (AES-256-GCM, version 2).

    Refresh tracking:
      - ``last_refresh_success`` / ``last_refresh_failure``: both timestamps
        tracked separately so we know when a credential last worked.
      - ``refresh_attempts``: incremented once per **day**, not per attempt,
        to prevent a provider outage from instantly exhausting retries.
      - ``refresh_exhausted``: set to True after ``MAX_CONSECUTIVE_DAYS``
        (default 4) consecutive days of failure.
      - ``last_fetched_at``: when these credentials were last resolved for a
        session; refresh cron skips connections not fetched recently.
    """

    __tablename__ = "credentials"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    agent_id: Mapped[uuid.UUID | None] = mapped_column(
        Uuid,
        ForeignKey("agents.id", ondelete="CASCADE"),
        nullable=True,  # NULL = platform-wide default connection (no specific agent)
    )
    name: Mapped[str] = mapped_column(String(255), nullable=False)
    provider: Mapped[str] = mapped_column(String(100), nullable=False)
    auth_type: Mapped[str] = mapped_column(String(50), nullable=False)
    encrypted_data: Mapped[str] = mapped_column(Text, nullable=False)

    # Encryption metadata
    encryption_iv: Mapped[str | None] = mapped_column(Text, nullable=True)
    encryption_version: Mapped[int] = mapped_column(
        Integer, nullable=False, server_default="1"
    )

    # Token lifecycle
    token_expires_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True, default=None
    )

    # Refresh tracking
    last_refresh_success: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    last_refresh_failure: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    last_refresh_error: Mapped[str | None] = mapped_column(Text, nullable=True)
    refresh_attempts: Mapped[int] = mapped_column(
        Integer, nullable=False, server_default="0"
    )
    refresh_exhausted: Mapped[bool] = mapped_column(
        Boolean, nullable=False, server_default="false"
    )

    # Stale connection tracking
    last_fetched_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )

    # Timestamps
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now()
    )

    def __repr__(self) -> str:
        owner = f"agent={self.agent_id}" if self.agent_id else "DEFAULT"
        return f"<Credential {self.provider}/{self.name} for {owner}>"
