//! Groq TTS provider — REST API (Playai).
//!
//! Docs: <https://console.groq.com/docs/text-audio>
//!
//! Request:  POST `https://api.groq.com/openai/v1/audio/speech`
//! Auth:     `Authorization: Bearer <api_key>`
//! Response: WAV audio at 24 kHz (via `response_format: "wav"`)
//!
//! # Default model
//!
//! - `canopylabs/orpheus-v1-english` — Groq-hosted Orpheus English TTS
//! - `canopylabs/orpheus-arabic-saudi` — Groq-hosted Orpheus Arabic TTS
//!
//! # Sample rate
//!
//! Groq TTS returns WAV at 24 kHz.  We parse the WAV to extract raw PCM
//! and resample to the pipeline's output rate if needed.

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::warn;

use super::builtin;
use super::TtsProvider;

pub struct GroqTtsProvider {
    client: Client,
    api_key: SecretString,
    model: String,
    output_sample_rate: u32,
    /// Lazy resampler — created on first use if native rate ≠ output rate.
    resampler: Option<soxr::SoxrStreamResampler>,
}

/// Groq TTS returns WAV at 24 kHz.
const GROQ_NATIVE_RATE: u32 = 24_000;

impl GroqTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32) -> Self {
        Self {
            client: Client::new(),
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "canopylabs/orpheus-v1-english".to_string()
            } else {
                model.to_string()
            },
            output_sample_rate,
            resampler: None,
        }
    }
}

#[async_trait]
impl TtsProvider for GroqTtsProvider {
    fn provider_name(&self) -> &str {
        "groq"
    }

    async fn synthesize_chunk(&mut self, text: &str, voice_id: &str) -> Option<Vec<u8>> {
        let voice = if voice_id.is_empty() || voice_id == "default" {
            "autumn" // default Orpheus English voice
        } else {
            voice_id
        };

        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": voice,
            "response_format": "wav",
        });

        let resp = self
            .client
            .post("https://api.groq.com/openai/v1/audio/speech")
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!("[groq-tts] Error {}: {}", status, body);
            return None;
        }

        let wav_data = resp.bytes().await.ok()?.to_vec();
        if wav_data.is_empty() {
            return None;
        }

        // Parse WAV to extract raw PCM using shared parser
        let decoded = match builtin::parse_wav(&wav_data) {
            Some(d) => d,
            None => {
                warn!("[groq-tts] Failed to parse WAV response");
                return None;
            }
        };

        if decoded.pcm.is_empty() {
            return None;
        }

        // Resample if needed
        let actual_rate = if decoded.sample_rate > 0 {
            decoded.sample_rate
        } else {
            GROQ_NATIVE_RATE
        };
        if actual_rate != self.output_sample_rate {
            let resampler = self.resampler.get_or_insert_with(|| {
                soxr::SoxrStreamResampler::new(actual_rate, self.output_sample_rate)
                    .expect("SoxrStreamResampler for Groq TTS")
            });
            Some(resampler.process(&decoded.pcm))
        } else {
            Some(decoded.pcm)
        }
    }
}
