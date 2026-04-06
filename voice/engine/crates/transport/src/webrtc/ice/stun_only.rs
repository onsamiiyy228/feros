//! Free STUN-only provider — no credentials, no TURN relay.
//!
//! Uses Cloudflare's public STUN server (`stun:stun.cloudflare.com:3478`).
//! This is the default when no TURN keys are configured.
//!
//! Sufficient for ~80% of NAT topologies. Only fails for symmetric NATs
//! or restrictive corporate firewalls that block all UDP.

use async_trait::async_trait;

use super::{IceProvider, IceProviderError, IceServer};

/// Free STUN-only provider — no configuration needed.
pub struct StunOnlyProvider;

#[async_trait]
impl IceProvider for StunOnlyProvider {
    async fn resolve(&self) -> Result<Vec<IceServer>, IceProviderError> {
        Ok(vec![IceServer {
            urls: vec!["stun:stun.cloudflare.com:3478".to_string()],
            username: None,
            credential: None,
        }])
    }
}
