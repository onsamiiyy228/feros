//! Integrations — credential management, OAuth, token refresh, tool bridges.
//!
//! This crate provides:
//! - **Encryption**: AES-256-GCM encryption engine for secrets at rest
//! - **Secret Store**: CRUD operations on encrypted credentials (via sqlx)
//! - **Secret Resolver**: Load and decrypt all credentials for a voice session
//! - **Integration Registry**: Parse `integrations.yaml` for auth config and credential schemas
//! - **Token Refresh**: Background service to refresh expiring OAuth tokens
//! - **Vault Server**: Lightweight Vault KV v2-compatible HTTP server for secret access

pub mod encryption;
pub mod registry;
pub mod secret_resolver;
pub mod secret_store;
pub mod token_refresh;
pub mod vault;

// Re-exports
pub use encryption::EncryptionEngine;
pub use registry::ProviderRegistry;
pub use secret_resolver::resolve_secrets;
pub use secret_store::SecretStore;
pub use token_refresh::TokenRefresher;
pub use vault::{
    start_vault_server, start_vault_server_with_provider, SecretProvider, VaultHandle,
};

#[cfg(feature = "extension-module")]
pub mod python;
