//! Cartesia WebSocket TTS provider — persistent WS, context-ID multiplexing.
//!
//! # Protocol
//!
//! Connection: `wss://api.cartesia.ai/tts/websocket?cartesia_version=2026-03-01`
//! Auth:       `X-API-Key: <api_key>` + `Cartesia-Version: <ver>` headers
//! Audio:      JSON messages with transcript + context_id → base64 PCM chunks back
//!
//! Each LLM turn gets a fresh UUID `context_id`.  Tokens are streamed in as
//! individual `{"transcript": "...", "context_id": "...", "continue": true}` messages.
//! At turn end, we send `{"context_id": "...", "continue": false}` to flush.
//! For barge-in we send `{"context_id": "...", "cancel": true}`.
//!
//! Responses (JSON):
//!   - `{"type":"chunk","data":"<base64 pcm>","context_id":"..."}` → PCM audio
//!   - `{"type":"done","context_id":"..."}` → is_final marker
//!   - `{"type":"error","..."}` → logged as warning
//!
//! The `voice_id` is supplied per-message in `send_text()`; callers must set
//! `voice_id` before the first `send_text()` by calling `set_voice(voice_id)`.
//!
//! Cartesia WebSocket times out after 5 minutes of inactivity.  The managed
//! connection sends WS-level pings every 60 s to reset the timer and
//! auto-reconnects on connection drop (up to 3 attempts).
//!
//! Ref: <https://docs.cartesia.ai/api-reference/tts/streaming>

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tracing::{info, warn};

use super::{TtsAudioChunk, TtsStreamingProvider};
use crate::providers::ws::{ManagedWsConnection, WsConfig, WsKeepalive, WsStatus};

/// Cartesia's native sample rate for WebSocket TTS output.
const CARTESIA_WS_SAMPLE_RATE: u32 = 24_000;

pub struct CartesiaWsTtsProvider {
    api_key: SecretString,
    model_id: String,
    /// ISO 639-1 language code injected into every message payload.
    /// Empty string = omit field (Cartesia defaults to English).
    language: String,
    /// Voice ID propagated to every `send_text` message.
    voice_id: String,
    output_sample_rate: u32,
    /// Managed WS connection — sends raw WS `Message` values.
    conn_msg_tx: Option<mpsc::UnboundedSender<Message>>,
    /// Audio chunks flow back through this receiver.
    audio_rx: Option<mpsc::Receiver<TtsAudioChunk>>,
}

impl CartesiaWsTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model_id: if model.is_empty() {
                "sonic-2".to_string()
            } else {
                model.to_string()
            },
            language: {
                use crate::language_config::cartesia_language_code;
                cartesia_language_code(language).to_string()
            },
            voice_id: String::new(),
            output_sample_rate,
            conn_msg_tx: None,
            audio_rx: None,
        }
    }

    fn ws_url() -> String {
        format!(
            "wss://api.cartesia.ai/tts/websocket\
             ?cartesia_version={ver}",
            ver = super::cartesia::CARTESIA_API_VERSION,
        )
    }
}

#[async_trait]
impl TtsStreamingProvider for CartesiaWsTtsProvider {
    fn provider_name(&self) -> &str {
        "cartesia-ws"
    }

