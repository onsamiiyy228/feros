"""Tests for the IntegrationRegistry service."""

from app.lib.integration_registry import (
    FieldDef,
    IntegrationConfig,
    IntegrationRegistry,
)


def test_load_credential_schema_missing() -> None:
    """Missing credential schemas should return None."""
    registry = IntegrationRegistry()
    schema = registry.load_credential_schema("nonexistent")
    assert schema is None


def test_load_credential_schema_from_registry() -> None:
    """Credential schemas should be loaded from integrations.yaml."""
    registry = IntegrationRegistry()
    schema = registry.load_credential_schema("airtable")
    assert schema is not None
    assert schema.provider == "airtable"
    assert schema.display_name == "Airtable"
    assert schema.auth_type == "oauth2"
    # Should have credentials + connection_config flattened into fields
    assert len(schema.fields) >= 2
    keys = [f.key for f in schema.fields]
    assert "api_token" in keys
    assert "base_id" in keys
    # Secret field should render as password
    api_token_field = next(f for f in schema.fields if f.key == "api_token")
    assert api_token_field.type == "password"


def test_load_credential_schema_oauth() -> None:
    """OAuth integration schema should include authorization URLs."""
    registry = IntegrationRegistry()
    schema = registry.load_credential_schema("google_calendar")
    assert schema is not None
    assert schema.auth_type == "oauth2"
    assert schema.authorization_url is not None
    assert schema.token_url is not None
    assert len(schema.default_scopes) > 0


def test_load_credential_schema_custom_webhook_field_order() -> None:
    """Header name fields should appear before API key/token inputs."""
    registry = IntegrationRegistry()
    schema = registry.load_credential_schema("custom_webhook")

    assert schema is not None
    assert [field.key for field in schema.fields[:2]] == ["header_name", "api_key"]


def test_build_credential_fields_prioritizes_header_name_fields() -> None:
    """Header-like connection fields should render before credential values."""
    registry = IntegrationRegistry()
    config = IntegrationConfig(
        display_name="Test",
        auth={"type": "api_key"},
        credentials={
            "api_key": FieldDef(
                title="API Key / Bearer Token",
                secret=True,
                required=False,
            )
        },
        connection_config={
            "header_name": FieldDef(
                title="Auth Header Name",
                required=False,
            ),
            "base_url": FieldDef(
                title="Base URL",
                required=True,
            ),
        },
    )

    fields = registry._build_credential_fields(config)

    assert [field.key for field in fields] == ["header_name", "api_key", "base_url"]


def test_list_integrations() -> None:
    """list_integrations should return all integrations from registry."""
    registry = IntegrationRegistry()
    integrations = registry.list_integrations()
    assert len(integrations) >= 5
    names = [i.name for i in integrations]
    assert "airtable" in names
    assert "google_calendar" in names
    assert "custom_webhook" in names
    # Should have categories as list
    gc = next(i for i in integrations if i.name == "google_calendar")
    assert isinstance(gc.categories, list)


def test_load_integration_config() -> None:
    """load_integration_config should return typed IntegrationConfig."""
    registry = IntegrationRegistry()
    config = registry.load_integration_config("google_calendar")
    assert config is not None
    assert config.auth.type == "oauth2"
    assert config.auth.authorization_url is not None
    assert config.auth.token_url is not None
    assert config.auth.authorization_params["access_type"] == "offline"
    assert config.auth.token_params["grant_type"] == "authorization_code"
    assert config.auth.refresh_params["grant_type"] == "refresh_token"
    assert "access_token" in config.credentials
    assert config.credentials["access_token"].secret is True
    assert "calendar_id" in config.connection_config
