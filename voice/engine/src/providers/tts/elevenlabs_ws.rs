//! ElevenLabs WebSocket TTS provider — multi-stream-input for token streaming.
//!
//! # Protocol
//!
//! ElevenLabs' `/multi-stream-input` endpoint is designed for LLM token streaming:
//! you can send individual tokens and ElevenLabs synthesizes them concurrently.
//!
//! Connection: `wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/multi-stream-input?model_id=<model>`
//! Auth:       `xi-api-key: <api_key>` header
//!
//! Client → Server (JSON):
//!   Turn init:  `{"text":" ","context_id":"<uuid>"}` (sends a space to open the context;
//!               must be the very first message for a given context_id)
//!   Tokens:     `{"text":"Hello ","context_id":"<uuid>"}`
//!   Flush:      `{"context_id":"<uuid>","flush":true}` — end of turn
//!   Barge-in:   `{"context_id":"<uuid>","close_context":true}`
//!   Keepalive (10 s): `{"text":""}` — empty text ping (managed by WsConnection)
//!
//! Server → Client (JSON):
//!   `{"audio":"<base64>","contextId":"<uuid>"}` — PCM audio chunk
//!   `{"alignment":{...},"contextId":"<uuid>"}` — word timestamps (ignored)
//!   `{"contextId":"<uuid>","isFinal":true}` — context closed
//!
//! ElevenLabs produces PCM at a fixed rate; when `output-format=pcm_<rate>` is
//! requested, there is no resampling needed.  We default to `pcm_24000`.
//!
//! Ref: <https://elevenlabs.io/docs/api-reference/text-to-speech/websockets>

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tracing::{info, warn};

use super::{TtsAudioChunk, TtsStreamingProvider};
use crate::providers::ws::{ManagedWsConnection, WsConfig, WsKeepalive, WsStatus};

pub struct ElevenLabsWsTtsProvider {
    api_key: SecretString,
    model_id: String,
    /// ISO 639-1 language code (e.g. `"es"`). Applied to the WS URL only
    /// for models in `ELEVENLABS_MULTILINGUAL_MODELS`.
    language: String,
    output_sample_rate: u32,
    conn_msg_tx: Option<mpsc::UnboundedSender<Message>>,
    audio_rx: Option<mpsc::Receiver<TtsAudioChunk>>,
    /// Voice ID for this connection (set via `set_voice` before `connect`).
    active_voice_id: String,
    /// Tracks context IDs that have already received their init space message.
    /// Cleared on reconnect to avoid sending tokens into a context the new
    /// server doesn't know about.
    initialized_contexts: HashSet<String>,
    /// Watches the managed WS connection lifecycle so we can detect reconnects
    /// and clear stale session state (e.g. `initialized_contexts`).
    status_rx: Option<tokio::sync::watch::Receiver<WsStatus>>,
}

impl ElevenLabsWsTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model_id: if model.is_empty() {
                "eleven_turbo_v2_5".to_string()
            } else {
                model.to_string()
            },
            language: language.to_string(),
            output_sample_rate,
            conn_msg_tx: None,
            audio_rx: None,
            active_voice_id: String::new(),
            initialized_contexts: HashSet::new(),
            status_rx: None,
        }
    }

    fn ws_url(&self, voice_id: &str) -> String {
        use crate::language_config::{elevenlabs_language_code, ELEVENLABS_MULTILINGUAL_MODELS};
        // ElevenLabs websocket API rejects some high quality PCM formats.
        // We reliably request pcm_24000 and use soxr to resample to the requested output rate.
        let rate = "pcm_24000";
        let mut url = format!(
            "wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/multi-stream-input\
             ?model_id={model}\
             &output_format={rate}",
            voice_id = voice_id,
            model = self.model_id,
            rate = rate,
        );
        // Inject language_code only for multilingual models.
        // Older models (eleven_multilingual_v2, eleven_turbo_v2, eleven_flash_v2)
        // do not accept language_code — language is encoded in the voice choice.
        let el_lang = elevenlabs_language_code(&self.language);
        if ELEVENLABS_MULTILINGUAL_MODELS.contains(&self.model_id.as_str()) {
            if !el_lang.is_empty() {
                url.push_str(&format!("&language_code={el_lang}"));
            }
        } else if !el_lang.is_empty() {
            warn!(
                "[elevenlabs-ws] language_code '{}' not applied — model '{}' is not multilingual. \
                 Use one of: {:?}",
                el_lang, self.model_id, ELEVENLABS_MULTILINGUAL_MODELS
            );
        }
        url
    }

    fn output_native_rate(&self) -> u32 {
        24_000
    }
}

