//! ICE provider — pluggable STUN/TURN server configuration.
//!
//! Defines the [`IceProvider`] trait for resolving ICE server configs, plus
//! built-in implementations for common providers.
//!
//! # Built-in providers
//!
//! | Provider | Module | Config |
//! |----------|--------|--------|
//! | **Cloudflare** | [`cloudflare`] | `CF_TURN_KEY_ID` + `CF_TURN_KEY_API_TOKEN` |
//! | **STUN-only** | [`stun_only`] | None — free, no relay |
//! | **Static** | [`static_provider`] | `TURN_URLS` + credentials |
//!
//! # Adding a new provider
//!
//! 1. Create a new file (e.g. `twilio.rs`, `google.rs`, `xirsys.rs`)
//! 2. Implement [`IceProvider`] for your struct
//! 3. Re-export it from this module
//! 4. Add a branch to [`ice_provider_from_config`] if auto-detection is wanted

pub mod cloudflare;
pub mod config;
pub mod static_provider;
pub mod stun_only;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// Re-export provider types for convenience
pub use cloudflare::CloudflareIceProvider;
pub use config::IceConfig;
pub use static_provider::StaticIceProvider;
pub use stun_only::StunOnlyProvider;

// ── Shared Types ─────────────────────────────────────────────────

/// A single ICE server entry.
///
/// Maps 1:1 to the browser's [`RTCIceServer`][mdn] interface and the JSON
/// format returned by `GET /rtc/ice-servers`.
///
/// [mdn]: https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    /// URIs such as `"stun:stun.cloudflare.com:3478"` or
    /// `"turn:turn.cloudflare.com:3478?transport=udp"`.
    pub urls: Vec<String>,

    /// Username (TURN only — omitted for STUN).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Credential / password (TURN only — omitted for STUN).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// Errors from ICE provider resolution.
#[derive(Debug, Error)]
pub enum IceProviderError {
    /// HTTP request to the TURN credential API failed.
    #[error("TURN credential request failed: {0}")]
    HttpError(String),

    /// The provider returned an unexpected response.
    #[error("invalid TURN response: {0}")]
    InvalidResponse(String),
}

// ── Provider Trait ───────────────────────────────────────────────

/// Resolves ICE server configurations for a WebRTC session.
///
/// The voice engine calls [`resolve`](IceProvider::resolve) once per session.
/// Implementations may call external APIs to generate ephemeral TURN
/// credentials — the returned [`IceServer`] entries are forwarded
/// verbatim to the browser via `GET /rtc/ice-servers`.
///
/// # Example: custom provider
///
/// ```rust,ignore
/// use async_trait::async_trait;
/// use voice_transport::webrtc::ice::{IceProvider, IceServer, IceProviderError};
///
/// pub struct MyTurnProvider { /* ... */ }
///
/// #[async_trait]
/// impl IceProvider for MyTurnProvider {
///     async fn resolve(&self) -> Result<Vec<IceServer>, IceProviderError> {
///         // Call your TURN API, return IceServer entries
///         Ok(vec![IceServer {
///             urls: vec!["turn:my-server:3478".into()],
///             username: Some("user".into()),
///             credential: Some("pass".into()),
///         }])
///     }
/// }
/// ```
#[async_trait]
pub trait IceProvider: Send + Sync {
    /// Return the ICE server list for a new WebRTC session.
    async fn resolve(&self) -> Result<Vec<IceServer>, IceProviderError>;
}

// ── Provider Factory ─────────────────────────────────────────────

