//! Integration test: mini vault server ↔ vaultrs client.
//!
//! Verifies that our Vault KV v2 HTTP server produces responses that
//! the standard `vaultrs` client can parse correctly.
//!
//! **No database required** — uses an in-memory `MockSecretProvider`.
//!
//! ```sh
//! cargo test -p integrations --test vault_integration
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use integrations::{SecretProvider, VaultHandle};
use uuid::Uuid;
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};

// ── Mock Secret Provider ────────────────────────────────────────

/// In-memory secret provider for testing.
///
/// Pre-loaded with a map of `agent_id → secrets`.
/// No database, no encryption — just returns whatever is stored.
struct MockSecretProvider {
    secrets: HashMap<Uuid, HashMap<String, String>>,
}

impl MockSecretProvider {
    fn new() -> Self {
        Self {
            secrets: HashMap::new(),
        }
    }

    fn with_agent(mut self, agent_id: Uuid, secrets: HashMap<String, String>) -> Self {
        self.secrets.insert(agent_id, secrets);
        self
    }
}

#[async_trait]
impl SecretProvider for MockSecretProvider {
    async fn resolve(&self, agent_id: Uuid) -> Result<HashMap<String, String>, String> {
        Ok(self.secrets.get(&agent_id).cloned().unwrap_or_default())
    }
}

impl MockSecretProvider {
    /// Convert this provider into a type-erased Arc.
    /// Uses AST branching to bypass rust-analyzer's E0308 trait-resolution bug.
    #[allow(unexpected_cfgs)]
    pub fn into_dyn_arc(self) -> Arc<dyn SecretProvider> {
        #[cfg(not(rust_analyzer))]
        return Arc::new(self);

        #[cfg(rust_analyzer)]
        return unsafe { std::mem::transmute(Arc::new(self)) };
    }
}

// ── Helpers ─────────────────────────────────────────────────────

/// Start the vault with a mock provider and return `(handle, client)`.
///
/// Writes the ephemeral self-signed cert to a temp file so `vaultrs` can
/// verify the TLS connection (same mechanism as `VAULT_CACERT`).
fn start_mock_vault(provider: MockSecretProvider) -> (VaultHandle, VaultClient) {
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    // Write the cert PEM to a temp file for vaultrs
    let cert_path = write_cert_to_tempfile(&handle.cert_pem);

    let client = VaultClient::new(
        VaultClientSettingsBuilder::default()
            .address(&handle.url())
            .token(&handle.token)
            .ca_certs(vec![cert_path])
            .build()
            .expect("Failed to build vault client settings"),
    )
    .expect("Failed to create vault client");

    (handle, client)
}

/// Write PEM cert content to a temporary file, return the path as a String.
fn write_cert_to_tempfile(pem: &str) -> String {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    file.write_all(pem.as_bytes())
        .expect("Failed to write cert");
    let path = file.into_temp_path();
    let path_str = path.to_string_lossy().to_string();
    // Keep the file alive by leaking the path (test only)
    std::mem::forget(path);
    path_str
}

/// Create a VaultClient that trusts the handle's self-signed cert.
fn make_vault_client(handle: &VaultHandle, token: &str) -> VaultClient {
    let cert_path = write_cert_to_tempfile(&handle.cert_pem);
    VaultClient::new(
        VaultClientSettingsBuilder::default()
            .address(&handle.url())
            .token(token)
            .ca_certs(vec![cert_path])
            .build()
            .expect("Failed to build vault client settings"),
    )
    .expect("Failed to create vault client")
}

/// Create a reqwest::Client that trusts the handle's self-signed cert.
fn make_https_client(handle: &VaultHandle) -> reqwest::Client {
    let cert = reqwest::tls::Certificate::from_pem(handle.cert_pem.as_bytes())
        .expect("Failed to parse cert PEM");
    reqwest::Client::builder()
        .add_root_certificate(cert)
        .build()
        .expect("Failed to build HTTPS client")
}

// ── Tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn kv2_read_returns_expected_secrets() {
    let agent_id = Uuid::new_v4();
    let mut secrets = HashMap::new();
    secrets.insert("github".to_string(), "gh-token-12345".to_string());
    secrets.insert(
        "github.access_token".to_string(),
        "gh-token-12345".to_string(),
    );
    secrets.insert(
        "slack.bot_token".to_string(),
        "xoxb-slack-token".to_string(),
    );

    let provider = MockSecretProvider::new().with_agent(agent_id, secrets.clone());
    let (handle, client) = start_mock_vault(provider);

    // Read via vaultrs — the same code path voice-rust uses
    let data: HashMap<String, String> =
        vaultrs::kv2::read(&client, "secret", &agent_id.to_string())
            .await
            .expect("Failed to read from vault");

    assert_eq!(
        data.get("github").map(|s| s.as_str()),
        Some("gh-token-12345")
    );
    assert_eq!(
        data.get("github.access_token").map(|s| s.as_str()),
        Some("gh-token-12345")
    );
    assert_eq!(
        data.get("slack.bot_token").map(|s| s.as_str()),
        Some("xoxb-slack-token")
    );
    assert_eq!(data.len(), 3);

    handle.shutdown();
}

#[tokio::test]
async fn kv2_read_returns_empty_for_unknown_agent() {
    let provider = MockSecretProvider::new(); // no agents
    let (handle, client) = start_mock_vault(provider);

    let data: HashMap<String, String> =
        vaultrs::kv2::read(&client, "secret", &Uuid::new_v4().to_string())
            .await
            .expect("Expected empty response, not an error");

    assert!(data.is_empty(), "Expected empty map, got: {:?}", data);

    handle.shutdown();
}

#[tokio::test]
async fn kv2_read_rejects_invalid_token() {
    let provider = MockSecretProvider::new();
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    // Create client with a WRONG token but correct TLS trust
    let client = make_vault_client(&handle, "hvs.this-is-a-bad-token");

    let result: Result<HashMap<String, String>, _> =
        vaultrs::kv2::read(&client, "secret", &Uuid::new_v4().to_string()).await;

    assert!(result.is_err(), "Expected auth failure, got: {:?}", result);

    handle.shutdown();
}

#[tokio::test]
async fn kv2_read_rejects_missing_token() {
    let provider = MockSecretProvider::new();
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    // Hit the endpoint directly without any token header (using TLS-aware client)
    let http_client = make_https_client(&handle);
    let resp = http_client
        .get(format!(
            "{}/v1/secret/data/{}",
            handle.url(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 403);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["errors"][0]
        .as_str()
        .unwrap()
        .contains("missing client token"));

    handle.shutdown();
}

#[tokio::test]
async fn kv2_read_rejects_invalid_uuid() {
    let provider = MockSecretProvider::new();
    let (handle, client) = start_mock_vault(provider);

    let result: Result<HashMap<String, String>, _> =
        vaultrs::kv2::read(&client, "secret", "not-a-uuid").await;

    assert!(result.is_err(), "Expected error for invalid UUID path");

    handle.shutdown();
}

#[tokio::test]
async fn health_endpoint_responds() {
    let provider = MockSecretProvider::new();
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    let http_client = make_https_client(&handle);
    let resp = http_client
        .get(format!("{}/v1/sys/health", handle.url()))
        .send()
        .await
        .expect("Health request failed");

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["initialized"], true);
    assert_eq!(body["sealed"], false);
    assert!(body["server_time_utc"].is_number());

    handle.shutdown();
}

#[tokio::test]
async fn vault_handle_url_contains_correct_port() {
    let provider = MockSecretProvider::new();
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    let url = handle.url();
    assert!(url.starts_with("https://127.0.0.1:"));

    // Port should be non-zero (OS assigned)
    let port: u16 = url.rsplit(':').next().unwrap().parse().unwrap();
    assert!(port > 0, "Expected non-zero port, got: {}", port);

    handle.shutdown();
}

// ── Scoped Token Tests ──────────────────────────────────────────

#[tokio::test]
async fn scoped_token_reads_own_agent() {
    let agent_id = Uuid::new_v4();
    let mut secrets = HashMap::new();
    secrets.insert("api_key".to_string(), "secret-123".to_string());

    let provider = MockSecretProvider::new().with_agent(agent_id, secrets);
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    // Create a scoped token for this specific agent
    let scoped_token = handle.create_scoped_token(agent_id, std::time::Duration::from_secs(60));
    assert!(
        scoped_token.starts_with("hvs.s."),
        "Scoped token should have hvs.s. prefix"
    );

    let client = make_vault_client(&handle, &scoped_token);

    let data: HashMap<String, String> =
        vaultrs::kv2::read(&client, "secret", &agent_id.to_string())
            .await
            .expect("Scoped token should be able to read its own agent's secrets");

    assert_eq!(data.get("api_key").map(|s| s.as_str()), Some("secret-123"));

    handle.shutdown();
}

