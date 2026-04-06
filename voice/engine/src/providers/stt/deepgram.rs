//! Deepgram STT provider — streaming transcription over their Nova WebSocket API.
//!
//! # Protocol
//!
//! Connection: `wss://api.deepgram.com/v1/listen?...`
//! Auth:       `Authorization: Token <api_key>` header
//! Audio:      Binary WS frames (raw PCM-16 LE at 16 kHz)
//! Finalize:   send `{"type":"Finalize"}` → wait for response with `from_finalize: true`
//! KeepAlive:  `{"type":"KeepAlive"}` every 5 s (handled by ManagedWsConnection)
//!
//! # Ref
//! <https://developers.deepgram.com/docs/getting-started-with-live-streaming-audio>

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tracing::{info, warn};

use crate::providers::ws::{ManagedWsConnection, WsConfig, WsKeepalive, WsStatus};
use crate::types::SttEvent;

use super::SttProvider;

// ── Response types ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DeepgramResult {
    /// `"Results"` for transcript events, `"Metadata"` for session info, etc.
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    channel: Option<DeepgramChannel>,
    /// Set to `true` when this result was triggered by a `Finalize` request.
    #[serde(default)]
    from_finalize: bool,
    /// Set to `true` for final (non-interim) results.
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    speech_final: bool,
}

#[derive(Debug, Deserialize)]
struct DeepgramChannel {
    alternatives: Vec<DeepgramAlternative>,
}

#[derive(Debug, Deserialize)]
struct DeepgramAlternative {
    transcript: String,
}

// ── DeepgramSttProvider ──────────────────────────────────────────

/// Streaming STT via Deepgram's Nova WebSocket API.
pub struct DeepgramSttProvider {
    api_key: SecretString,
    model: String,
    language: String,
    /// Sends outgoing WS messages (binary audio + text control) to the managed connection.
    conn_msg_tx: Option<mpsc::UnboundedSender<Message>>,
    result_rx: Option<mpsc::Receiver<SttEvent>>,
}

impl DeepgramSttProvider {
    pub fn new(api_key: &str, model: &str, language: &str) -> Self {
        Self {
            api_key: SecretString::from(api_key.to_string()),
            model: if model.is_empty() {
                "nova-3-general".to_string()
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
            "wss://api.deepgram.com/v1/listen\
             ?model={model}\
             &language={lang}\
             &encoding=linear16\
             &sample_rate=16000\
             &interim_results=true\
             &punctuate=true\
             &smart_format=true",
            model = self.model,
            lang = self.language,
        )
    }
}

#[async_trait]
impl SttProvider for DeepgramSttProvider {
    fn provider_name(&self) -> &str {
        "deepgram"
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
                keepalive: WsKeepalive::TextMessage {
                    interval: std::time::Duration::from_secs(5),
                    message: r#"{"type":"KeepAlive"}"#.to_string(),
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

        // Reader task: parse Deepgram transcript events.
        //
        // Deepgram sends multiple `is_final` segments per utterance, followed
        // by a single `speech_final` (or `from_finalize`) that marks the end
        // of the complete utterance.  We accumulate `is_final` segment texts
        // and emit the concatenated result only at the utterance boundary to
        // avoid sending duplicate / partial transcripts as finals.
        tokio::spawn(async move {
            let mut sent_first_text = false;
            let mut accumulated_segments: Vec<String> = Vec::new();

            while let Some(msg) = incoming_rx.recv().await {
                if let Message::Text(text) = msg {
                    match serde_json::from_str::<DeepgramResult>(&text) {
                        Ok(res) if res.msg_type == "Results" => {
                            let transcript = res
                                .channel
                                .as_ref()
                                .and_then(|c| c.alternatives.first())
                                .map(|a| a.transcript.trim().to_string())
                                .unwrap_or_default();

                            if transcript.is_empty() && !res.speech_final && !res.from_finalize {
                                continue;
                            }

                            if !transcript.is_empty() && !sent_first_text {
                                sent_first_text = true;
                                let _ = result_tx.send(SttEvent::FirstTextReceived).await;
                            }

                            if res.is_final && !transcript.is_empty() {
                                accumulated_segments.push(transcript.clone());
                            }

                            // Utterance boundary: emit the full accumulated text.
                            // We MUST emit even if empty if it was triggered by a realize() call,
                            // so the Reactor's state machine knows the request completed
                            // (unblocking BargeInWordCount or cancelling WaitingForTranscript timers).
                            if res.speech_final || res.from_finalize {
                                // If this message itself is_final, it's already
                                // pushed above; if not, add it now.
                                if !res.is_final && !transcript.is_empty() {
                                    accumulated_segments.push(transcript);
                                }
                                let full = accumulated_segments.join(" ").trim().to_string();
                                accumulated_segments.clear();
                                sent_first_text = false;

                                if full.is_empty() && !res.from_finalize {
                                    continue; // Ignore spontaneous empty endpointing
                                }

                                let _ = result_tx.send(SttEvent::Transcript(full)).await;
                            } else if !res.is_final && !transcript.is_empty() {
                                // Interim (non-final) result — useful for live UI
                                let _ = result_tx
                                    .send(SttEvent::PartialTranscript(transcript))
                                    .await;
                            }
                        }
                        Ok(_) => {} // Metadata, SpeechStarted, etc. — ignore
                        Err(e) => {
                            warn!("[deepgram] Failed to parse message: {} — raw: {}", e, text)
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
                        "deepgram WS disconnected: {}",
                        reason
                    )))
                    .await;
            }
        });

        info!(
            "[deepgram] Connected (model={}, lang={})",
            self.model, self.language
        );
        Ok(())
    }

    fn feed_audio(&self, audio: &[u8]) {
        if let Some(tx) = &self.conn_msg_tx {
            let _ = tx.send(Message::Binary(audio.to_vec().into()));
        }
    }

    fn finalize(&self) {
        if let Some(tx) = &self.conn_msg_tx {
            let _ = tx.send(Message::Text(r#"{"type":"Finalize"}"#.to_string().into()));
        }
    }

    fn close(&self) {
        if let Some(tx) = &self.conn_msg_tx {
            let _ = tx.send(Message::Text(
                r#"{"type":"CloseStream"}"#.to_string().into(),
            ));
        }
    }

    fn take_result_rx(&mut self) -> Option<mpsc::Receiver<SttEvent>> {
        self.result_rx.take()
    }
}
