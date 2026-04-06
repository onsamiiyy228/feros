//! Session secret resolution — fetch + refresh credentials from the vault.
//!
//! Owns the lifecycle of per-session secrets:
//!   1. Initial resolution at session registration time
//!   2. Background refresh task that re-queries the vault every 90 seconds
//!
//! voice-engine receives a pre-populated `SharedSecretMap` and never touches
//! the vault directly. This keeps voice-engine free of infrastructure concerns.

use std::sync::Arc;

use agent_kit::agent_backends::{SecretMap, SharedSecretMap};
use tracing::warn;

use crate::vault_client;

/// Cached vault address from `VAULT_ADDR` environment variable.
///
/// Returns `None` if `VAULT_ADDR` is not set. The value is read once and
/// cached for the process lifetime.
fn vault_addr() -> Option<&'static str> {
    use std::sync::OnceLock;
    static VAULT_ADDR: OnceLock<Option<String>> = OnceLock::new();
    VAULT_ADDR
        .get_or_init(|| std::env::var("VAULT_ADDR").ok())
        .as_deref()
}

/// Resolve agent secrets from the vault server (if configured).
///
/// Requires a per-session scoped `vault_token`. If no token is provided,
/// returns an empty `SecretMap` — there is no fallback to a global token.
///
/// Returns a `SharedSecretMap` (`Arc<RwLock<SecretMap>>`) that can be
/// refreshed in the background while the session is running.
pub async fn resolve_vault_secrets(
    agent_id: &str,
    session_vault_token: Option<&str>,
) -> SharedSecretMap {
    if let (Some(addr), Some(token)) = (vault_addr(), session_vault_token) {
        if let Ok(agent_uuid) = uuid::Uuid::parse_str(agent_id) {
            match vault_client::resolve_secrets(addr, token, agent_uuid).await {
                Ok(secrets) => return Arc::new(std::sync::RwLock::new(secrets)),
                Err(e) => {
                    warn!(
                        agent_id = %agent_id,
                        error = %e,
                        "Failed to resolve vault secrets — session will run without agent credentials"
                    );
                }
            }
        }
    }

    Arc::new(std::sync::RwLock::new(SecretMap::new()))
}

/// Refresh interval for re-querying vault secrets (90 seconds).
///
/// This is more frequent than the `TokenRefresher`'s 60-second cron, ensuring
/// the session picks up refreshed tokens within ~90 seconds after they're
/// written to the DB. Short enough to catch most refreshes, long enough to
/// avoid hammering the vault server.
const SECRET_REFRESH_INTERVAL_SECS: u64 = 90;

/// Spawn a background task that periodically re-fetches secrets from the vault
/// and updates the shared secret map.
///
/// Returns a `JoinHandle` — the caller should abort it when the session ends.
/// If `vault_token` is `None`, no task is spawned (returns `None`).
pub fn spawn_secret_refresh_task(
    agent_id: String,
    vault_token: Option<String>,
    secrets: SharedSecretMap,
) -> Option<tokio::task::JoinHandle<()>> {
    let addr = vault_addr()?.to_string();
    let token = vault_token?;

    let Ok(agent_uuid) = uuid::Uuid::parse_str(&agent_id) else {
        return None;
    };

    Some(tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(SECRET_REFRESH_INTERVAL_SECS));
        interval.tick().await; // skip the first immediate tick

        loop {
            interval.tick().await;

            match vault_client::resolve_secrets(&addr, &token, agent_uuid).await {
                Ok(new_secrets) => {
                    // Don't erase previously loaded credentials when vault
                    // returns an empty map (e.g. 404 — no secrets for this agent).
                    if new_secrets.is_empty() {
                        continue;
                    }
                    match secrets.write() {
                        Ok(mut map) => {
                            *map = new_secrets;
                            tracing::debug!(
                                agent_id = %agent_id,
                                "Refreshed session secrets from vault"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                agent_id = %agent_id,
                                error = %e,
                                "Failed to write refreshed secrets — lock poisoned"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = %agent_id,
                        error = %e,
                        "Failed to refresh vault secrets — will retry next interval"
                    );
                }
            }
        }
    }))
}
