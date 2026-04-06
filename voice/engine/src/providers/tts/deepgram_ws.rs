//! Deepgram Aura WebSocket TTS provider — binary PCM frames, Flush/Clear protocol.
//!
//! # Protocol
//!
//! Connection: `wss://api.deepgram.com/v1/speak?model=<voice>&encoding=linear16&sample_rate=<rate>`
//! Auth:       `Authorization: Token <api_key>` header
//!
//! Client → Server:
//!   - `{"type":"Speak","text":"..."}` — queue text for synthesis
//!   - `{"type":"Flush"}`    — end of turn; server sends all remaining audio + Flushed msg
//!   - `{"type":"Clear"}`    — barge-in; discards server buffer
//!   - `{"type":"Close"}`    — graceful close
//!
//! Server → Client:
//!   - Binary frames: raw PCM-16 LE at the requested sample rate (no base64!)
//!   - `{"type":"Metadata"}`  — session info
//!   - `{"type":"Flushed"}`   — all audio for the current Flush is sent → is_final
//!   - `{"type":"Cleared"}`   — barge-in acknowledged
//!   - `{"type":"Warning"}`   — non-fatal warning
//!
//! Deepgram Aura-2 closes the connection from their side immediately after
//! sending `Flushed`. `ManagedWsConnection` handles this gracefully: when
//! Deepgram closes the connection at the end of a turn, it immediately
//! reconnects in the background. This effectively provides a "pre-warmed"
//! connection for the next turn, keeping TTFB (Time to First Byte) very low.
//!
//! Ref: <https://developers.deepgram.com/docs/tts-websocket>

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tracing::{info, warn};

use super::{TtsAudioChunk, TtsStreamingProvider};
use crate::providers::ws::{ManagedWsConnection, WsConfig, WsKeepalive, WsStatus};

pub struct DeepgramWsTtsProvider {
    api_key: SecretString,
    /// Deepgram Aura voice model name (e.g. `"aura-2-helena-en"`).
    voice: String,
    output_sample_rate: u32,
    conn_msg_tx: Option<mpsc::UnboundedSender<Message>>,
    audio_rx: Option<mpsc::Receiver<TtsAudioChunk>>,
    /// Publishes the active context_id to the reader task so it can tag
    /// audio frames and `Flushed` events correctly.
    /// Deepgram doesn't echo context_id in server responses; we mirror it
    /// from the send side via this watch channel.
    context_id_tx: Option<watch::Sender<String>>,
    /// Snapshot of the context_id that was current when `cancel()` was called.
    /// By the time Deepgram sends `Cleared`, `context_id_tx` may already hold
    /// the *new* turn's id. We tag `Cleared` with this stale id so the reactor
    /// drops it as expected rather than forwarding it to the wrong turn.
    cancel_ctx_tx: Option<watch::Sender<String>>,
    /// Instructs the reader thread to drop in-flight binary frames while waiting
    /// for a `Cleared` message after a barge-in.
    clear_tx: Option<watch::Sender<bool>>,
}

impl DeepgramWsTtsProvider {
    pub fn new(api_key: &str, model: &str, output_sample_rate: u32) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            voice: if model.is_empty() {
                "aura-2-helena-en".to_string()
            } else {
                model.to_string()
            },
            output_sample_rate,
            conn_msg_tx: None,
            audio_rx: None,
            context_id_tx: None,
            cancel_ctx_tx: None,
            clear_tx: None,
        }
    }

    fn ws_url(&self) -> String {
        format!(
            "wss://api.deepgram.com/v1/speak\
             ?model={model}\
             &encoding=linear16\
             &sample_rate={sr}",
            model = self.voice,
            sr = self.output_sample_rate,
        )
    }
}

#[async_trait]
impl TtsStreamingProvider for DeepgramWsTtsProvider {
    fn provider_name(&self) -> &str {
        "deepgram-ws"
    }

