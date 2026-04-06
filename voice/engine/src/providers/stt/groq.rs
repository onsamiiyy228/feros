//! Groq Whisper STT provider — segmented HTTP transcription.
//!
//! # How it works
//!
//! Unlike streaming WS providers, Groq Whisper is a **segmented** provider:
//! 1. `feed_audio()` accumulates raw PCM-16 LE frames in an internal buffer.
//! 2. `finalize()` drains the buffer, wraps it in a WAV header, and POSTs it to
//!    `https://api.groq.com/openai/v1/audio/transcriptions` (OpenAI-compatible).
//! 3. The transcript arrives over the `result_rx` channel, same as WS providers.
//!
//! This design keeps `SttStage` and the Reactor completely unaware of the HTTP
//! transport — they see the same `SttEvent` stream as all other providers.
//!
//! # Thread safety
//!
//! The `SttProvider` trait takes `&self` on `feed_audio` and `finalize` (to
//! match WS providers that send to unbounded channels).  The PCM buffer is
//! protected by a `Mutex` so both methods can share the reference safely.
//!
//! # Supported models
//!
//! - `whisper-large-v3-turbo` (default) — best speed/accuracy tradeoff
//! - `whisper-large-v3` — highest accuracy
//! - `distil-whisper-large-v3-en` — English-only, fastest
//!
//! Ref: <https://console.groq.com/docs/speech-text>

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::types::SttEvent;

use super::SttProvider;
use crate::utils::pcm_to_wav;

// ── GroqWhisperSttProvider ────────────────────────────────────────────────────

/// Segmented STT via Groq's Whisper API (OpenAI-compatible `/audio/transcriptions`).
pub struct GroqWhisperSttProvider {
    api_key: SecretString,
    model: String,
    language: String,
    base_url: String,
    /// Reused across all finalize() calls to benefit from connection pooling.
    client: reqwest::Client,
    /// PCM buffer shared between `feed_audio` (&self) and `finalize` (&self).
    pcm_buffer: Arc<Mutex<Vec<u8>>>,
    result_tx: Option<mpsc::Sender<SttEvent>>,
    result_rx: Option<mpsc::Receiver<SttEvent>>,
}

impl GroqWhisperSttProvider {
    pub fn new(api_key: &str, model: &str, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "whisper-large-v3-turbo".to_string()
            } else {
                model.to_string()
            },
            language: language.to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest Client for Groq"),
            pcm_buffer: Arc::new(Mutex::new(Vec::new())),
            result_tx: None,
            result_rx: None,
        }
    }
}

#[async_trait]
impl SttProvider for GroqWhisperSttProvider {
    fn provider_name(&self) -> &str {
        "groq-whisper"
    }

    /// No persistent connection needed — create the result channel and clear the buffer.
    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::channel::<SttEvent>(8);
        self.result_tx = Some(tx);
        self.result_rx = Some(rx);
        self.pcm_buffer.lock().unwrap().clear();
        info!("[groq-whisper] Ready (model={})", self.model);
        Ok(())
    }

    /// Append raw PCM-16 LE audio. Non-blocking; grabs the Mutex briefly.
    fn feed_audio(&self, audio: &[u8]) {
        if let Ok(mut buf) = self.pcm_buffer.lock() {
            buf.extend_from_slice(audio);
        }
    }

    /// Drain the buffer, wrap in WAV, and POST to Groq in a background task.
    fn finalize(&self) {
        let pcm = {
            let Ok(mut buf) = self.pcm_buffer.lock() else {
                return;
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

        let Some(tx) = self.result_tx.clone() else {
            warn!("[groq-whisper] finalize() called before connect() — transcript discarded");
            return;
        };

        let api_key = self.api_key.expose_secret().to_string();
        let model = self.model.clone();
        let language = self.language.clone();
        let url = format!("{}/audio/transcriptions", self.base_url);
        let client = self.client.clone();

        tokio::spawn(async move {
            let wav = pcm_to_wav(&pcm);

            let part = match reqwest::multipart::Part::bytes(wav)
                .file_name("audio.wav")
                .mime_str("audio/wav")
            {
                Ok(p) => p,
                Err(e) => {
                    warn!("[groq-whisper] Failed to build multipart part: {}", e);
                    return;
                }
            };

            let mut form = reqwest::multipart::Form::new()
                .part("file", part)
                .text("model", model)
                .text("response_format", "json");

            if !language.is_empty() {
                form = form.text("language", language);
            }

            let resp = client
                .post(&url)
                .bearer_auth(&api_key)
                .multipart(form)
                .send()
                .await;

            match resp {
                Err(e) => {
                    warn!("[groq-whisper] HTTP request failed: {}", e);
                    // Unblock the reactor immediately — don't wait for SttTimeout.
                    let _ = tx.send(SttEvent::Transcript("".into())).await;
                }
                Ok(r) if !r.status().is_success() => {
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    warn!("[groq-whisper] HTTP {} — {}", status, body);
                    // Unblock the reactor immediately — don't wait for SttTimeout.
                    let _ = tx.send(SttEvent::Transcript("".into())).await;
                }
                Ok(r) => {
                    let json: serde_json::Value = match r.json().await {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("[groq-whisper] Failed to parse response: {}", e);
                            return;
                        }
                    };

                    let text = json
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();

                    info!("[groq-whisper] Transcript: {:?}", text);
                    if !text.is_empty() {
                        let _ = tx.send(SttEvent::FirstTextReceived).await;
                    }
                    let _ = tx.send(SttEvent::Transcript(text)).await;
                }
            }
        });
    }

    /// Clear the buffer — no socket to close.
    ///
    /// If called without a preceding `finalize()` call (e.g. abrupt session
    /// shutdown or barge-in), any buffered audio is silently discarded.  A
    /// warning is emitted so operators can diagnose missed utterances.
    fn close(&self) {
        if let Ok(mut buf) = self.pcm_buffer.lock() {
            if !buf.is_empty() {
                warn!(
                    "[groq-whisper] close() called with {} buffered PCM bytes \
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
