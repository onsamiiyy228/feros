"""Admin CRUD endpoints for managing platform-level OAuth app registrations.

These endpoints let admins register/list/delete OAuth client credentials
for integrations (e.g. Airtable, Slack) without redeploying.
"""

from __future__ import annotations

import uuid

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.lib.database import get_db
from app.lib.oauth_apps import encrypt_client_secret
from app.models.oauth_app import OAuthApp

router = APIRouter(prefix="/oauth-apps", tags=["oauth-apps"])


# ── Request / Response schemas ───────────────────────────────────


class OAuthAppCreate(BaseModel):
    """Request body for registering an OAuth app."""

    integration_name: str = Field(
        ..., min_length=1, max_length=100, description="e.g. 'airtable', 'slack'"
    )
    client_id: str = Field(..., min_length=1, max_length=255)
    client_secret: str = Field(..., min_length=1)
    enabled: bool = True


class OAuthAppResponse(BaseModel):
    """Public representation of an OAuth app (no secrets)."""

    id: uuid.UUID
    integration_name: str
    client_id: str
    enabled: bool

    model_config = {"from_attributes": True}


# ── Routes ───────────────────────────────────────────────────────


@router.get("")
async def list_oauth_apps(
    db: AsyncSession = Depends(get_db),
) -> list[OAuthAppResponse]:
    """List all registered OAuth apps (secrets are never returned)."""
    result = await db.execute(select(OAuthApp).order_by(OAuthApp.integration_name))
    apps = result.scalars().all()
    return [OAuthAppResponse.model_validate(app) for app in apps]


@router.post("", status_code=201)
async def create_oauth_app(
    body: OAuthAppCreate,
    db: AsyncSession = Depends(get_db),
) -> OAuthAppResponse:
    """Register or update an OAuth app for an integration.

    If an app for this integration already exists, it is updated.
    The client_secret is encrypted before storage.
    """
    ciphertext, iv = encrypt_client_secret(body.client_secret)

    # Upsert: update if exists, create if not
    result = await db.execute(
        select(OAuthApp).where(OAuthApp.integration_name == body.integration_name)
    )
    existing = result.scalar_one_or_none()

    if existing:
        existing.client_id = body.client_id
        existing.client_secret_encrypted = ciphertext
        existing.client_secret_iv = iv
        existing.enabled = body.enabled
        await db.commit()
        return OAuthAppResponse.model_validate(existing)

    app = OAuthApp(
        integration_name=body.integration_name,
        client_id=body.client_id,
        client_secret_encrypted=ciphertext,
        client_secret_iv=iv,
        enabled=body.enabled,
    )
    db.add(app)
    await db.commit()
    return OAuthAppResponse.model_validate(app)


@router.delete("/{integration_name}", status_code=204)
async def delete_oauth_app(
    integration_name: str,
    db: AsyncSession = Depends(get_db),
) -> None:
    """Remove an OAuth app registration."""
    result = await db.execute(
        delete(OAuthApp).where(OAuthApp.integration_name == integration_name)
    )
    if getattr(result, "rowcount", 0) == 0:
        raise HTTPException(
            status_code=404,
            detail=f"OAuth app '{integration_name}' not found",
        )


class OAuthAppUpdate(BaseModel):
    """Request body for partially updating an OAuth app."""

    client_id: str | None = Field(default=None, min_length=1, max_length=255)
    client_secret: str | None = Field(default=None, min_length=1)
    enabled: bool | None = None


@router.patch("/{integration_name}")
async def update_oauth_app(
    integration_name: str,
    body: OAuthAppUpdate,
    db: AsyncSession = Depends(get_db),
) -> OAuthAppResponse:
    """Partially update an existing OAuth app.

    Only fields that are not None will be updated.
    Omit client_secret to leave the encrypted secret unchanged.
    """
    result = await db.execute(
        select(OAuthApp).where(OAuthApp.integration_name == integration_name)
    )
    app = result.scalar_one_or_none()
    if not app:
        raise HTTPException(
            status_code=404,
            detail=f"OAuth app '{integration_name}' not found",
        )

    if body.client_id is not None:
        app.client_id = body.client_id
    if body.client_secret is not None:
        ciphertext, iv = encrypt_client_secret(body.client_secret)
        app.client_secret_encrypted = ciphertext
        app.client_secret_iv = iv
    if body.enabled is not None:
        app.enabled = body.enabled

    await db.commit()
    return OAuthAppResponse.model_validate(app)
