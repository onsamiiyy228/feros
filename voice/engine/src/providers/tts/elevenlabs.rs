//! ElevenLabs TTS provider — REST API.
//!
//! Docs: <https://elevenlabs.io/docs/api-reference/text-to-speech>
//!
//! Request:  POST `https://api.elevenlabs.io/v1/text-to-speech/{voice_id}`
//! Auth:     `xi-api-key: <api_key>`
//! Response: MP3 or PCM (we request `pcm_16000` or `pcm_24000`)

use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::warn;

use super::TtsProvider;

pub struct ElevenLabsTtsProvider {
    client: Client,
    api_key: SecretString,
    model_id: String,
    /// ElevenLabs language code resolved at construction.
    /// Non-empty only for multilingual models (see `ELEVENLABS_MULTILINGUAL_MODELS`);
    /// appended as `&language_code=<lang>` to the REST URL.
    language: String,
    output_sample_rate: u32,
    resampler: Option<soxr::SoxrStreamResampler>,
}

impl ElevenLabsTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32, language: &str) -> Self {
        use crate::language_config::{elevenlabs_language_code, ELEVENLABS_MULTILINGUAL_MODELS};
        let model_id = if model.is_empty() {
            "eleven_turbo_v2_5".to_string()
        } else {
            model.to_string()
        };
        // Only resolve language for multilingual models; others encode language via voice.
        let language = if ELEVENLABS_MULTILINGUAL_MODELS.contains(&model_id.as_str()) {
            elevenlabs_language_code(language).to_string()
        } else {
            String::new()
        };
        Self {
            client: Client::new(),
            api_key: SecretString::from(api_key.to_string()),
            model_id,
            language,
            output_sample_rate,
            resampler: None,
        }
    }

    fn pcm_output_format(&self) -> &'static str {
        // ElevenLabs occasionally ignores `pcm_8000` and falls back to 44100, which
        // severely distorts Twilio audio. We request 24000 and let soxr downsample to 8000.
        match self.output_sample_rate {
            16_000 => "pcm_16000",
            22_050 => "pcm_22050",
            44_100 => "pcm_44100",
            _ => "pcm_24000",
        }
    }

    fn native_rate(&self) -> u32 {
        match self.output_sample_rate {
            16_000 | 22_050 | 44_100 => self.output_sample_rate,
            _ => 24_000,
        }
    }
}

#[async_trait]
impl TtsProvider for ElevenLabsTtsProvider {
    fn provider_name(&self) -> &str {
        "elevenlabs"
    }

    async fn synthesize_chunk(&mut self, text: &str, voice_id: &str) -> Option<Vec<u8>> {
        if voice_id.is_empty() || voice_id == "default" {
            warn!("[elevenlabs-tts] No voice_id configured — skipping synthesis");
            return None;
        }

        let mut url = format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{}/stream?output_format={}",
            voice_id,
            self.pcm_output_format()
        );
        if !self.language.is_empty() {
            url.push_str(&format!("&language_code={}", self.language));
        }

        let body = serde_json::json!({
            "text": text,
            "model_id": self.model_id,
            "voice_settings": {
                "stability": 0.5,
                "similarity_boost": 0.75,
            },
        });

        let resp = self
            .client
            .post(&url)
            .header("xi-api-key", self.api_key.expose_secret())
            .header("Content-Type", "application/json")
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
                    "[elevenlabs-tts] Auth error ({}): {} — check API key",
                    status, body
                );
            } else {
                warn!("[elevenlabs-tts] Error {}: {}", status, body);
            }
            return None;
        }

        let raw = resp.bytes().await.ok()?.to_vec();
        if raw.is_empty() {
            return None;
        }

        let native = self.native_rate();
        if native != self.output_sample_rate {
            let resampler = self.resampler.get_or_insert_with(|| {
                soxr::SoxrStreamResampler::new(native, self.output_sample_rate)
                    .expect("SoxrStreamResampler for ElevenLabs")
            });
            Some(resampler.process(&raw))
        } else {
            Some(raw)
        }
    }
}
