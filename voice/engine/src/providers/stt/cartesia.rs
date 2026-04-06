//! Cartesia STT provider — streaming WebSocket transcription via ink‑whisper.
//!
//! # Protocol
//!
//! Connection: `wss://api.cartesia.ai/stt/websocket?model=ink-whisper&language=en&encoding=pcm_s16le&sample_rate=16000`
//! Auth:       `X-API-Key: <api_key>` + `Cartesia-Version: 2025-04-16` headers
//! Audio:      Binary WS frames (raw PCM-16 LE at 16 kHz)
//! Finalize:   send text frame `"finalize"` on VAD speech-end
//! Results:    JSON: `{"type":"transcript","text":"...","is_final":true,"language":"en"}`
//! KeepAlive:  Cartesia closes after 3 min inactivity; managed connection sends
//!             silent audio every 30 s to reset the timer.
//!
//! Ref: <https://docs.cartesia.ai/api-reference/stt/stt>

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tracing::{info, warn};

use crate::providers::ws::{ManagedWsConnection, WsConfig, WsKeepalive, WsStatus};
use crate::types::SttEvent;

use super::SttProvider;

/// Cartesia STT WebSocket API version.
const CARTESIA_STT_API_VERSION: &str = "2025-04-16";

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CartesiaTranscriptMsg {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    is_final: bool,
}

// ── CartesiaSttProvider ───────────────────────────────────────────────────────

// 100 ms of silence at 16 kHz mono PCM-16 = 3200 bytes.
// Sent every 30 s to reset Cartesia's 3-min idle timeout.
// Cartesia's server-side VAD ignores all-zero audio, so this does not trigger
// false transcripts.  Confirmed safe per their WS docs.
const SILENCE_CHUNK: &[u8] = &[0u8; 3200];

/// Streaming STT via the Cartesia Live WebSocket API (`ink-whisper` model).
pub struct CartesiaSttProvider {
    api_key: SecretString,
    model: String,
    language: String,
    /// Sends outgoing WS messages (binary audio + text control) to managed connection.
    conn_msg_tx: Option<mpsc::UnboundedSender<Message>>,
    result_rx: Option<mpsc::Receiver<SttEvent>>,
}

impl CartesiaSttProvider {
    pub fn new(api_key: &str, model: &str, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "ink-whisper".to_string()
            } else {
                model.to_string()
            },
            language: if language.is_empty() {
                "en".to_string()
            } else {
                language.to_string()
            },
            conn_msg_tx: None,
            result_rx: None,
        }
    }

    fn ws_url(&self) -> String {
        format!(
            "wss://api.cartesia.ai/stt/websocket\
             ?model={model}\
             &language={lang}\
             &encoding=pcm_s16le\
             &sample_rate=16000\
             &min_volume=0.01\
             &max_silence_duration_secs=5.0",
            model = self.model,
            lang = self.language,
        )
    }
}

#[async_trait]
impl SttProvider for CartesiaSttProvider {
    fn provider_name(&self) -> &str {
        "cartesia"
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let url = self.ws_url();
        let api_key = self.api_key.expose_secret().to_string();

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
                headers.insert("Cartesia-Version", CARTESIA_STT_API_VERSION.parse()
                    .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue|
                        -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?);
                Ok(req)
            },
            WsConfig {
                keepalive: WsKeepalive::BinaryMessage {
                    interval: std::time::Duration::from_secs(30),
                    payload: SILENCE_CHUNK.to_vec(),
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
        self.conn_msg_tx = Some(msg_tx);
        self.result_rx = Some(result_rx);

        // Reader task: parse Cartesia transcript JSON events
        tokio::spawn(async move {
            let mut sent_first_text = false;
            while let Some(msg) = incoming_rx.recv().await {
                if let Message::Text(text) = msg {
                    match serde_json::from_str::<CartesiaTranscriptMsg>(&text) {
                        Ok(msg) if msg.msg_type == "transcript" => {
                            if msg.text.is_empty() && !msg.is_final {
                                continue;
                            }

                            if !msg.text.is_empty() && !sent_first_text {
                                sent_first_text = true;
                                let _ = result_tx.send(SttEvent::FirstTextReceived).await;
                            }

                            if msg.is_final {
                                sent_first_text = false;
                                let _ = result_tx
                                    .send(SttEvent::Transcript(msg.text.trim().to_string()))
                                    .await;
                            } else if !msg.text.is_empty() {
                                let _ = result_tx.send(SttEvent::PartialTranscript(msg.text)).await;
                            }
                        }
                        Ok(msg) if msg.msg_type == "error" => {
                            warn!("[cartesia-stt] Error from server: {:?}", msg);
                        }
                        Ok(_) => {} // other message types — ignore
                        Err(e) => {
                            warn!("[cartesia-stt] Parse error: {} — raw: {}", e, text)
                        }
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
                        "cartesia STT disconnected: {}",
                        reason
                    )))
                    .await;
            }
        });

        info!(
            "[cartesia-stt] Connected (model={}, lang={})",
            self.model, self.language
        );
        Ok(())
    }

    fn feed_audio(&self, audio: &[u8]) {
        if let Some(tx) = &self.conn_msg_tx {
            let _ = tx.send(Message::Binary(audio.to_vec().into()));
        }
    }

    /// Send the text literal `"finalize"` to commit the current utterance.
    fn finalize(&self) {
        if let Some(tx) = &self.conn_msg_tx {
            let _ = tx.send(Message::Text("finalize".to_string().into()));
        }
    }

    fn close(&self) {
        // No explicit close handshake needed — Cartesia's WS has no close frame.
        // Dropping the sender (`conn_msg_tx`) when the provider is dropped
        // causes `ManagedWsConnection` to shut down the WS cleanly.
    }

    fn take_result_rx(&mut self) -> Option<mpsc::Receiver<SttEvent>> {
        self.result_rx.take()
    }
}
