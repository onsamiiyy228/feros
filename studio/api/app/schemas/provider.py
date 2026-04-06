"""Provider configuration Pydantic schemas."""

from __future__ import annotations

import uuid
from typing import Any

from pydantic import BaseModel, Field


class ProviderConfigCreate(BaseModel):
    provider_type: str = Field(..., pattern="^(stt|llm|tts|telephony)$")
    provider_name: str
    display_name: str
    config_json: dict[str, Any] = Field(default_factory=dict)
    is_default: bool = False


class ProviderConfigResponse(BaseModel):
    id: uuid.UUID
    provider_type: str
    provider_name: str
    display_name: str
    config_json: dict[str, Any]
    is_default: bool

    model_config = {"from_attributes": True}
