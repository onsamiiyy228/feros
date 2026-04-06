//! Deepgram Aura TTS provider — REST API.
//!
//! Docs: <https://developers.deepgram.com/docs/tts-rest>
//!
//! Request:  POST `https://api.deepgram.com/v1/speak?model=<model>`
//! Auth:     `Authorization: Token <api_key>`
//! Response: WAV or raw PCM (we request `encoding=linear16`)

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::warn;

use super::TtsProvider;

pub struct DeepgramAuraTtsProvider {
    client: Client,
    api_key: SecretString,
    /// Model name (e.g. `"aura-asteria-en"`, `"aura-zeus-en"`).
    model: String,
    output_sample_rate: u32,
    resampler: Option<soxr::SoxrStreamResampler>,
}

impl DeepgramAuraTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32) -> Self {
        Self {
            client: Client::new(),
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "aura-asteria-en".to_string()
            } else {
                model.to_string()
            },
            output_sample_rate,
            resampler: None,
        }
    }
}

/// Deepgram Aura supported output rates.  When the pipeline rate matches one
/// of these, we request it directly to avoid resampling.
const DEEPGRAM_SUPPORTED_RATES: [u32; 4] = [8_000, 16_000, 24_000, 48_000];

/// Fallback rate when the pipeline needs a non-supported rate.
const DEEPGRAM_FALLBACK_RATE: u32 = 24_000;

#[async_trait]
impl TtsProvider for DeepgramAuraTtsProvider {
    fn provider_name(&self) -> &str {
        "deepgram"
    }

    async fn synthesize_chunk(&mut self, text: &str, _voice_id: &str) -> Option<Vec<u8>> {
        // Deepgram Aura encodes the "voice" in the model name (e.g. aura-asteria-en),
        // not as a separate voice_id parameter, so _voice_id is unused.

        // Request the pipeline rate directly if Deepgram supports it;
        // otherwise request 24 kHz and resample client-side.
        let request_rate = if DEEPGRAM_SUPPORTED_RATES.contains(&self.output_sample_rate) {
            self.output_sample_rate
        } else {
            DEEPGRAM_FALLBACK_RATE
        };

        let url = format!(
            "https://api.deepgram.com/v1/speak?model={}&encoding=linear16&sample_rate={}&container=none",
            self.model, request_rate,
        );

        let body = serde_json::json!({ "text": text });

        let resp = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Token {}", self.api_key.expose_secret()),
            )
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
                    "[deepgram-tts] Auth error ({}): {} — check API key",
                    status, body
                );
            } else {
                warn!("[deepgram-tts] Error {}: {}", status, body);
            }
            return None;
        }

        let raw = resp.bytes().await.ok()?.to_vec();
        if raw.is_empty() {
            return None;
        }

        // Deepgram returns raw PCM (no WAV header) when encoding=linear16.
        // Resample only when the requested rate differs from the pipeline rate.
        if request_rate != self.output_sample_rate {
            let resampler = self.resampler.get_or_insert_with(|| {
                soxr::SoxrStreamResampler::new(request_rate, self.output_sample_rate)
                    .expect("SoxrStreamResampler for Deepgram TTS")
            });
            Some(resampler.process(&raw))
        } else {
            Some(raw)
        }
    }
}
