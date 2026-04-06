//! Cartesia TTS provider — HTTP REST API.
//!
//! Docs: <https://docs.cartesia.ai/api-reference/tts/tts>
//!
//! Request:  POST `https://api.cartesia.ai/tts/bytes`
//! Auth:     `X-API-Key: <api_key>`
//! Response: Raw PCM-16 LE at the requested sample rate (or WAV)

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::warn;

use super::{builtin, TtsProvider};

/// API version string shared by both the HTTP and WS Cartesia providers.
///
/// Using a single constant prevents the HTTP vs WS version mismatch flagged
/// in review item #10.
pub(super) const CARTESIA_API_VERSION: &str = "2026-03-01";

pub struct CartesiaTtsProvider {
    client: Client,
    api_key: SecretString,
    model_id: String,
    /// Cartesia language code resolved at construction via `cartesia_language_code()`.
    /// Empty string = omit field (Cartesia defaults to English).
    language: String,
    output_sample_rate: u32,
    resampler: Option<soxr::SoxrStreamResampler>,
}

impl CartesiaTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32, language: &str) -> Self {
        use crate::language_config::cartesia_language_code;
        Self {
            client: Client::new(),
            api_key: SecretString::from(api_key.to_string()),
            model_id: if model.is_empty() {
                "sonic-english".to_string()
            } else {
                model.to_string()
            },
            language: cartesia_language_code(language).to_string(),
            output_sample_rate,
            resampler: None,
        }
    }
}

#[async_trait]
impl TtsProvider for CartesiaTtsProvider {
    fn provider_name(&self) -> &str {
        "cartesia"
    }

    async fn synthesize_chunk(&mut self, text: &str, voice_id: &str) -> Option<Vec<u8>> {
        if voice_id.is_empty() || voice_id == "default" {
            warn!("[cartesia-tts] No voice_id configured — skipping synthesis");
            return None;
        }

        let mut body = serde_json::json!({
            "model_id": self.model_id,
            "transcript": text,
            "voice": {
                "mode": "id",
                "id": voice_id,
            },
            "output_format": {
                "container": "raw",
                "encoding": "pcm_s16le",
                "sample_rate": self.output_sample_rate,
            },
        });
        if !self.language.is_empty() {
            body["language"] = serde_json::Value::String(self.language.clone());
        }

        let resp = self
            .client
            .post("https://api.cartesia.ai/tts/bytes")
            .header("X-API-Key", self.api_key.expose_secret())
            .header("Cartesia-Version", CARTESIA_API_VERSION)
            .json(&body)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                warn!(
                    "[cartesia-tts] Auth error ({}): {} — check API key",
                    status, body
                );
            } else {
                warn!("[cartesia-tts] Error {}: {}", status, body);
            }
            return None;
        }

        let raw = resp.bytes().await.ok()?.to_vec();
        if raw.is_empty() {
            return None;
        }

        // Normally Cartesia returns raw PCM at the requested sample_rate.
        // On the rare occasion it returns WAV, decode it via the shared helper
        // from builtin — avoids duplicating WAV parsing logic.
        if raw.len() > 44 && &raw[..4] == b"RIFF" {
            let decoded = builtin::parse_wav(&raw)?;
            if decoded.sample_rate != self.output_sample_rate {
                let resampler = self.resampler.get_or_insert_with(|| {
                    soxr::SoxrStreamResampler::new(decoded.sample_rate, self.output_sample_rate)
                        .expect("SoxrStreamResampler for Cartesia")
                });
                return Some(resampler.process(&decoded.pcm));
            }
            return Some(decoded.pcm);
        }

        Some(raw)
    }
}
