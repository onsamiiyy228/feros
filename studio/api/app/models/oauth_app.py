from __future__ import annotations

import uuid
from datetime import datetime

from sqlalchemy import Boolean, DateTime, String, Text, Uuid, func
from sqlalchemy.orm import Mapped, mapped_column

from app.lib.database import Base


class OAuthApp(Base):
    """Platform-level OAuth app registration for an integration.

    Stores the OAuth client_id and encrypted client_secret for each
    integration (e.g. "airtable", "slack"). One row per integration,
    shared across all agents.

    The client_secret is encrypted at rest using the same AES-256-GCM
    ``integrations.EncryptionEngine`` used for user credentials.
    """

    __tablename__ = "oauth_apps"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    integration_name: Mapped[str] = mapped_column(
        String(100), unique=True, nullable=False, index=True
    )
    client_id: Mapped[str] = mapped_column(String(255), nullable=False)
    client_secret_encrypted: Mapped[str] = mapped_column(Text, nullable=False)
    client_secret_iv: Mapped[str] = mapped_column(Text, nullable=False)
    enabled: Mapped[bool] = mapped_column(
        Boolean, nullable=False, server_default="true"
    )

    # Timestamps
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now()
    )

    def __repr__(self) -> str:
        return f"<OAuthApp {self.integration_name} enabled={self.enabled}>"
