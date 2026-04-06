//! Twilio telephony provider — WebSocket Media Streams protocol + REST call control.
//!
//! Protocol reference: https://www.twilio.com/docs/voice/media-streams

use async_trait::async_trait;
use tracing::{error, info, warn};

use crate::error::TransportError;

use super::TelephonyProviderImpl;

/// Twilio Media Streams provider.
pub struct Twilio;

#[async_trait]
impl TelephonyProviderImpl for Twilio {
    fn name(&self) -> &'static str {
        "twilio"
    }

    fn extract_stream_id(&self, start_json: &serde_json::Value) -> Option<String> {
        start_json
            .get("start")
            .and_then(|s| s.get("streamSid"))
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn extract_call_id(&self, start_json: &serde_json::Value) -> Option<String> {
        start_json
            .get("start")
            .and_then(|s| s.get("callSid"))
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn media_frame(&self, payload_b64: &str, stream_id: &str) -> serde_json::Value {
        serde_json::json!({
            "event": "media",
            "streamSid": stream_id,
            "media": { "payload": payload_b64 }
        })
    }

    fn clear_frame(&self, stream_id: &str) -> serde_json::Value {
        serde_json::json!({
            "event": "clear",
            "streamSid": stream_id,
        })
    }

    async fn hangup(
        &self,
        config: &super::super::config::TelephonyConfig,
        call_id: &str,
    ) -> Result<(), TransportError> {
        let (account_sid, auth_token) = match &config.credentials {
            super::super::config::TelephonyCredentials::Twilio {
                account_sid,
                auth_token,
            } => (account_sid.as_str(), auth_token.as_str()),
            _ => return Err(TransportError::SendFailed("Invalid credentials for Twilio provider".into())),
        };

        let endpoint = format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Calls/{}.json",
            account_sid, call_id
        );

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .basic_auth(account_sid, Some(auth_token))
            .form(&[("Status", "completed")])
            .send()
            .await
            .map_err(|e| {
                TransportError::SendFailed(format!("Twilio hangup request failed: {}", e))
            })?;

        match resp.status().as_u16() {
            200 => {
                info!("[twilio] Successfully terminated call {}", call_id);
                Ok(())
            }
            404 => {
                warn!("[twilio] Call {} already terminated (404)", call_id);
                Ok(())
            }
            status => {
                let body = resp.text().await.unwrap_or_default();
                error!(
                    "[twilio] Failed to terminate call {}: status={}, body={}",
                    call_id, status, body
                );
                Err(TransportError::SendFailed(format!(
                    "Twilio hangup failed: status={}",
                    status
                )))
            }
        }
    }
}