    /// Update the Deepgram Aura voice model.
    ///
    /// Deepgram WS encodes the voice in the connection URL (built by `ws_url()`),
    /// so this must be called **before** `connect()`.  If called after `connect()`
    /// the change only takes effect on the next reconnect.
    /// The Reactor calls `set_voice` once during session initialization before
    /// connecting.
    fn set_voice(&mut self, voice_id: &str) {
        if !voice_id.is_empty() && voice_id != "default" {
            self.voice = voice_id.to_string();
        }
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let url = self.ws_url();
        let api_key = self.api_key.expose_secret().to_string();

        let conn = ManagedWsConnection::connect(
            move || {
                let mut req = url.clone().into_client_request()
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
                req.headers_mut().insert(
                    "Authorization",
                    format!("Token {}", api_key).parse()
                        .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue|
                            -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?,
                );
                Ok(req)
            },
            WsConfig {
                keepalive: WsKeepalive::WsPing {
                    interval: Duration::from_secs(5),
                },
                max_reconnect_attempts: 3,
                reconnect_delay: Duration::from_secs(1),
                // Deepgram closes the connection after every turn. Allow unlimited
                // reconnect rounds so the background connection stays warm forever.
                max_total_reconnect_rounds: u32::MAX,
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
        // context_id watch: send side updates on every send_text(); reader tags audio + Flushed.
        let (ctx_tx, ctx_rx) = watch::channel(String::new());
        // cancel_ctx watch: snapshotted at cancel() time; reader uses it to tag Cleared.
        let (cancel_ctx_tx, cancel_ctx_rx) = watch::channel(String::new());
        // clear watch: sent locally to shield against the race condition window.
        let (clear_tx, mut clear_rx) = watch::channel(false);
        self.conn_msg_tx = Some(msg_tx);
        self.audio_rx = Some(audio_rx);
        self.context_id_tx = Some(ctx_tx);
        self.cancel_ctx_tx = Some(cancel_ctx_tx);
        self.clear_tx = Some(clear_tx);

        // Reader task: binary audio + JSON metadata
        tokio::spawn(async move {
            let mut is_clearing = false;
            while let Some(msg) = incoming_rx.recv().await {
                if clear_rx.has_changed().unwrap_or(false) {
                    is_clearing = *clear_rx.borrow_and_update();
                }

                match msg {
                    Message::Binary(pcm) => {
                        if is_clearing {
                            // Drop stale in-flight audio frames from the interrupted turn.
                            continue;
                        }
                        // Snapshot live context_id — always correct for in-flight audio.
                        let ctx = ctx_rx.borrow().clone();
                        let _ = audio_tx
                            .send(TtsAudioChunk {
                                pcm: pcm.to_vec(),
                                is_final: false,
                                context_id: ctx,
                            })
                            .await;
                    }
                    Message::Text(text) => {
                        let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) else {
                            warn!("[deepgram-ws] Parse error: {}", text);
                            continue;
                        };
                        let msg_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

                        match msg_type {
                            "Flushed" => {
                                // Normal end-of-turn: all audio for this context is sent.
                                // Use the live watch — Flush/Flushed are in-order with Send.
                                let ctx = ctx_rx.borrow().clone();
                                let _ = audio_tx
                                    .send(TtsAudioChunk {
                                        pcm: vec![],
                                        is_final: true,
                                        context_id: ctx,
                                    })
                                    .await;
                            }
                            "Cleared" => {
                                is_clearing = false;
                                // Barge-in acknowledged. No Flushed follows a Clear.
                                // By the time this arrives, ctx_rx may already hold the *new*
                                // turn's context_id. Use cancel_ctx_rx which was snapshotted
                                // at cancel()-call time — this gives us the stale (old) id
                                // that the reactor will discard as expected.
                                let cancel_ctx = cancel_ctx_rx.borrow().clone();
                                let _ = audio_tx
                                    .send(TtsAudioChunk {
                                        pcm: vec![],
                                        is_final: true,
                                        context_id: cancel_ctx,
                                    })
                                    .await;
                            }
                            "Metadata" => {}
                            "Warning" => {
                                warn!(
                                    "[deepgram-ws] Warning: {}",
                                    event
                                        .get("description")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                );
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            // incoming_rx closed — check if the managed connection died
            if let WsStatus::Disconnected { reason } = status_rx.borrow().clone() {
                if reason != "connection closed by server" {
                    warn!("[deepgram-ws] Connection lost: {}", reason);
                }
            }
        });

        info!(
            "[deepgram-ws] Connected (voice={}, rate={})",
            self.voice, self.output_sample_rate
        );
        Ok(())
    }

    async fn send_text(&mut self, text: &str, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        // Mirror context_id into the reader so it can tag audio frames + Flushed.
        if let Some(ctx_tx) = &self.context_id_tx {
            let _ = ctx_tx.send(context_id.to_string());
        }
        let msg = json!({"type": "Speak", "text": text});
        let _ = tx.send(Message::Text(msg.to_string().into()));
    }

    async fn flush(&mut self, _context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        let _ = tx.send(Message::Text(r#"{"type":"Flush"}"#.to_string().into()));
    }

    async fn cancel(&mut self, context_id: &str) {
        let Some(tx) = &self.conn_msg_tx else { return };
        // Snapshot the about-to-be-cancelled context_id *before* sending Clear.
        // The Cleared response may arrive after send_text() has already updated
        // context_id_tx to the new turn; cancel_ctx_tx gives the reader the
        // correct stale id so it tags Cleared correctly for the reactor to drop.
        if let Some(cancel_tx) = &self.cancel_ctx_tx {
            let _ = cancel_tx.send(context_id.to_string());
        }
        // Immediately instruct the reader thread to drop incoming binary frames
        // to prevent stale frames from the interrupted turn bleeding into the next
        // turn's context_id.
        if let Some(clear_tx) = &self.clear_tx {
            let _ = clear_tx.send(true);
        }
        let _ = tx.send(Message::Text(r#"{"type":"Clear"}"#.to_string().into()));
    }

    async fn close(&mut self) {
        if let Some(tx) = &self.conn_msg_tx {
            let _ = tx.send(Message::Text(r#"{"type":"Close"}"#.to_string().into()));
        }
        self.conn_msg_tx = None;
    }

    fn take_audio_rx(&mut self) -> Option<mpsc::Receiver<TtsAudioChunk>> {
        self.audio_rx.take()
    }
}