#[async_trait]
impl TtsStreamingProvider for ElevenLabsWsTtsProvider {
    fn provider_name(&self) -> &str {
        "elevenlabs-ws"
    }

    /// Store the voice ID so it's available when `connect()` builds the WS URL.
    ///
    /// ElevenLabs WS encodes the voice in the URL at connect time, so this
    /// must be called **before** `connect()`.  The Reactor calls `set_voice`
    /// once during session initialization before connecting.
    fn set_voice(&mut self, voice_id: &str) {
        if !voice_id.is_empty() && voice_id != "default" {
            self.active_voice_id = voice_id.to_string();
        }
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.active_voice_id.is_empty() || self.active_voice_id == "default" {
            return Err(
                "[elevenlabs-ws-tts] No voice_id configured — call set_voice() before connect()"
                    .into(),
            );
        }

        let voice_id = self.active_voice_id.clone();
        let url = self.ws_url(&voice_id);
        let api_key = self.api_key.expose_secret().to_string();
        let output_sample_rate = self.output_sample_rate;
        let native_rate = self.output_native_rate();

        let conn = ManagedWsConnection::connect(
            move || {
                let mut req = url.clone().into_client_request()
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
                req.headers_mut()
                    .insert("xi-api-key", api_key.parse()
                        .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue|
                            -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?);
                Ok(req)
            },
            WsConfig {
                keepalive: WsKeepalive::TextMessage {
                    interval: Duration::from_secs(10),
                    message: r#"{"text":""}"#.to_string(),
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
        // Clone so the reader task can send close_context on isFinal without
        // holding the same sender that the rest of the provider uses.
        let reader_msg_tx = msg_tx.clone();
        self.conn_msg_tx = Some(msg_tx);
        self.audio_rx = Some(audio_rx);
        self.initialized_contexts.clear();
        self.status_rx = Some(status_rx.clone());

        // Reader task: parse ElevenLabs events
        tokio::spawn(async move {
            let mut resampler: Option<soxr::SoxrStreamResampler> = None;

            while let Some(msg) = incoming_rx.recv().await {
                let Message::Text(text) = msg else { continue };
                let Ok(event) = serde_json::from_str::<Value>(&text) else {
                    warn!("[elevenlabs-ws] Parse error: {}", text);
                    continue;
                };

                // Audio chunk
                if let Some(audio_b64) = event.get("audio").and_then(|v| v.as_str()) {
                    let Ok(pcm_native) = BASE64.decode(audio_b64) else {
                        warn!("[elevenlabs-ws] Failed to base64-decode audio");
                        continue;
                    };

                    let pcm = if output_sample_rate != native_rate {
                        let r = resampler.get_or_insert_with(|| {
                            soxr::SoxrStreamResampler::new(native_rate, output_sample_rate)
                                .expect("SoxrStreamResampler for ElevenLabs")
                        });
                        r.process(&pcm_native)
                    } else {
                        pcm_native
                    };

                    let ctx_id = event
                        .get("contextId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let _ = audio_tx
                        .send(TtsAudioChunk {
                            pcm,
                            is_final: false,
                            context_id: ctx_id,
                        })
                        .await;
                }

                // Context done / is_final marker.
                // ElevenLabs keeps the context slot "active" until it receives
                // close_context or a 20s inactivity timeout fires. Without sending
                // close_context here, every normal turn leaves a dangling slot and
                // the connection hits max_active_conversations (limit: 5) after 5 turns.
                if event
                    .get("isFinal")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    let ctx_id = event
                        .get("contextId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Free the server-side context slot immediately.
                    let close = serde_json::json!({"context_id": ctx_id, "close_context": true});
                    let _ = reader_msg_tx.send(Message::Text(close.to_string().into()));

                    let _ = audio_tx
                        .send(TtsAudioChunk {
                            pcm: vec![],
                            is_final: true,
                            context_id: ctx_id,
                        })
                        .await;
                }

                // Error response — ElevenLabs will produce no further audio for this context.
                if let Some(err) = event.get("error").and_then(|v| v.as_str()) {
                    let ctx_id = event
                        .get("contextId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Distinguish permanent (fatal) errors from transient context errors.
                    // Fatal errors cannot be resolved by reconnecting with the same config —
                    // breaking here drops `incoming_rx`, which causes ManagedWsConnection to
                    // detect "receiver dropped" and exit the reconnect loop immediately.
                    let is_fatal = matches!(
                        err,
                        "voice_id_does_not_exist"
                            | "invalid_api_key"
                            | "quota_exceeded"
                            | "model_not_found"
                    );

                    if is_fatal {
                        warn!(
                            "[elevenlabs-ws] Fatal error from server (will not retry): {}",
                            err
                        );
                    } else {
                        warn!("[elevenlabs-ws] Error from server: {}", err);
                    }

                    let _ = audio_tx
                        .send(TtsAudioChunk {
                            pcm: vec![],
                            is_final: true,
                            context_id: ctx_id,
                        })
                        .await;

                    if is_fatal {
                        // Drop incoming_rx by breaking — ManagedWsConnection will see
                        // "receiver dropped" and skip reconnect entirely.
                        break;
                    }
                }
            }
            // incoming_rx closed — check if the managed connection died
            if let WsStatus::Disconnected { reason } = status_rx.borrow().clone() {
                warn!("[elevenlabs-ws] Connection lost: {}", reason);
            }
        });

        info!(
            "[elevenlabs-ws] Connected (model={}, voice={})",
            self.model_id, voice_id
        );
        Ok(())
    }

    async fn send_text(&mut self, text: &str, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };

        // If the managed WS connection just reconnected, clear stale context
        // state — the new server has no memory of previously initialized contexts.
        if let Some(ref mut srx) = self.status_rx {
            if srx.has_changed().unwrap_or(false) {
                let status = srx.borrow_and_update().clone();
                if status == WsStatus::Connected && !self.initialized_contexts.is_empty() {
                    info!(
                        "[elevenlabs-ws] Reconnected — clearing {} stale contexts",
                        self.initialized_contexts.len()
                    );
                    self.initialized_contexts.clear();
                }
            }
        }

        // ElevenLabs requires an init message with a single space for each new context_id.
        if !self.initialized_contexts.contains(context_id) {
            // Cap the set to prevent unbounded growth from abandoned contexts
            // (e.g. barge-in without flush/cancel).
            if self.initialized_contexts.len() >= 100 {
                warn!(
                    "[elevenlabs-ws] initialized_contexts reached {} entries — clearing stale state",
                    self.initialized_contexts.len()
                );
                self.initialized_contexts.clear();
            }
            self.initialized_contexts.insert(context_id.to_string());
            let init = json!({"text": " ", "context_id": context_id});
            let _ = tx.send(Message::Text(init.to_string().into()));
        }

        let msg = json!({"text": text, "context_id": context_id});
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    async fn flush(&mut self, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        self.initialized_contexts.remove(context_id);
        let msg = json!({"context_id": context_id, "flush": true});
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    async fn cancel(&mut self, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        self.initialized_contexts.remove(context_id);
        let msg = json!({"context_id": context_id, "close_context": true});
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    async fn close(&mut self) {
        self.conn_msg_tx = None;
        self.initialized_contexts.clear();
        self.status_rx = None;
    }

    fn take_audio_rx(&mut self) -> Option<mpsc::Receiver<TtsAudioChunk>> {
        self.audio_rx.take()
    }
}
