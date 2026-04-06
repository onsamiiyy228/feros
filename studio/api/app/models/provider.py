import uuid
from datetime import datetime
from typing import Any

from sqlalchemy import Boolean, DateTime, String, Uuid, func
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import Mapped, mapped_column

from app.lib.database import Base


class ProviderConfig(Base):
    """Configuration for an STT/LLM/TTS/Telephony provider."""

    __tablename__ = "provider_configs"

    id: Mapped[uuid.UUID] = mapped_column(Uuid, primary_key=True, default=uuid.uuid4)
    provider_type: Mapped[str] = mapped_column(
        String(20), nullable=False
    )  # stt | llm | tts | telephony
    provider_name: Mapped[str] = mapped_column(
        String(100), nullable=False
    )  # e.g. "openai", "ollama", "twilio"
    display_name: Mapped[str] = mapped_column(String(255), nullable=False)
    config_json: Mapped[dict[str, Any]] = mapped_column(
        JSONB, nullable=False, default=dict
    )  # API keys, endpoints, etc.
    is_default: Mapped[bool] = mapped_column(Boolean, default=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now()
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now()
    )

    def __repr__(self) -> str:
        return f"<ProviderConfig {self.provider_type}/{self.provider_name}>"
