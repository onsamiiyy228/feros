//! ElevenLabs STT provider — segmented HTTP transcription.
//!
//! # How it works
//!
//! Same segmented pattern as `GroqWhisperSttProvider`:
//! 1. `feed_audio()` accumulates raw PCM-16 LE frames in an internal buffer.
//! 2. `finalize()` drains the buffer, wraps it in a WAV, and POSTs it to
//!    `https://api.elevenlabs.io/v1/speech-to-text`.
//! 3. The transcript arrives over the `result_rx` channel.
//!
//! # Language codes
//!
//! ElevenLabs uses ISO 639-3 three-letter codes (`eng`, `spa`, `fra`) while
//! our pipeline uses ISO 639-1 two-letter codes (`en`, `es`, `fr`).  The
//! provider normalizes at the boundary via [`to_elevenlabs_language`].
//!
//! # Default model
//!
//! - `scribe_v2` — ElevenLabs' latest transcription model
//!
//! Ref: <https://elevenlabs.io/docs/api-reference/speech-to-text/convert>

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::types::SttEvent;

use super::SttProvider;

// Reuse the WAV helper from the top-level utils module.
use crate::utils::pcm_to_wav;

// ── Language normalization ───────────────────────────────────────

/// Convert ISO 639-1 two-letter codes to ElevenLabs ISO 639-3 codes.
///
/// If the input is already a 3-letter code, it's passed through unchanged.
/// Unknown codes are passed through as-is (the API will reject them with
/// a clear error message).
fn to_elevenlabs_language(lang: &str) -> String {
    match lang {
        "en" => "eng",
        "zh" => "zho",
        "es" => "spa",
        "hi" => "hin",
        "ar" => "ara",
        "pt" => "por",
        "bn" => "ben",
        "ru" => "rus",
        "ja" => "jpn",
        "de" => "deu",
        "fr" => "fra",
        "it" => "ita",
        "ko" => "kor",
        "nl" => "nld",
        "pl" => "pol",
        "tr" => "tur",
        "vi" => "vie",
        "th" => "tha",
        "sv" => "swe",
        "da" => "dan",
        "fi" => "fin",
        "no" => "nor",
        "el" => "ell",
        "cs" => "ces",
        "ro" => "ron",
        "hu" => "hun",
        "uk" => "ukr",
        "id" => "ind",
        "ms" => "msa",
        "he" => "heb",
        "fa" => "fas",
        "ta" => "tam",
        "ur" => "urd",
        "sw" => "swa",
        "af" => "afr",
        "hr" => "hrv",
        "bg" => "bul",
        "sk" => "slk",
        "sl" => "slv",
        "sr" => "srp",
        "ca" => "cat",
        "ga" => "gle",
        "cy" => "cym",
        "et" => "est",
        "lv" => "lav",
        "lt" => "lit",
        "mt" => "mlt",
        "ka" => "kat",
        "hy" => "hye",
        "pa" => "pan",
        "gu" => "guj",
        "kn" => "kan",
        "ml" => "mal",
        "te" => "tel",
        "mr" => "mar",
        "ne" => "nep",
        "km" => "khm",
        "lo" => "lao",
        "my" => "mya",
        "mn" => "mon",
        "am" => "amh",
        "sd" => "snd",
        other => return other.to_string(), // pass through 3-letter codes or unknowns
    }
    .to_string()
}

// ── Provider ─────────────────────────────────────────────────────

/// Segmented STT via ElevenLabs' speech-to-text API.
pub struct ElevenLabsSttProvider {
    api_key: SecretString,
    model: String,
    /// Stored as ISO 639-3 (e.g. `eng`), normalized from the pipeline's 639-1 codes.
    language: String,
    client: reqwest::Client,
    pcm_buffer: Arc<Mutex<Vec<u8>>>,
    result_tx: Option<mpsc::Sender<SttEvent>>,
    result_rx: Option<mpsc::Receiver<SttEvent>>,
}

impl ElevenLabsSttProvider {
    pub fn new(api_key: &str, model: &str, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "scribe_v2".to_string()
            } else {
                model.to_string()
            },
            language: to_elevenlabs_language(language),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest Client for ElevenLabs STT"),
            pcm_buffer: Arc::new(Mutex::new(Vec::new())),
            result_tx: None,
            result_rx: None,
        }
    }
}

#[async_trait]
impl SttProvider for ElevenLabsSttProvider {
    fn provider_name(&self) -> &str {
        "elevenlabs"
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::channel::<SttEvent>(16);
        self.result_tx = Some(tx);
        self.result_rx = Some(rx);
        info!("[elevenlabs-stt] Ready (model={})", self.model);
        Ok(())
    }

    fn feed_audio(&self, audio: &[u8]) {
        if let Ok(mut buf) = self.pcm_buffer.lock() {
            buf.extend_from_slice(audio);
        }
    }

    fn finalize(&self) {
        let pcm = {
            let mut buf = match self.pcm_buffer.lock() {
                Ok(b) => b,
                Err(_) => return,
            };
            std::mem::take(&mut *buf)
        };

        if pcm.is_empty() {
            // No audio gathered (e.g. extremely short barge-in).
            // Instantly emit empty transcript to unblock the Reactor lifecycle.
            if let Some(tx) = &self.result_tx {
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    let _ = tx_clone.send(SttEvent::Transcript("".into())).await;
                });
            }
            return;
        }

        let wav = pcm_to_wav(&pcm);
        let tx = match &self.result_tx {
            Some(tx) => tx.clone(),
            None => return,
        };

        let client = self.client.clone();
        let api_key = self.api_key.expose_secret().to_string();
        let model = self.model.clone();
        let language = self.language.clone();

        tokio::spawn(async move {
            let file_part = reqwest::multipart::Part::bytes(wav)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .unwrap();

            let mut form = reqwest::multipart::Form::new()
                .part("file", file_part)
                .text("model_id", model);

            if !language.is_empty() {
                form = form.text("language_code", language);
            }

            match client
                .post("https://api.elevenlabs.io/v1/speech-to-text")
                .header("xi-api-key", &api_key)
                .multipart(form)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        let text = body["text"].as_str().unwrap_or("").trim().to_string();
                        if !text.is_empty() {
                            let _ = tx.send(SttEvent::FirstTextReceived).await;
                        }
                        let _ = tx.send(SttEvent::Transcript(text)).await;
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    warn!("[elevenlabs-stt] Error {}: {}", status, body);
                    // Unblock the reactor immediately — don't wait for SttTimeout.
                    let _ = tx.send(SttEvent::Transcript("".into())).await;
                }
                Err(e) => {
                    warn!("[elevenlabs-stt] Request failed: {}", e);
                    // Unblock the reactor immediately — don't wait for SttTimeout.
                    let _ = tx.send(SttEvent::Transcript("".into())).await;
                }
            }
        });
    }

    fn close(&self) {
        // No persistent connection to close — just clear any buffered audio.
        if let Ok(mut buf) = self.pcm_buffer.lock() {
            if !buf.is_empty() {
                warn!(
                    "[elevenlabs-stt] close() called with {} buffered PCM bytes \
                     — audio discarded without transcription",
                    buf.len()
                );
            }
            buf.clear();
        }
    }

    fn take_result_rx(&mut self) -> Option<mpsc::Receiver<SttEvent>> {
        self.result_rx.take()
    }
}
