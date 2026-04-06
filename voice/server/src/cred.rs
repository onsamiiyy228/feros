//! Credential management shim — re-exports real types when the `integrations`
//! feature is enabled, or provides no-op stubs when it is not.
//!
//! All voice-server code imports from here (`crate::cred::*`) so that the
//! integrations-free build still compiles.

// ── With `integrations` feature ────────────────────────────────────────────────

#[cfg(feature = "integrations")]
pub use integrations::encryption::EncryptionEngine;

#[cfg(feature = "integrations")]
pub use integrations::vault::{start_vault_server, VaultHandle};

// ── Without `integrations` feature: no-op stubs ───────────────────────────

/// No-op encryption engine — decryption always fails, falling back to
/// plaintext credential fields in the DB.
#[cfg(not(feature = "integrations"))]
pub struct EncryptionEngine;

#[cfg(not(feature = "integrations"))]
impl EncryptionEngine {
    pub fn new(_key: &[u8; 32]) -> Self {
        Self
    }

    pub fn from_base64(_key_b64: &str) -> Result<Self, String> {
        Ok(Self)
    }

    pub fn decrypt(&self, _ct: &str, _nonce: &str) -> Result<Vec<u8>, String> {
        Err("integrations feature not enabled".into())
    }
}

/// No-op vault handle — creates empty tokens, no server is started.
#[cfg(not(feature = "integrations"))]
pub struct VaultHandle;

#[cfg(not(feature = "integrations"))]
impl VaultHandle {
    pub fn create_scoped_token(&self, _agent_id: uuid::Uuid, _ttl: std::time::Duration) -> String {
        String::new()
    }

    pub fn url(&self) -> String {
        String::new()
    }

    pub fn cert_pem(&self) -> &str {
        ""
    }
}