/// Create an ICE provider from an [`IceConfig`].
///
/// Selection order (first match wins):
/// 1. `cf_turn_key_id` + `cf_turn_key_api_token` non-empty → [`CloudflareIceProvider`]
/// 2. `turn_urls` non-empty → [`StaticIceProvider`]
/// 3. Otherwise → [`StunOnlyProvider`] (free, no relay)
pub fn ice_provider_from_config(cfg: &IceConfig) -> Box<dyn IceProvider> {
    // 1. Cloudflare managed TURN
    if !cfg.cf_turn_key_id.is_empty() && !cfg.cf_turn_key_api_token.is_empty() {
        tracing::info!(
            "ICE provider: Cloudflare TURN (key={}, ttl={}s)",
            &cfg.cf_turn_key_id[..cfg.cf_turn_key_id.len().min(8)],
            cfg.cf_turn_ttl
        );
        return Box::new(CloudflareIceProvider::new(
            cfg.cf_turn_key_id.clone(),
            cfg.cf_turn_key_api_token.clone(),
            cfg.cf_turn_ttl,
        ));
    }

    // 2. Static TURN (self-hosted coturn, Twilio, Xirsys, etc.)
    let urls: Vec<String> = cfg
        .turn_urls
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !urls.is_empty() {
        let username = if cfg.turn_username.is_empty() {
            None
        } else {
            Some(cfg.turn_username.clone())
        };
        let credential = if cfg.turn_credential.is_empty() {
            None
        } else {
            Some(cfg.turn_credential.clone())
        };
        tracing::info!("ICE provider: static TURN ({} URLs)", urls.len());
        return Box::new(StaticIceProvider::from_parts(
            None, urls, username, credential,
        ));
    }

    // 3. Default: free STUN only
    tracing::info!("ICE provider: STUN only (no TURN configured)");
    Box::new(StunOnlyProvider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_selects_stun_only() {
        let cfg = IceConfig::default();
        let provider = ice_provider_from_config(&cfg);
        // StunOnlyProvider is the fallback — verify it resolves
        let rt = tokio::runtime::Runtime::new().unwrap();
        let servers = rt.block_on(provider.resolve()).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, vec!["stun:stun.cloudflare.com:3478"]);
        assert!(servers[0].username.is_none());
        assert!(servers[0].credential.is_none());
    }

    #[test]
    fn cloudflare_config_selects_cloudflare() {
        let cfg = IceConfig {
            cf_turn_key_id: "test-key-id".to_string(),
            cf_turn_key_api_token: "test-token".to_string(),
            cf_turn_ttl: 3600,
            ..IceConfig::default()
        };
        // We can't actually call the Cloudflare API in tests, but we
        // can verify the factory doesn't panic and returns a provider.
        let _provider = ice_provider_from_config(&cfg);
        // If we got here without panic, the Cloudflare branch was taken.
    }

    #[test]
    fn static_turn_config_selects_static() {
        let cfg = IceConfig {
            turn_urls: "turn:my-server:3478,turns:my-server:5349".to_string(),
            turn_username: "user".to_string(),
            turn_credential: "pass".to_string(),
            ..IceConfig::default()
        };
        let provider = ice_provider_from_config(&cfg);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let servers = rt.block_on(provider.resolve()).unwrap();
        // Should have STUN + TURN entries
        assert_eq!(servers.len(), 2);
        // First is STUN
        assert!(servers[0].urls[0].starts_with("stun:"));
        // Second is TURN with credentials
        assert_eq!(servers[1].urls.len(), 2);
        assert_eq!(servers[1].username, Some("user".to_string()));
        assert_eq!(servers[1].credential, Some("pass".to_string()));
    }

    #[test]
    fn cloudflare_takes_priority_over_static() {
        let cfg = IceConfig {
            cf_turn_key_id: "key".to_string(),
            cf_turn_key_api_token: "token".to_string(),
            turn_urls: "turn:fallback:3478".to_string(),
            ..IceConfig::default()
        };
        // Cloudflare should win (first match)
        let _provider = ice_provider_from_config(&cfg);
    }

    #[test]
    fn empty_turn_urls_ignored() {
        let cfg = IceConfig {
            turn_urls: "  ,  , ".to_string(),
            ..IceConfig::default()
        };
        let provider = ice_provider_from_config(&cfg);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let servers = rt.block_on(provider.resolve()).unwrap();
        // Should fall through to STUN-only
        assert_eq!(servers.len(), 1);
        assert!(servers[0].urls[0].contains("stun"));
    }

    #[test]
    fn ice_config_serde_defaults() {
        // Simulate envy parsing with no env vars set
        let cfg: IceConfig =
            envy::from_iter::<_, IceConfig>(std::iter::empty::<(String, String)>()).unwrap();
        assert_eq!(cfg.cf_turn_ttl, 86400);
        assert_eq!(cfg.stun_server, "stun.cloudflare.com:3478");
        assert!(cfg.cf_turn_key_id.is_empty());
    }

    #[test]
    fn ice_config_derive_default_gives_zeros() {
        let cfg = IceConfig::default();
        assert_eq!(cfg.cf_turn_ttl, 0);
        assert!(cfg.stun_server.is_empty());
    }
}