#[tokio::test]
async fn scoped_token_rejected_for_other_agent() {
    let agent_a = Uuid::new_v4();
    let agent_b = Uuid::new_v4();
    let mut secrets = HashMap::new();
    secrets.insert("api_key".to_string(), "secret-a".to_string());

    let provider = MockSecretProvider::new()
        .with_agent(agent_a, secrets.clone())
        .with_agent(agent_b, secrets);
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    // Create a scoped token for agent_a
    let scoped_token = handle.create_scoped_token(agent_a, std::time::Duration::from_secs(60));

    let client = make_vault_client(&handle, &scoped_token);

    // Try to read agent_b's secrets with agent_a's scoped token
    let result: Result<HashMap<String, String>, _> =
        vaultrs::kv2::read(&client, "secret", &agent_b.to_string()).await;

    assert!(
        result.is_err(),
        "Scoped token for agent_a should NOT be able to read agent_b's secrets"
    );

    handle.shutdown();
}

#[tokio::test]
async fn expired_scoped_token_rejected() {
    let agent_id = Uuid::new_v4();
    let mut secrets = HashMap::new();
    secrets.insert("api_key".to_string(), "secret-123".to_string());

    let provider = MockSecretProvider::new().with_agent(agent_id, secrets);
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    // Create a scoped token that expires immediately (0-second TTL)
    let scoped_token = handle.create_scoped_token(agent_id, std::time::Duration::from_secs(0));

    // Sleep a moment to ensure expiry
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = make_vault_client(&handle, &scoped_token);

    let result: Result<HashMap<String, String>, _> =
        vaultrs::kv2::read(&client, "secret", &agent_id.to_string()).await;

    assert!(result.is_err(), "Expired scoped token should be rejected");

    handle.shutdown();
}

#[tokio::test]
async fn root_token_still_reads_any_agent() {
    let agent_a = Uuid::new_v4();
    let agent_b = Uuid::new_v4();

    let provider = MockSecretProvider::new()
        .with_agent(
            agent_a,
            [("key_a".to_string(), "val_a".to_string())]
                .into_iter()
                .collect(),
        )
        .with_agent(
            agent_b,
            [("key_b".to_string(), "val_b".to_string())]
                .into_iter()
                .collect(),
        );

    let (handle, client) = start_mock_vault(provider);

    // Root token should read both agents
    let data_a: HashMap<String, String> =
        vaultrs::kv2::read(&client, "secret", &agent_a.to_string())
            .await
            .expect("Root token should read agent_a");
    assert_eq!(data_a.get("key_a").map(|s| s.as_str()), Some("val_a"));

    let data_b: HashMap<String, String> =
        vaultrs::kv2::read(&client, "secret", &agent_b.to_string())
            .await
            .expect("Root token should read agent_b");
    assert_eq!(data_b.get("key_b").map(|s| s.as_str()), Some("val_b"));

    handle.shutdown();
}

#[tokio::test]
async fn rate_limit_returns_429() {
    let provider = MockSecretProvider::new();
    let handle = integrations::start_vault_server_with_provider(provider.into_dyn_arc(), 0)
        .expect("Failed to start vault server");

    let http_client = make_https_client(&handle);
    let url = format!("{}/v1/secret/data/{}", handle.url(), Uuid::new_v4());

    // Send 100 rapid requests — some should be rate-limited.
    // The rate limiter allows a burst of 50 per second.
    let mut got_429 = false;
    for _ in 0..100 {
        let resp = http_client
            .get(&url)
            .header("X-Vault-Token", &handle.token)
            .send()
            .await
            .expect("Request failed");

        if resp.status() == 429 {
            got_429 = true;
            break;
        }
    }

    assert!(
        got_429,
        "Expected at least one 429 response from rate limiting"
    );

    handle.shutdown();
}
