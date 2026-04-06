//! OpenAI Realtime STT provider — WebSocket transcription using gpt-4o-transcribe.
//!
//! # Protocol
//!
//! Connection: `wss://api.openai.com/v1/realtime?intent=transcription`
//! Auth:       `Authorization: Bearer <api_key>` header
//!
//! After connecting, we receive `session.created` and respond with
//! `session.update` to configure the transcription session.  Audio is then
//! sent as base-64 encoded PCM via `input_audio_buffer.append` messages.
//!
//! On VAD speech-end, `finalize()` sends `input_audio_buffer.commit` which
//! triggers the server to transcribe the buffered audio.  Results arrive as:
//!   - `conversation.item.input_audio_transcription.delta`  → PartialTranscript
//!   - `conversation.item.input_audio_transcription.completed` → Transcript
//!
//! Audio must be 24 kHz PCM-16 LE.  Our pipeline runs at 16 kHz, so we
//! up-sample each chunk before sending.  Resampling is done in a dedicated
//! background task to avoid blocking the Reactor's Tokio thread.
//!
//! Ref: <https://platform.openai.com/docs/api-reference/realtime>

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tracing::{debug, info, warn};

use crate::providers::ws::{ManagedWsConnection, WsConfig, WsKeepalive, WsStatus};
use crate::types::SttEvent;

use super::SttProvider;

// ── OpenAIRealtimeSttProvider ─────────────────────────────────────────────────

/// Streaming STT via OpenAI's Realtime API transcription session.
pub struct OpenAIRealtimeSttProvider {
    api_key: SecretString,
    model: String,
    language: String,
    base_url: String,
    /// Sends messages to the managed WS connection.
    conn_msg_tx: Option<mpsc::UnboundedSender<Message>>,
    result_rx: Option<mpsc::Receiver<SttEvent>>,
    /// Non-blocking channel for raw PCM audio.  A background task reads from
    /// this, resamples 16→24 kHz, base64-encodes, and writes the JSON envelope
    /// to `conn_msg_tx`.  This keeps `feed_audio(&self)` lock-free and
    /// prevents the Soxr CPU work from blocking the Reactor's Tokio thread.
    audio_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
}

impl OpenAIRealtimeSttProvider {
    pub fn new(api_key: &str, model: &str, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "gpt-4o-transcribe".to_string()
            } else {
                model.to_string()
            },
            language: if language.is_empty() {
                "en".to_string()
            } else {
                language.to_string()
            },
            base_url: "wss://api.openai.com/v1/realtime".to_string(),
            conn_msg_tx: None,
            result_rx: None,
            audio_tx: None,
        }
    }
}

#[async_trait]
impl SttProvider for OpenAIRealtimeSttProvider {
    fn provider_name(&self) -> &str {
        "openai-realtime"
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let url = format!("{}?intent=transcription", self.base_url);
        let api_key = self.api_key.expose_secret().to_string();
        let model = self.model.clone();
        let language = self.language.clone();

        let conn = ManagedWsConnection::connect(
            move || {
                let mut req = url.clone().into_client_request()
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
                let headers = req.headers_mut();
                headers.insert(
                    "Authorization",
                    format!("Bearer {}", api_key).parse()
                        .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue|
                            -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?,
                );
                headers.insert("OpenAI-Beta", "realtime=v1".parse()
                    .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue|
                        -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?);
                Ok(req)
            },
            WsConfig {
                keepalive: WsKeepalive::WsPing {
                    interval: std::time::Duration::from_secs(15),
                },
                max_reconnect_attempts: 3,
                reconnect_delay: std::time::Duration::from_secs(1),
                max_total_reconnect_rounds: 5,
            },
        )
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { e })?;

        let ManagedWsConnection {
            msg_tx,
            mut incoming_rx,
            status_rx,
            ..
        } = conn;

        let (result_tx, result_rx) = mpsc::channel::<SttEvent>(16);

        // Clone msg_tx for the reader task — it needs to send the session.update
        // response back through the managed connection when session.created arrives.
        let reply_tx = msg_tx.clone();

