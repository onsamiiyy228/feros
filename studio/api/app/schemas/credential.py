"""Pydantic schemas for credential management.

Secrets flow in via ``CredentialCreate.data`` and are encrypted before
reaching the database. They are NEVER returned in API responses.
"""

from __future__ import annotations

import uuid
from datetime import datetime

from pydantic import BaseModel, Field


class CredentialCreate(BaseModel):
    """Request body to store a new credential."""

    name: str = Field(
        ..., min_length=1, max_length=255, description="User-facing label"
    )
    provider: str = Field(
        ..., min_length=1, max_length=100, description="Skill name, e.g. 'airtable'"
    )
    auth_type: str = Field(
        ..., description="One of: api_key, bearer_token, oauth2, basic_auth"
    )
    data: dict[str, str] = Field(
        ..., description="Plaintext credential fields — encrypted before storage"
    )


class CredentialUpdate(BaseModel):
    """Request body to update an existing credential."""

    name: str | None = Field(
        default=None, min_length=1, max_length=255, description="Updated label"
    )
    data: dict[str, str] | None = Field(
        default=None, description="New credential fields — re-encrypted before storage"
    )


class CredentialResponse(BaseModel):
    """Credential metadata returned to the frontend — secrets omitted."""

    id: uuid.UUID
    agent_id: uuid.UUID | None  # None for default connections
    name: str
    provider: str
    auth_type: str
    token_expires_at: datetime | None = None
    created_at: datetime
    updated_at: datetime
    # True when this row is the platform default (agent_id IS NULL),
    # meaning the agent inherits it unless it has its own override.
    is_default: bool = False

    model_config = {"from_attributes": True}


class CredentialListResponse(BaseModel):
    """Paginated list of credentials for an agent."""

    credentials: list[CredentialResponse]
    total: int


# ── Default Connection schemas ────────────────────────────────────


class DefaultConnectionUpsert(BaseModel):
    """Request to create or replace a platform-wide default connection."""

    auth_type: str = Field(..., description="api_key | oauth2 | bearer_token")
    data: dict[str, str] = Field(
        ..., description="Plaintext fields — encrypted before storage"
    )


class DefaultConnectionResponse(BaseModel):
    """Platform-wide default connection metadata — secrets omitted."""

    id: uuid.UUID
    provider: str
    auth_type: str
    is_configured: bool = True
    created_at: datetime
    updated_at: datetime

    model_config = {"from_attributes": True}
