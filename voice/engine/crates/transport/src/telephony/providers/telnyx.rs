//! Telnyx telephony provider — WebSocket media protocol + REST call control.
//!
//! Protocol reference: https://developers.telnyx.com/docs/voice/media-streaming

use async_trait::async_trait;
use tracing::{error, info, warn};

use crate::error::TransportError;

use super::TelephonyProviderImpl;

/// Telnyx Media Streams provider.
pub struct Telnyx;

#[async_trait]
impl TelephonyProviderImpl for Telnyx {
    fn name(&self) -> &'static str {
        "telnyx"
    }

    fn extract_stream_id(&self, start_json: &serde_json::Value) -> Option<String> {
        start_json
            .get("stream_id")
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn extract_call_id(&self, start_json: &serde_json::Value) -> Option<String> {
        start_json
            .get("start")
            .and_then(|s| s.get("call_control_id"))
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn extract_custom_param(&self, start_json: &serde_json::Value, name: &str) -> Option<String> {
        start_json
            .get("start")
            .and_then(|s| s.get("custom_parameters"))
            .and_then(|p| p.get(name))
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn media_frame(&self, payload_b64: &str, stream_id: &str) -> serde_json::Value {
        serde_json::json!({
            "event": "media",
            "stream_id": stream_id,
            "media": { "payload": payload_b64 }
        })
    }

    fn clear_frame(&self, stream_id: &str) -> serde_json::Value {
        serde_json::json!({
            "event": "clear",
            "stream_id": stream_id,
        })
    }

    async fn hangup(
        &self,
        config: &super::super::config::TelephonyConfig,
        call_id: &str,
    ) -> Result<(), TransportError> {
        let api_key = match &config.credentials {
            super::super::config::TelephonyCredentials::Telnyx { api_key } => api_key.as_str(),
            _ => return Err(TransportError::SendFailed("Invalid credentials for Telnyx provider".into())),
        };

        let endpoint = format!("https://api.telnyx.com/v2/calls/{}/actions/hangup", call_id);

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await
            .map_err(|e| {
                TransportError::SendFailed(format!("Telnyx hangup request failed: {}", e))
            })?;

        match resp.status().as_u16() {
            200 => {
                info!("[telnyx] Successfully terminated call {}", call_id);
                Ok(())
            }
            422 => {
                warn!("[telnyx] Call {} already terminated (422)", call_id);
                Ok(())
            }
            status => {
                let body = resp.text().await.unwrap_or_default();
                error!(
                    "[telnyx] Failed to terminate call {}: status={}, body={}",
                    call_id, status, body
                );
                Err(TransportError::SendFailed(format!(
                    "Telnyx hangup failed: status={}",
                    status
                )))
            }
        }
    }
}
