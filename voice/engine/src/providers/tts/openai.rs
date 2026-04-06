//! OpenAI TTS provider — REST API.
//!
//! Docs: <https://platform.openai.com/docs/api-reference/audio/createSpeech>
//!
//! Request:  POST `https://api.openai.com/v1/audio/speech`
//! Auth:     `Authorization: Bearer <api_key>`
//! Response: raw PCM at 24 kHz (via `response_format: "pcm"`)

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::warn;

use super::TtsProvider;

pub struct OpenAiTtsProvider {
    client: Client,
    api_key: SecretString,
    model: String,
    output_sample_rate: u32,
    /// Lazy resampler — created on first use if 24kHz ≠ output_sample_rate.
    resampler: Option<soxr::SoxrStreamResampler>,
}

impl OpenAiTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32) -> Self {
        Self {
            client: Client::new(),
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "gpt-4o-mini-tts".to_string()
            } else {
                model.to_string()
            },
            output_sample_rate,
            resampler: None,
        }
    }
}

// OpenAI PCM output is always 24 kHz mono 16-bit LE
const OPENAI_NATIVE_RATE: u32 = 24_000;

#[async_trait]
impl TtsProvider for OpenAiTtsProvider {
    fn provider_name(&self) -> &str {
        "openai"
    }

    async fn synthesize_chunk(&mut self, text: &str, voice_id: &str) -> Option<Vec<u8>> {
        // OpenAI voices: alloy, ash, coral, echo, fable, onyx, nova, sage, shimmer
        let voice = if voice_id.is_empty() || voice_id == "default" {
            "alloy"
        } else {
            voice_id
        };

        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": voice,
            "response_format": "pcm",   // raw PCM-16 LE at 24 kHz, no container
        });

        let resp = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(self.api_key.expose_secret())
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
                    "[openai-tts] Auth error ({}): {} — check API key",
                    status, body
                );
            } else {
                warn!("[openai-tts] Error {}: {}", status, body);
            }
            return None;
        }

        let raw = resp.bytes().await.ok()?.to_vec();
        if raw.is_empty() {
            return None;
        }

        if OPENAI_NATIVE_RATE != self.output_sample_rate {
            let resampler = self.resampler.get_or_insert_with(|| {
                soxr::SoxrStreamResampler::new(OPENAI_NATIVE_RATE, self.output_sample_rate)
                    .expect("SoxrStreamResampler for OpenAI TTS")
            });
            Some(resampler.process(&raw))
        } else {
            Some(raw)
        }
    }
}
