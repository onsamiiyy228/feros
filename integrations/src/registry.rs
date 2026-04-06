//! Integration registry — parse `integrations.yaml` into typed config.
//!
//! The registry is the single source of truth for all provider configurations.
//! Definitions and credential schemas for external services.
//!
//! Consumed by:
//! - **OAuth engine**: authorize URLs, token URLs, scopes, PKCE, params
//! - **Token refresher**: refresh params, margin, TTL
//! - **Frontend credential forms**: field definitions via Python admin API
//! - **Api/Bridge**: base_url, header templates with `${credentials.var}` interpolation
//! - **Builder Agent**: two-level loading (summary + on-demand detail)

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RegistryError {
    #[error("failed to read providers file: {0}")]
    IoError(#[from] std::io::Error),
    #[error("failed to parse providers YAML: {0}")]
    ParseError(#[from] serde_yaml::Error),
    #[error("provider not found: {0}")]
    NotFound(String),
}

/// Top-level registry — maps integration slug → config.
#[derive(Debug, Deserialize, Clone)]
pub struct ProviderRegistry {
    pub integrations: HashMap<String, ProviderConfig>,
}

/// Full configuration for a single provider.
#[derive(Debug, Deserialize, Clone)]
pub struct ProviderConfig {
    pub display_name: String,
    pub description: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub auth: AuthConfig,
    /// Credential fields — secrets the user provides (API keys, tokens).
    /// Keys are field names, values are field definitions with `secret: true`.
    #[serde(default)]
    pub credentials: HashMap<String, CredentialField>,
    /// Connection config — non-secret, user-provided runtime config
    /// (e.g. domain, subdomain, spreadsheet_id).
    #[serde(default)]
    pub connection_config: HashMap<String, ConnectionConfigField>,
    /// Api configuration for the HTTP bridge.
    #[serde(default)]
    pub api: Option<ApiConfig>,
}

/// Authentication configuration for a provider.
#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    #[serde(rename = "type")]
    pub auth_type: String,
    #[serde(default = "default_client_auth_method")]
    pub client_auth_method: String,
    #[serde(default)]
    pub authorization_url: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    /// Extra params appended to the authorization URL.
    /// e.g. `{ "access_type": "offline", "prompt": "consent" }`
    #[serde(default)]
    pub authorization_params: HashMap<String, String>,
    /// Params sent in the token exchange request.
    /// e.g. `{ "grant_type": "authorization_code" }`
    #[serde(default)]
    pub token_params: HashMap<String, String>,
    /// Params sent in the refresh token request.
    /// e.g. `{ "grant_type": "refresh_token" }`
    #[serde(default)]
    pub refresh_params: HashMap<String, String>,
    /// Default scopes requested during OAuth authorization.
    #[serde(default)]
    pub default_scopes: Vec<String>,
    /// Scope separator character. Default: " " (space).
    /// Some APIs like Accelo use "," instead.
    #[serde(default = "default_scope_separator")]
    pub scope_separator: String,
    /// Whether to use PKCE. Default: true.
    /// Set to false for providers that don't support it.
    #[serde(default = "default_true")]
    pub pkce: bool,
    /// Expected token lifetime in seconds (for scheduling refresh).
    #[serde(default)]
    pub token_expires_in_seconds: Option<u64>,
    /// Minutes before expiry to trigger refresh. Default: 5.
    #[serde(default = "default_margin")]
    pub refresh_margin_minutes: u32,
}

/// A credential field definition — secret values the user provides.
#[derive(Debug, Deserialize, Clone)]
pub struct CredentialField {
    #[serde(rename = "type", default = "default_string_type")]
    pub field_type: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Whether this field contains a secret value (API key, token, etc.).
    #[serde(default)]
    pub secret: bool,
    /// Whether this field is required. Default: true.
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub doc_url: Option<String>,
}

/// Connection config field — non-secret, user-provided runtime config.
#[derive(Debug, Deserialize, Clone)]
pub struct ConnectionConfigField {
    #[serde(rename = "type", default = "default_string_type")]
    pub field_type: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub example: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
}

/// Api configuration for the HTTP bridge.
#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct ApiConfig {
    #[serde(default)]
    pub base_url: Option<String>,
    /// Header templates with `${credentials.var}` interpolation.
    /// e.g. `{ "authorization": "Bearer ${credentials.api_key}" }`
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn default_true() -> bool {
    true
}

fn default_client_auth_method() -> String {
    "body".to_string()
}

fn default_margin() -> u32 {
    5
}

fn default_scope_separator() -> String {
    " ".to_string()
}

fn default_string_type() -> String {
    "string".to_string()
}

impl ProviderRegistry {
    /// Load providers from a YAML file.
    pub fn load(path: &Path) -> Result<Self, RegistryError> {
        let content = std::fs::read_to_string(path)?;
        let registry: Self = serde_yaml::from_str(&content)?;
        Ok(registry)
    }

    /// Load providers from the `integrations.yaml` baked in at compile time.
    ///
    /// This is the preferred method for Rust consumers (voice-server, etc.)
    /// since it requires no file on disk at runtime.
    pub fn from_embedded() -> Result<Self, RegistryError> {
        Self::from_yaml(include_str!("../integrations.yaml"))
    }

    /// Parse providers from a YAML string (useful for tests / embedding).
    pub fn from_yaml(yaml: &str) -> Result<Self, RegistryError> {
        let registry: Self = serde_yaml::from_str(yaml)?;
        Ok(registry)
    }

    /// Look up a single provider by slug.
    pub fn get(&self, name: &str) -> Option<&ProviderConfig> {
        self.integrations.get(name)
    }

    /// List all provider slugs.
    pub fn provider_names(&self) -> Vec<&str> {
        self.integrations.keys().map(|k| k.as_str()).collect()
    }

    /// Generate the lightweight summary for Builder Agent injection (~600 tokens total).
    pub fn builder_summary(&self) -> Vec<BuilderSkillEntry> {
        self.integrations
            .iter()
            .map(|(name, p)| BuilderSkillEntry {
                name: name.clone(),
                display_name: p.display_name.clone(),
                description: p.description.clone(),
                auth_type: p.auth.auth_type.clone(),
                categories: p.categories.clone(),
            })
            .collect()
    }

    /// Get on-demand context for the Builder Agent when coding a specific integration.
    pub fn builder_provider_context(&self, name: &str) -> Option<BuilderProviderContext> {
        let p = self.integrations.get(name)?;
        Some(BuilderProviderContext {
            name: name.to_string(),
            display_name: p.display_name.clone(),
            auth_type: p.auth.auth_type.clone(),
            api: p.api.clone(),
            credential_keys: p
                .credentials
                .iter()
                .map(|(k, f)| (k.clone(), f.description.clone()))
                .collect(),
            connection_config_keys: p
                .connection_config
                .iter()
                .map(|(k, f)| (k.clone(), f.description.clone()))
                .collect(),
        })
    }

    /// Interpolate `${credentials.var}` and `${connectionConfig.var}` in a template string.
    pub fn interpolate_template(
        template: &str,
        credentials: &HashMap<String, String>,
        connection_config: &HashMap<String, String>,
    ) -> String {
        let mut result = template.to_string();
        for (key, value) in credentials {
            result = result.replace(&format!("${{credentials.{}}}", key), value);
        }
        for (key, value) in connection_config {
            result = result.replace(&format!("${{connectionConfig.{}}}", key), value);
        }
        result
    }
}

/// Lightweight summary entry for Builder Agent context.
#[derive(Debug, serde::Serialize)]
pub struct BuilderSkillEntry {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub auth_type: String,
    pub categories: Vec<String>,
}

/// On-demand provider context for Builder Agent.
#[derive(Debug, serde::Serialize)]
pub struct BuilderProviderContext {
    pub name: String,
    pub display_name: String,
    pub auth_type: String,
    pub api: Option<ApiConfig>,
    pub credential_keys: Vec<(String, String)>,
    pub connection_config_keys: Vec<(String, String)>,
}

// ── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
integrations:
  google_calendar:
    display_name: Google Calendar
    description: Create and manage calendar events
    categories:
      - scheduling
      - productivity
    auth:
      type: oauth2
      authorization_url: https://accounts.google.com/o/oauth2/v2/auth
      token_url: https://oauth2.googleapis.com/token
      authorization_params:
        response_type: code
        access_type: offline
        prompt: consent
      token_params:
        grant_type: authorization_code
      refresh_params:
        grant_type: refresh_token
      default_scopes:
        - https://www.googleapis.com/auth/calendar
      scope_separator: " "
      pkce: true
      token_expires_in_seconds: 3600
      refresh_margin_minutes: 5
    credentials:
      access_token:
        type: string
        title: Service Account Token
        description: Only for service accounts
        secret: true
        required: false
    connection_config:
      calendar_id:
        type: string
        title: Calendar ID
        description: Your calendar ID
        example: primary
        required: false
    api:
      base_url: https://www.googleapis.com/calendar/v3
      headers:
        authorization: "Bearer ${credentials.access_token}"

  custom_webhook:
    display_name: Custom Webhook
    description: Send data to any HTTP endpoint
    categories:
      - custom
    auth:
      type: api_key
      pkce: false
    credentials:
      api_key:
        type: string
        title: API Key
        secret: true
        required: false
    connection_config:
      header_name:
        type: string
        title: Header Name
        description: Auth header name
        example: Authorization
        required: false
"#;

    #[test]
    fn parse_providers_yaml() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();

        assert_eq!(registry.integrations.len(), 2);
        assert!(registry.integrations.contains_key("google_calendar"));
        assert!(registry.integrations.contains_key("custom_webhook"));
    }

    #[test]
    fn oauth_provider_config() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();
        let gc = registry.get("google_calendar").unwrap();

        assert_eq!(gc.display_name, "Google Calendar");
        assert_eq!(gc.auth.auth_type, "oauth2");
        assert!(gc.auth.pkce);
        assert_eq!(gc.auth.default_scopes.len(), 1);
        assert!(gc.auth.authorization_url.is_some());
        assert!(gc.auth.token_url.is_some());
        assert_eq!(gc.auth.scope_separator, " ");
        assert_eq!(gc.auth.refresh_margin_minutes, 5);
        assert_eq!(gc.auth.token_expires_in_seconds, Some(3600));

        // Authorization params (Nango-style)
        assert_eq!(
            gc.auth.authorization_params.get("access_type").unwrap(),
            "offline"
        );
        assert_eq!(
            gc.auth.authorization_params.get("prompt").unwrap(),
            "consent"
        );
        assert_eq!(
            gc.auth.token_params.get("grant_type").unwrap(),
            "authorization_code"
        );
        assert_eq!(
            gc.auth.refresh_params.get("grant_type").unwrap(),
            "refresh_token"
        );

        // Categories
        assert_eq!(gc.categories.len(), 2);
        assert!(gc.categories.contains(&"scheduling".to_string()));
    }

    #[test]
    fn credentials_and_connection_config() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();
        let gc = registry.get("google_calendar").unwrap();

        // Credentials (secrets)
        assert_eq!(gc.credentials.len(), 1);
        let token = gc.credentials.get("access_token").unwrap();
        assert!(token.secret);
        assert!(!token.required);

        // Connection config (non-secret)
        assert_eq!(gc.connection_config.len(), 1);
        let cal_id = gc.connection_config.get("calendar_id").unwrap();
        assert!(!cal_id.required);
        assert_eq!(cal_id.example.as_deref(), Some("primary"));
    }

    #[test]
    fn api_config_and_interpolation() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();
        let gc = registry.get("google_calendar").unwrap();

        let api = gc.api.as_ref().unwrap();
        assert_eq!(
            api.base_url.as_deref(),
            Some("https://www.googleapis.com/calendar/v3")
        );
        let auth_header = api.headers.get("authorization").unwrap();
        assert_eq!(auth_header, "Bearer ${credentials.access_token}");

        // Test interpolation
        let mut creds = HashMap::new();
        creds.insert("access_token".to_string(), "ya29.xxx".to_string());
        let result = ProviderRegistry::interpolate_template(auth_header, &creds, &HashMap::new());
        assert_eq!(result, "Bearer ya29.xxx");
    }

    #[test]
    fn api_key_provider_no_pkce() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();
        let wh = registry.get("custom_webhook").unwrap();

        assert_eq!(wh.auth.auth_type, "api_key");
        assert!(!wh.auth.pkce);
        assert_eq!(wh.credentials.len(), 1);
        assert!(wh.credentials.get("api_key").unwrap().secret);
        assert_eq!(wh.connection_config.len(), 1);
    }

    #[test]
    fn builder_summary() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();
        let summary = registry.builder_summary();

        assert_eq!(summary.len(), 2);
        for entry in &summary {
            assert!(!entry.name.is_empty());
            assert!(!entry.display_name.is_empty());
            assert!(!entry.auth_type.is_empty());
            assert!(!entry.categories.is_empty());
        }
    }

    #[test]
    fn builder_provider_context() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();

        let ctx = registry
            .builder_provider_context("google_calendar")
            .unwrap();
        assert_eq!(ctx.auth_type, "oauth2");
        assert_eq!(ctx.credential_keys.len(), 1);
        assert_eq!(ctx.connection_config_keys.len(), 1);

        assert!(registry.builder_provider_context("nonexistent").is_none());
    }

    #[test]
    fn provider_names() {
        let registry = ProviderRegistry::from_yaml(SAMPLE_YAML).unwrap();
        let names = registry.provider_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"google_calendar"));
        assert!(names.contains(&"custom_webhook"));
    }
}
