//! Builtin STT provider — WebSocket client to the speech-inference STT server.
//!
//! # Protocol
//!
//! Connection: `ws://<host>/v1/listen?language=<lang>`
//! Audio:      Binary WS frames (raw PCM-16 LE at 16 kHz)
//! Finalize:   send `{"finalize": {}}` → server returns transcript
//! Close:      send `{"close": {}}`
//! Results:    JSON `{"transcript": {"is_final": true, "text": "...", ...}}`
//!
//! Ref: proto/stt.proto (serialized as protobuf canonical JSON)

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

use crate::types::SttEvent;

use super::SttProvider;

// ── Response types ───────────────────────────────────────────────
// Hand-written serde types that mirror proto/stt.proto.
// The canonical types live in `crate::proto::stt` (prost + pbjson codegen).
// If you add fields to stt.proto, update these structs to match.

/// Server → client response envelope.
#[derive(Debug, Deserialize)]
struct SttResponseMsg {
    transcript: Option<TranscriptResult>,
}

/// A transcript result from the STT server.
#[derive(Debug, Deserialize)]
struct TranscriptResult {
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    text: String,
}

// ── BuiltinSttProvider ───────────────────────────────────────────

/// Streaming STT via the speech-inference WebSocket server.
///
/// Lifecycle: `connect()` → `feed_audio()` (continuous) → `finalize()` → `close()`.
pub struct BuiltinSttProvider {
    ws_url: String,
    /// Sends raw binary audio frames to the writer task.
    audio_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Sends JSON control messages to the writer task.
    control_tx: Option<mpsc::UnboundedSender<String>>,
    /// Receives STT events from the reader task.
    result_rx: Option<mpsc::Receiver<SttEvent>>,
}

impl BuiltinSttProvider {
    pub fn new(base_url: &str, language: &str) -> Self {
        let ws_base = base_url
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        let ws_url = format!("{}/v1/listen?language={}", ws_base, language);

        Self {
            ws_url,
            audio_tx: None,
            control_tx: None,
            result_rx: None,
        }
    }
}

#[async_trait]
impl SttProvider for BuiltinSttProvider {
    fn provider_name(&self) -> &str {
        "builtin"
    }

    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (control_tx, mut control_rx) = mpsc::unbounded_channel::<String>();
        let (result_tx, result_rx) = mpsc::channel::<SttEvent>(16);

        self.audio_tx = Some(audio_tx);
        self.control_tx = Some(control_tx);
        self.result_rx = Some(result_rx);

        // Writer task: binary audio + JSON control messages
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(audio) = audio_rx.recv() => {
                        if write.send(Message::Binary(audio.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(json) = control_rx.recv() => {
                        if write.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    else => break,
                }
            }
        });

        // Reader task: parse transcript JSON
        tokio::spawn(async move {
            let mut sent_first_text = false;
            while let Some(Ok(msg)) = read.next().await {
                if let Message::Text(text) = msg {
                    match serde_json::from_str::<SttResponseMsg>(&text) {
                        Ok(resp) => {
                            if let Some(tr) = resp.transcript {
                                let transcript = tr.text.trim().to_string();

                                if !sent_first_text && !transcript.is_empty() {
                                    sent_first_text = true;
                                    let _ = result_tx.send(SttEvent::FirstTextReceived).await;
                                }

                                if tr.is_final && !transcript.is_empty() {
                                    sent_first_text = false;
                                    let _ = result_tx.send(SttEvent::Transcript(transcript)).await;
                                }
                            }
                        }
                        Err(e) => warn!("[builtin-stt] Invalid JSON: {} — raw: {}", e, text),
                    }
                }
            }
        });

        info!("[builtin-stt] Connected to {}", self.ws_url);
        Ok(())
    }

    fn feed_audio(&self, audio: &[u8]) {
        if let Some(tx) = &self.audio_tx {
            let _ = tx.send(audio.to_vec());
        }
    }

    fn finalize(&self) {
        if let Some(tx) = &self.control_tx {
            let _ = tx.send(r#"{"finalize": {}}"#.to_string());
        }
    }

    fn close(&self) {
        if let Some(tx) = &self.control_tx {
            let _ = tx.send(r#"{"close": {}}"#.to_string());
        }
    }

    fn take_result_rx(&mut self) -> Option<mpsc::Receiver<SttEvent>> {
        self.result_rx.take()
    }
}
