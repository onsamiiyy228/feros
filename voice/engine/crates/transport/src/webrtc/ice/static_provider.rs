//! Static ICE provider — fixed server list, no API calls.
//!
//! For users who self-host coturn, or use third-party TURN services
//! (Twilio, Xirsys, etc.) with long-lived credentials.
//!
//! # Environment variables
//!
//! ```text
//! TURN_URLS=turn:my-server:3478,turns:my-server:5349
//! TURN_USERNAME=myuser
//! TURN_CREDENTIAL=mypass
//! ```

use async_trait::async_trait;

use super::{IceProvider, IceProviderError, IceServer};

/// Static ICE provider with a fixed set of servers.
///
/// Credentials are assumed to be long-lived or externally managed.
pub struct StaticIceProvider {
    servers: Vec<IceServer>,
}

impl StaticIceProvider {
    /// Create a provider with a pre-built set of ICE servers.
    pub fn new(servers: Vec<IceServer>) -> Self {
        Self { servers }
    }

    /// Create from environment-style parameters.
    ///
    /// Produces one STUN entry (defaulting to Cloudflare) plus one TURN
    /// entry if `turn_urls` is non-empty.
    pub fn from_parts(
        stun_url: Option<String>,
        turn_urls: Vec<String>,
        username: Option<String>,
        credential: Option<String>,
    ) -> Self {
        let mut servers = Vec::new();

        // STUN entry (default to Cloudflare if not specified)
        let stun = stun_url.unwrap_or_else(|| "stun:stun.cloudflare.com:3478".to_string());
        servers.push(IceServer {
            urls: vec![stun],
            username: None,
            credential: None,
        });

        // TURN entry (only if URLs provided)
        if !turn_urls.is_empty() {
            servers.push(IceServer {
                urls: turn_urls,
                username,
                credential,
            });
        }

        Self { servers }
    }
}

#[async_trait]
impl IceProvider for StaticIceProvider {
    async fn resolve(&self) -> Result<Vec<IceServer>, IceProviderError> {
        Ok(self.servers.clone())
    }
}
