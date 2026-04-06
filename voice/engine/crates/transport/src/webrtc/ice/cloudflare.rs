//! Cloudflare managed TURN — ephemeral credentials via REST API.
//!
//! # Setup
//!
//! 1. Create a TURN key in the [Cloudflare Dashboard](https://dash.cloudflare.com)
//!    or via `POST /accounts/{account_id}/calls/turn_keys`.
//! 2. Set `CF_TURN_KEY_ID` and `CF_TURN_KEY_API_TOKEN` environment variables.
//!
//! Each call to [`resolve`](super::IceProvider::resolve) hits the Cloudflare
//! REST API to mint short-lived credentials with the configured TTL.
//!
//! # Pricing (as of 2026)
//!
//! - **STUN**: free, unlimited (`stun:stun.cloudflare.com:3478`)
//! - **TURN**: $0.05/GB, 1 TB/month free tier
//!
//! # API reference
//!
//! <https://developers.cloudflare.com/calls/turn/generate-credentials/>

use async_trait::async_trait;
use serde::Deserialize;

use super::{IceProvider, IceProviderError, IceServer};

/// Cloudflare managed TURN provider.
///
/// Generates ephemeral TURN credentials by calling:
/// ```text
/// POST https://rtc.live.cloudflare.com/v1/turn/keys/{key_id}/credentials/generate-ice-servers
/// Authorization: Bearer {api_token}
/// {"ttl": <seconds>}
/// ```
pub struct CloudflareIceProvider {
    turn_key_id: String,
    turn_key_api_token: String,
    ttl_secs: u64,
    http_client: reqwest::Client,
}

impl CloudflareIceProvider {
    /// Create a new Cloudflare ICE provider.
    ///
    /// # Arguments
    ///
    /// - `turn_key_id` — TURN Key ID from Cloudflare dashboard
    /// - `turn_key_api_token` — API token for that key
    /// - `ttl_secs` — credential lifetime (86400 = 24h recommended)
    pub fn new(turn_key_id: String, turn_key_api_token: String, ttl_secs: u64) -> Self {
        Self {
            turn_key_id,
            turn_key_api_token,
            ttl_secs,
            http_client: reqwest::Client::new(),
        }
    }
}

// ── Cloudflare API response types ────────────────────────────────

#[derive(Debug, Deserialize)]
struct CloudflareIceResponse {
    #[serde(rename = "iceServers")]
    ice_servers: Vec<CloudflareIceEntry>,
}

#[derive(Debug, Deserialize)]
struct CloudflareIceEntry {
    urls: Vec<String>,
    username: Option<String>,
    credential: Option<String>,
}

#[async_trait]
impl IceProvider for CloudflareIceProvider {
    async fn resolve(&self) -> Result<Vec<IceServer>, IceProviderError> {
        let url = format!(
            "https://rtc.live.cloudflare.com/v1/turn/keys/{}/credentials/generate-ice-servers",
            self.turn_key_id,
        );

        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(&self.turn_key_api_token)
            .json(&serde_json::json!({"ttl": self.ttl_secs}))
            .send()
            .await
            .map_err(|e| IceProviderError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(IceProviderError::HttpError(format!(
                "Cloudflare API returned {}: {}",
                status, body
            )));
        }

        let cf_resp: CloudflareIceResponse = resp
            .json()
            .await
            .map_err(|e| IceProviderError::InvalidResponse(e.to_string()))?;

        // Convert Cloudflare response → IceServer entries.
        // Filter out port 53 — blocked by Chrome, Firefox, and most ISPs.
        let servers = cf_resp
            .ice_servers
            .into_iter()
            .map(|entry| IceServer {
                urls: entry.urls.into_iter().filter(|u| !is_port_53(u)).collect(),
                username: entry.username,
                credential: entry.credential,
            })
            .filter(|s| !s.urls.is_empty())
            .collect();

        Ok(servers)
    }
}

/// Check if a TURN/STUN URL uses port 53 specifically.
///
/// Port 53 (DNS) is blocked by Chrome, Firefox, and most ISPs for
/// non-DNS traffic. Cloudflare sometimes returns `:53` TURN URLs.
///
/// Must not match `:5349` (standard TURNS/TLS port), `:530`, etc.
fn is_port_53(url: &str) -> bool {
    // TURN URLs look like: "turn:host:53?transport=udp" or "turn:host:53"
    url.ends_with(":53") || url.contains(":53?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_53_exact() {
        assert!(is_port_53("turn:turn.cloudflare.com:53"));
    }

    #[test]
    fn port_53_with_transport() {
        assert!(is_port_53("turn:turn.cloudflare.com:53?transport=udp"));
    }

    #[test]
    fn port_5349_not_matched() {
        assert!(!is_port_53("turns:turn.cloudflare.com:5349"));
    }

    #[test]
    fn port_5349_with_transport_not_matched() {
        assert!(!is_port_53("turns:turn.cloudflare.com:5349?transport=tcp"));
    }

    #[test]
    fn port_530_not_matched() {
        assert!(!is_port_53("turn:turn.example.com:530"));
    }

    #[test]
    fn port_3478_not_matched() {
        assert!(!is_port_53("stun:stun.cloudflare.com:3478"));
    }

    #[test]
    fn stun_url_not_matched() {
        assert!(!is_port_53("stun:stun.cloudflare.com:3478"));
    }
}
