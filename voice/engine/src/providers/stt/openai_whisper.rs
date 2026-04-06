//! OpenAI Whisper STT provider — segmented HTTP transcription.
//!
//! # How it works
//!
//! Same segmented pattern as `GroqWhisperSttProvider`:
//! 1. `feed_audio()` accumulates raw PCM-16 LE frames in an internal buffer.
//! 2. `finalize()` drains the buffer, wraps it in a WAV, and POSTs it to
//!    `https://api.openai.com/v1/audio/transcriptions`.
//! 3. The transcript arrives over the `result_rx` channel.
//!
//! # Supported models
//!
//! - `gpt-4o-transcribe` (default) — best accuracy + speed
//! - `gpt-4o-mini-transcribe` — faster, lower cost
//! - `whisper-1` — legacy Whisper model
//!
//! Ref: <https://platform.openai.com/docs/api-reference/audio/createTranscription>

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::types::SttEvent;

use super::SttProvider;

// Reuse the WAV helper from the top-level utils module.
use crate::utils::pcm_to_wav;

/// Segmented STT via OpenAI's Whisper / GPT-4o-transcribe API.
pub struct OpenAiWhisperSttProvider {
    api_key: SecretString,
    model: String,
    language: String,
    client: reqwest::Client,
    pcm_buffer: Arc<Mutex<Vec<u8>>>,
    result_tx: Option<mpsc::Sender<SttEvent>>,
    result_rx: Option<mpsc::Receiver<SttEvent>>,
}

impl OpenAiWhisperSttProvider {
    pub fn new(api_key: &str, model: &str, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "gpt-4o-transcribe".to_string()
            } else {
                model.to_string()
            },
            language: language.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest Client for OpenAI Whisper STT"),
            pcm_buffer: Arc::new(Mutex::new(Vec::new())),
            result_tx: None,
            result_rx: None,
        }
    }
}

#[async_trait]
impl SttProvider for OpenAiWhisperSttProvider {
    fn provider_name(&self) -> &str {
        "openai-whisper"
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::channel::<SttEvent>(16);
        self.result_tx = Some(tx);
        self.result_rx = Some(rx);
        info!("[openai-whisper-stt] Ready (model={})", self.model);
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
                .text("model", model)
                .text("response_format", "json");

            if !language.is_empty() {
                form = form.text("language", language);
            }

            match client
                .post("https://api.openai.com/v1/audio/transcriptions")
                .bearer_auth(&api_key)
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
                    warn!("[openai-whisper-stt] Error {}: {}", status, body);
                    // Unblock the reactor immediately — don't wait for SttTimeout.
                    let _ = tx.send(SttEvent::Transcript("".into())).await;
                }
                Err(e) => {
                    warn!("[openai-whisper-stt] Request failed: {}", e);
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
                    "[openai-whisper-stt] close() called with {} buffered PCM bytes \
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
