"""Pydantic schemas for the Phone Number management API."""

from __future__ import annotations

import uuid
from datetime import datetime

from pydantic import BaseModel, Field


class PhoneNumberResponse(BaseModel):
    """A phone number entry returned by the API."""

    id: uuid.UUID
    provider: str
    phone_number: str
    provider_sid: str | None
    friendly_name: str | None
    agent_id: uuid.UUID | None
    voice_server_url: str | None
    telnyx_connection_id: str | None
    is_active: bool
    has_credentials: bool = False
    created_at: datetime
    updated_at: datetime

    model_config = {"from_attributes": True}


class PhoneNumberListResponse(BaseModel):
    """Paginated list of phone numbers."""

    phone_numbers: list[PhoneNumberResponse]
    total: int


class FetchNumbersRequest(BaseModel):
    """Request to fetch available numbers from a provider using inline credentials."""

    provider: str = Field(
        ...,
        pattern="^(twilio|telnyx)$",
    )
    twilio_account_sid: str = ""
    twilio_auth_token: str = ""
    telnyx_api_key: str = ""


class ProviderNumber(BaseModel):
    """A phone number fetched from the provider, before import."""

    phone_number: str
    provider_sid: str
    friendly_name: str
    locality: str = ""
    region: str = ""
    number_type: str = ""
    already_imported: bool
    disabled_reason: str = ""


class FetchNumbersResponse(BaseModel):
    """Response from fetching numbers from a provider."""

    numbers: list[ProviderNumber]


class ImportNumbersRequest(BaseModel):
    """Import selected numbers with inline credentials."""

    provider: str = Field(
        ...,
        pattern="^(twilio|telnyx)$",
    )
    twilio_account_sid: str = ""
    twilio_auth_token: str = ""
    telnyx_api_key: str = ""
    selected_numbers: list[str]  # E.164 phone numbers to import


class AssignPhoneNumberRequest(BaseModel):
    """Assign (or unassign) a phone number to/from an agent.

    To unassign, pass agent_id=null.
    To assign, agent_id is required. voice_server_url is read from global Settings
    and persisted onto the phone number as a snapshot.
    """

    agent_id: uuid.UUID | None = None
    telnyx_connection_id: str | None = Field(
        default=None,
        description="Telnyx Voice API Application (connection) ID — required for Telnyx numbers",
    )