        // ── Resampler task ──────────────────────────────────────────────
        // Runs on a dedicated spawned task. Receives raw 16 kHz PCM from
        // `audio_tx`, resamples to 24 kHz, base64-encodes, and sends the
        // JSON envelope through `msg_tx`.  This keeps `feed_audio` a simple
        // non-blocking channel send with zero CPU work on the Reactor thread.
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let ws_tx_for_resample = msg_tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut resampler = soxr::SoxrStreamResampler::new(
                crate::utils::SAMPLE_RATE,
                crate::utils::TTS_SAMPLE_RATE,
            )
            .expect("Failed to create Soxr for OpenAI STT");
            while let Some(pcm_16k) = audio_rx.blocking_recv() {
                if pcm_16k.is_empty() {
                    continue;
                }
                let audio_24k = resampler.process(&pcm_16k);
                if audio_24k.is_empty() {
                    continue; // resampler may buffer on first call
                }
                let b64 = BASE64.encode(&audio_24k);
                let msg = json!({
                    "type": "input_audio_buffer.append",
                    "audio": b64
                });
                if ws_tx_for_resample
                    .send(Message::Text(msg.to_string().into()))
                    .is_err()
                {
                    break; // WS connection closed
                }
            }
        });

        self.conn_msg_tx = Some(msg_tx);
        self.result_rx = Some(result_rx);
        self.audio_tx = Some(audio_tx);

        // Reader task
        tokio::spawn(async move {
            let mut sent_first_text = false;

            while let Some(msg) = incoming_rx.recv().await {
                if let Message::Text(text) = msg {
                    let Ok(event) = serde_json::from_str::<Value>(&text) else {
                        warn!("[openai-realtime-stt] Parse error: {}", text);
                        continue;
                    };

                    let evt_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

                    // With ?intent=transcription, OpenAI sends
                    // "transcription_session.created"; without it,
                    // "session.created".  Match both for safety.
                    match evt_type {
                        "session.created" | "transcription_session.created" => {
                            debug!("[openai-realtime-stt] Session created, configuring");
                            let update = json!({
                                "type": "transcription_session.update",
                                "session": {
                                    "input_audio_format": "pcm16",
                                    "input_audio_transcription": {
                                        "model": model,
                                        "language": language
                                    },
                                    "turn_detection": null
                                }
                            });
                            let _ = reply_tx.send(Message::Text(update.to_string().into()));
                        }

                        "transcription_session.updated" | "session.updated" => {
                            debug!("[openai-realtime-stt] Session ready");
                        }

                        "conversation.item.input_audio_transcription.delta" => {
                            let delta = event
                                .get("delta")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !delta.is_empty() {
                                if !sent_first_text {
                                    sent_first_text = true;
                                    let _ = result_tx.send(SttEvent::FirstTextReceived).await;
                                }
                                let _ = result_tx.send(SttEvent::PartialTranscript(delta)).await;
                            }
                        }

                        "conversation.item.input_audio_transcription.completed" => {
                            let transcript = event
                                .get("transcript")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            if !transcript.is_empty() {
                                if !sent_first_text {
                                    let _ = result_tx.send(SttEvent::FirstTextReceived).await;
                                }
                                sent_first_text = false;
                                let _ = result_tx.send(SttEvent::Transcript(transcript)).await;
                            }
                        }

                        "conversation.item.input_audio_transcription.failed" | "error" => {
                            warn!("[openai-realtime-stt] Error event: {}", text);
                        }

                        _ => {} // input_audio_buffer.committed, etc.
                    }
                }
            }
            // incoming_rx closed — check if the managed connection died
            let disconnect_reason = match status_rx.borrow().clone() {
                WsStatus::Disconnected { reason } => Some(reason),
                _ => None,
            };
            if let Some(reason) = disconnect_reason {
                let _ = result_tx
                    .send(SttEvent::Error(format!(
                        "openai-realtime STT disconnected: {}",
                        reason
                    )))
                    .await;
            }
        });

        info!(
            "[openai-realtime-stt] Connected (model={}, lang={})",
            self.model, self.language
        );
        Ok(())
    }

    /// Enqueue raw 16 kHz PCM for the background resampler task.
    ///
    /// Non-blocking: just a channel send. The resampler task does the CPU-
    /// intensive 16→24 kHz conversion and base64 encoding on its own thread.
    /// Empty chunks are dropped immediately to avoid OpenAI rejecting
    /// zero-byte `input_audio_buffer.append` messages.
    fn feed_audio(&self, audio: &[u8]) {
        if audio.is_empty() {
            return;
        }
        if let Some(tx) = &self.audio_tx {
            let _ = tx.send(audio.to_vec());
        }
    }

    /// Commit the audio buffer so the server begins transcription.
    fn finalize(&self) {
        let Some(tx) = &self.conn_msg_tx else { return };
        let msg = r#"{"type":"input_audio_buffer.commit"}"#;
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    fn close(&self) {
        // Dropping the sender closes the managed connection naturally.
    }

    fn take_result_rx(&mut self) -> Option<mpsc::Receiver<SttEvent>> {
        self.result_rx.take()
    }
}
