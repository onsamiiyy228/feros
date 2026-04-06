//! ICE provider configuration — typed config for STUN/TURN settings.
//!
//! This is a plain data struct. It derives `Deserialize` so callers
//! can populate it from any serde-compatible source (env vars via `envy`,
//! JSON, TOML, tests, etc.). The transport crate never reads env vars
//! itself — that's the binary's job.

use serde::Deserialize;

/// Configuration for the ICE provider system.
///
/// Determines which STUN/TURN provider to use and how to connect to it.
///
/// # Environment Variables (when parsed via `envy`)
///
/// | Variable | Default | Description |
/// |----------|---------|-------------|
/// | `CF__TURN_KEY_ID` | *(empty)* | Cloudflare TURN Key ID → enables Cloudflare provider |
/// | `CF__TURN_KEY_API_TOKEN` | *(empty)* | Cloudflare TURN API Token |
/// | `CF__TURN_TTL` | `86400` | Credential TTL in seconds |
/// | `TURN__URLS` | *(empty)* | Static TURN URLs (comma-separated) → enables static provider |
/// | `TURN__USERNAME` | *(empty)* | Static TURN username |
/// | `TURN__CREDENTIAL` | *(empty)* | Static TURN credential |
/// | `STUN__SERVER` | `stun.cloudflare.com:3478` | STUN server for server-side IP discovery |
///
/// **Note:** `Default::default()` gives zero/empty values (suitable for tests
/// where you set only the fields you care about). Production defaults
/// (`86400`, `"stun.cloudflare.com:3478"`) are applied by `serde(default)`
/// when deserializing from env vars via `envy`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct IceConfig {
    /// Cloudflare TURN Key ID (enables Cloudflare managed TURN).
    #[serde(rename = "cf__turn_key_id", default)]
    pub cf_turn_key_id: String,

    /// Cloudflare TURN API Token.
    #[serde(rename = "cf__turn_key_api_token", default)]
    pub cf_turn_key_api_token: String,

    /// Cloudflare TURN credential TTL in seconds (default: 86400 = 24h).
    #[serde(rename = "cf__turn_ttl", default = "default_turn_ttl")]
    pub cf_turn_ttl: u64,

    /// Static TURN server URLs (comma-separated, for coturn/Twilio/Xirsys).
    #[serde(rename = "turn__urls", default)]
    pub turn_urls: String,

    /// Static TURN username.
    #[serde(rename = "turn__username", default)]
    pub turn_username: String,

    /// Static TURN credential.
    #[serde(rename = "turn__credential", default)]
    pub turn_credential: String,

    /// STUN server for server-side IP discovery in `connection.rs`.
    #[serde(rename = "stun__server", default = "default_stun_server")]
    pub stun_server: String,
}

fn default_turn_ttl() -> u64 {
    86400
}

fn default_stun_server() -> String {
    "stun.cloudflare.com:3478".to_string()
}