    fn set_voice(&mut self, voice_id: &str) {
        self.voice_id = voice_id.to_string();
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let url = Self::ws_url();
        let api_key = self.api_key.expose_secret().to_string();
        let output_sample_rate = self.output_sample_rate;

        // Build the managed WS connection with Cartesia-specific keepalive.
        // Auth is via X-API-Key header (not URL query param) to prevent leaking in logs.
        let conn = ManagedWsConnection::connect(
            move || {
                let mut req = url
                    .clone()
                    .into_client_request()
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
                let headers = req.headers_mut();
                headers.insert("X-API-Key", api_key.parse()
                    .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue|
                        -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?);
                headers.insert("Cartesia-Version", super::cartesia::CARTESIA_API_VERSION.parse()
                    .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue|
                        -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?);
                Ok(req)
            },
            WsConfig {
                keepalive: WsKeepalive::WsPing {
                    interval: Duration::from_secs(60),
                },
                max_reconnect_attempts: 3,
                reconnect_delay: Duration::from_secs(1),
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

        let (audio_tx, audio_rx) = mpsc::channel::<TtsAudioChunk>(64);
        self.conn_msg_tx = Some(msg_tx);
        self.audio_rx = Some(audio_rx);

        // Reader task: parse incoming WS messages into TtsAudioChunks.
        // The ManagedWsConnection handles keepalive and reconnect;
        // this task only handles the protocol-specific parsing.
        tokio::spawn(async move {
            // Lazily created on first use if resampling is needed.
            // Kept alive across chunks so filter state is preserved at boundaries.
            let mut resampler: Option<soxr::SoxrStreamResampler> = None;

            while let Some(msg) = incoming_rx.recv().await {
                let Message::Text(text) = msg else { continue };
                let Ok(event) = serde_json::from_str::<Value>(&text) else {
                    warn!("[cartesia-ws] Parse error: {}", text);
                    continue;
                };

                let msg_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let ctx_id = event
                    .get("context_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                match msg_type {
                    "chunk" => {
                        let Some(data_b64) = event.get("data").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let Ok(pcm_native) = BASE64.decode(data_b64) else {
                            warn!("[cartesia-ws] Failed to base64-decode audio chunk");
                            continue;
                        };

                        // Cartesia streams at CARTESIA_WS_SAMPLE_RATE; resample if needed.
                        let pcm = if output_sample_rate != CARTESIA_WS_SAMPLE_RATE {
                            let r = resampler.get_or_insert_with(|| {
                                soxr::SoxrStreamResampler::new(
                                    CARTESIA_WS_SAMPLE_RATE,
                                    output_sample_rate,
                                )
                                .expect("SoxrStreamResampler for Cartesia WS")
                            });
                            r.process(&pcm_native)
                        } else {
                            pcm_native
                        };

                        let _ = audio_tx
                            .send(TtsAudioChunk {
                                pcm,
                                is_final: false,
                                context_id: ctx_id,
                            })
                            .await;
                    }
                    "done" => {
                        let _ = audio_tx
                            .send(TtsAudioChunk {
                                pcm: vec![],
                                is_final: true,
                                context_id: ctx_id,
                            })
                            .await;
                    }
                    "error" => {
                        // Server-side error: Cartesia will produce no further audio
                        // for this context. Emit is_final to close out the turn cleanly.
                        warn!("[cartesia-ws] Error from server: {}", text);
                        let _ = audio_tx
                            .send(TtsAudioChunk {
                                pcm: vec![],
                                is_final: true,
                                context_id: ctx_id,
                            })
                            .await;
                    }
                    _ => {}
                }
            }
            // incoming_rx closed — check if the managed connection died
            if let WsStatus::Disconnected { reason } = status_rx.borrow().clone() {
                warn!("[cartesia-ws] Connection lost: {}", reason);
            }
        });

        info!("[cartesia-ws] Connected (model={})", self.model_id);
        Ok(())
    }

    async fn send_text(&mut self, text: &str, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        if self.voice_id.is_empty() || self.voice_id == "default" {
            warn!("[cartesia-ws-tts] No voice_id configured — skipping send_text");
            return;
        }
        // Cartesia requires a UUID voice ID — catch misconfigured human-readable names early.
        if uuid::Uuid::parse_str(&self.voice_id).is_err() {
            warn!(
                "[cartesia-ws-tts] voice_id '{}' is not a valid UUID — Cartesia requires a UUID (e.g. from play.cartesia.ai). Skipping.",
                self.voice_id
            );
            return;
        }
        let mut msg = json!({
            "model_id": self.model_id,
            "transcript": text,
            "context_id": context_id,
            "continue": true,
            "output_format": {
                "container": "raw",
                "encoding": "pcm_s16le",
                "sample_rate": CARTESIA_WS_SAMPLE_RATE
            },
            "voice": {
                "mode": "id",
                "id": self.voice_id
            }
        });
        if !self.language.is_empty() {
            msg["language"] = serde_json::Value::String(self.language.clone());
        }
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    async fn flush(&mut self, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        // Same guard as send_text: skip if voice_id is missing or not a valid UUID.
        if self.voice_id.is_empty()
            || self.voice_id == "default"
            || uuid::Uuid::parse_str(&self.voice_id).is_err()
        {
            return;
        }
        // Cartesia requires the full spec (model, voice, output_format) on every
        // message in a context — including the final `"continue": false` flush.
        let mut msg = json!({
            "model_id": self.model_id,
            "transcript": "",
            "context_id": context_id,
            "continue": false,
            "output_format": {
                "container": "raw",
                "encoding": "pcm_s16le",
                "sample_rate": CARTESIA_WS_SAMPLE_RATE
            },
            "voice": {
                "mode": "id",
                "id": self.voice_id
            }
        });
        if !self.language.is_empty() {
            msg["language"] = serde_json::Value::String(self.language.clone());
        }
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    async fn cancel(&mut self, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        let msg = json!({
            "context_id": context_id,
            "cancel": true
        });
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    async fn close(&mut self) {
        self.conn_msg_tx = None; // drops the sender → managed loop exits cleanly
    }

    fn take_audio_rx(&mut self) -> Option<mpsc::Receiver<TtsAudioChunk>> {
        self.audio_rx.take()
    }
}
