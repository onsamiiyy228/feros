"""OAuth Apps — DB lookup helpers for platform-level OAuth client credentials.

Provides functions to load and decrypt OAuth app credentials from the
``oauth_apps`` table, replacing the old environment-variable approach.
"""

from __future__ import annotations

from typing import cast

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

import integrations
from app.lib import get_settings
from app.models.oauth_app import OAuthApp


async def get_oauth_app(
    integration_name: str,
    db: AsyncSession,
) -> OAuthApp | None:
    """Look up an enabled OAuth app by integration name."""
    result = await db.execute(
        select(OAuthApp).where(
            OAuthApp.integration_name == integration_name,
            OAuthApp.enabled.is_(True),
        )
    )
    return result.scalar_one_or_none()


async def get_all_enabled_oauth_apps(
    db: AsyncSession,
) -> list[OAuthApp]:
    """Load all enabled OAuth apps."""
    result = await db.execute(select(OAuthApp).where(OAuthApp.enabled.is_(True)))
    return list(result.scalars().all())


def decrypt_client_secret(app: OAuthApp) -> str:
    """Decrypt the client_secret from an OAuthApp row."""
    engine = integrations.EncryptionEngine(get_settings().auth.secret_key)
    data = cast(
        dict[str, str],
        engine.decrypt(app.client_secret_encrypted, app.client_secret_iv),
    )
    return data.get("client_secret", "")


def encrypt_client_secret(client_secret: str) -> tuple[str, str]:
    """Encrypt a client_secret for storage. Returns (ciphertext, iv)."""
    engine = integrations.EncryptionEngine(get_settings().auth.secret_key)
    ciphertext, iv = engine.encrypt({"client_secret": client_secret})
    return ciphertext, iv


async def get_oauth_client_credentials(
    integration_name: str,
    db: AsyncSession,
) -> tuple[str, str]:
    """Get (client_id, client_secret) for an integration. Returns ("", "") if not found."""
    app = await get_oauth_app(integration_name, db)
    if not app:
        return "", ""
    return app.client_id, decrypt_client_secret(app)
