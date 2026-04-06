"""Integration Registry — reads integration definitions from integrations.yaml.

Used by the API and OAuth engine to:
  - Serve integration metadata to the frontend
  - Load credential form schemas
  - Load full integration config for OAuth flows and token refresh
"""

from __future__ import annotations

import tempfile
from typing import Any

import yaml
from loguru import logger
from pydantic import BaseModel, Field

try:
    import integrations as _integrations

    _INTEGRATIONS_YAML: str = _integrations.embedded_integrations_yaml()
except ImportError as exc:
    raise RuntimeError(
        "integrations extension is required to load the embedded integrations registry"
    ) from exc

with tempfile.NamedTemporaryFile(
    mode="w", suffix="_integrations.yaml", delete=False, encoding="utf-8"
) as _temp:
    _temp.write(_INTEGRATIONS_YAML)
    _temp.flush()
    INTEGRATIONS_PATH: str = _temp.name


# ── Pydantic models mirroring integrations.yaml schema ───────────


class FieldDef(BaseModel):
    """A single credential or connection config field."""

    type: str = "string"
    title: str = ""
    description: str = ""
    secret: bool = False
    required: bool = True
    example: str = ""
    doc_url: str | None = None


class ApiConfig(BaseModel):
    """API endpoint configuration for an integration."""

    base_url: str = ""
    headers: dict[str, str] = Field(default_factory=dict)


class AuthConfig(BaseModel):
    """Authentication configuration for an integration."""

    type: str  # "oauth2", "bearer_token", "api_key", "bot_token"
    client_auth_method: str = Field(default="body")
    authorization_url: str | None = None
    token_url: str | None = None
    # Use Any values to accommodate YAML booleans/ints (e.g. Square's `session: false`)
    authorization_params: dict[str, Any] = Field(default_factory=dict)
    token_params: dict[str, Any] = Field(default_factory=dict)
    refresh_params: dict[str, Any] = Field(default_factory=dict)
    default_scopes: list[str] = Field(default_factory=list)
    scope_separator: str = " "
    pkce: bool = True
    token_expires_in_seconds: int | None = None
    refresh_margin_minutes: int | None = None


class IntegrationConfig(BaseModel):
    """A single integration entry from integrations.yaml."""

    display_name: str
    description: str = ""
    categories: list[str] = Field(default_factory=list)
    icon: str = ""
    auth: AuthConfig
    credentials: dict[str, FieldDef] = Field(default_factory=dict)
    connection_config: dict[str, FieldDef] = Field(default_factory=dict)
    api: ApiConfig | None = None


# ── API response models ──────────────────────────────────────────


class IntegrationSummary(BaseModel):
    """Level 1 metadata — shown in frontend sidebar."""

    name: str
    display_name: str
    description: str
    auth_type: str
    categories: list[str]
    icon: str
    supports_byok: bool


class CredentialFieldSchema(BaseModel):
    """A single field in the credential collection form."""

    key: str
    label: str
    type: str  # "text" or "password"
    required: bool
    placeholder: str
    help_text: str
    help_url: str | None = None


class CredentialSchema(BaseModel):
    """Full credential form schema for an integration."""

    provider: str
    display_name: str
    icon: str
    auth_type: str
    fields: list[CredentialFieldSchema]
    authorization_url: str | None = None
    token_url: str | None = None
    default_scopes: list[str] = Field(default_factory=list)
    oauth_available: bool = False


# ── Registry ─────────────────────────────────────────────────────


class IntegrationRegistry:
    """Load and cache integration definitions from integrations.yaml."""

    def __init__(self) -> None:
        self._registry: dict[str, IntegrationConfig] | None = None

    def _load_registry(self) -> dict[str, IntegrationConfig]:
        """Load and cache the embedded integrations registry."""
        if self._registry is not None:
            return self._registry

        try:
            data: dict[str, Any] = yaml.safe_load(_INTEGRATIONS_YAML)
            raw_integrations = data.get("integrations", {})
            self._registry = {
                name: IntegrationConfig.model_validate(cfg)
                for name, cfg in raw_integrations.items()
            }
            logger.info(
                "Loaded {} integrations from embedded registry",
                len(self._registry),
            )
            return self._registry
        except Exception:
            logger.exception("Failed to parse embedded integrations registry")
            self._registry = {}
            return self._registry

    def list_integrations(self) -> list[IntegrationSummary]:
        """List all integrations as Level 1 metadata (for frontend sidebar)."""
        registry = self._load_registry()
        return [
            IntegrationSummary(
                name=name,
                display_name=config.display_name,
                description=config.description,
                auth_type=config.auth.type,
                categories=config.categories,
                icon=config.icon,
                supports_byok=bool(config.credentials or config.connection_config),
            )
            for name, config in registry.items()
        ]

    def load_credential_schema(self, name: str) -> CredentialSchema | None:
        """Load the credential form schema for an integration.

        Converts the credentials/connection_config sections into
        a flat fields list compatible with the existing frontend format.
        """
        registry = self._load_registry()
        config = registry.get(name)
        if config is None:
            return None

        fields = self._build_credential_fields(config)

        return CredentialSchema(
            provider=name,
            display_name=config.display_name,
            icon=config.icon,
            auth_type=config.auth.type,
            fields=fields,
            authorization_url=config.auth.authorization_url,
            token_url=config.auth.token_url,
            default_scopes=config.auth.default_scopes,
        )

    def _build_credential_fields(
        self, config: IntegrationConfig
    ) -> list[CredentialFieldSchema]:
        def _credential_field(key: str, field_def: FieldDef) -> CredentialFieldSchema:
            return CredentialFieldSchema(
                key=key,
                label=field_def.title or key,
                type="password" if field_def.secret else "text",
                required=field_def.required,
                placeholder=field_def.example,
                help_text=field_def.description,
                help_url=field_def.doc_url,
            )

        def _connection_config_field(
            key: str, field_def: FieldDef
        ) -> CredentialFieldSchema:
            return CredentialFieldSchema(
                key=key,
                label=field_def.title or key,
                type="text",
                required=field_def.required,
                placeholder=field_def.example,
                help_text=field_def.description,
            )

        credential_fields = [
            _credential_field(key, field_def)
            for key, field_def in config.credentials.items()
        ]
        connection_fields = [
            _connection_config_field(key, field_def)
            for key, field_def in config.connection_config.items()
        ]

        header_fields = [
            field for field in connection_fields if self._is_header_like_field(field)
        ]
        if not header_fields:
            return credential_fields + connection_fields

        remaining_connection_fields = [
            field
            for field in connection_fields
            if not self._is_header_like_field(field)
        ]
        return header_fields + credential_fields + remaining_connection_fields

    @staticmethod
    def _is_header_like_field(field: CredentialFieldSchema) -> bool:
        normalized = f"{field.key} {field.label}".lower()
        return "header" in normalized

    def load_integration_config(self, name: str) -> IntegrationConfig | None:
        """Load full integration config (used by OAuth engine, refresh service)."""
        registry = self._load_registry()
        return registry.get(name)


# Module-level singleton
integration_registry = IntegrationRegistry()
