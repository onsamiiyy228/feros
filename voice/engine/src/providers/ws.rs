//! Managed WebSocket connection with keepalive, auto-reconnect, and error signaling.
//!
//! # Architecture
//!
//! `ManagedWsConnection` handles the common WS lifecycle concerns that all
//! STT/TTS WS providers share:
//!
//! - TCP+TLS handshake
//! - Keepalive (configurable per-vendor)
//! - Auto-reconnect on connection drop
//! - Connection status signaling
//!
//! Providers remain responsible only for **protocol-specific message parsing**
//! (reader side) and **message formatting** (writer side).
//!
//! ```text
//! ┌────────────────────────────────────────────────────┐
//! │  ManagedWsConnection                               │
//! │                                                    │
//! │  msg_tx ──► [connection loop] ──► WS write half    │
//! │                  │   keepalive                     │
//! │                  │                                 │
//! │  WS read half ──►│──► incoming_tx                  │
//! │                                                    │
//! │  On error:                                         │
//! │    1. Update status_tx → Reconnecting              │
//! │    2. Backoff delay                                │
//! │    3. Re-call build_request()                      │
//! │    4. Resume with new WS                           │
//! └────────────────────────────────────────────────────┘
//!
//!  Provider code:
//!    - Sends messages via msg_tx (text or binary)
//!    - Reads raw messages from incoming_rx
//!    - Watches connection status via status_rx
//! ```
//!
//! # Usage
//!
//! ```ignore
//! let conn = ManagedWsConnection::connect(
//!     || build_ws_request("wss://api.vendor.com/ws", &api_key),
//!     WsConfig {
//!         keepalive: WsKeepalive::TextMessage {
//!             interval: Duration::from_secs(10),
//!             message: r#"{"text":""}"#.to_string(),
//!         },
//!         max_reconnect_attempts: 3,
//!         reconnect_delay: Duration::from_secs(1),
//!         max_total_reconnect_rounds: 5,
//!     },
//! ).await?;
//!
//! // Send messages
//! conn.msg_tx.send(Message::Text("hello".into()));
//!
//! // Receive messages (spawn your own parser)
//! while let Some(msg) = conn.incoming_rx.recv().await { ... }
//!
//! // Check health
//! assert!(conn.is_connected());
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{http::Request, Message};
use tracing::{info, warn};

// ── Configuration ───────────────────────────────────────────────

/// Keepalive strategy for a managed WS connection.
#[derive(Clone, Debug)]
pub enum WsKeepalive {
    /// WebSocket-level Ping frame (empty payload).
    WsPing { interval: Duration },
    /// Application-level text message (e.g. `{"type":"KeepAlive"}`).
    TextMessage { interval: Duration, message: String },
    /// Application-level binary message (e.g. silent audio).
    BinaryMessage {
        interval: Duration,
        payload: Vec<u8>,
    },
    /// No keepalive (provider handles it, or not needed).
    None,
}

impl WsKeepalive {
    fn interval(&self) -> Option<Duration> {
        match self {
            Self::WsPing { interval } => Some(*interval),
            Self::TextMessage { interval, .. } => Some(*interval),
            Self::BinaryMessage { interval, .. } => Some(*interval),
            Self::None => None,
        }
    }

    fn message(&self) -> Option<Message> {
        match self {
            Self::WsPing { .. } => Some(Message::Ping(vec![].into())),
            Self::TextMessage { message, .. } => Some(Message::Text(message.clone().into())),
            Self::BinaryMessage { payload, .. } => Some(Message::Binary(payload.clone().into())),
            Self::None => None,
        }
    }
}

/// Configuration for a managed WebSocket connection.
#[derive(Clone, Debug)]
pub struct WsConfig {
    pub keepalive: WsKeepalive,
    /// Maximum number of reconnection attempts per round (0 = no reconnect).
    pub max_reconnect_attempts: u32,
    /// Delay between reconnection attempts.
    pub reconnect_delay: Duration,
    /// Maximum total reconnect rounds before giving up permanently.
    ///
    /// Each "round" is a full cycle of `max_reconnect_attempts`. This prevents
    /// an infinite loop when a server accepts connections but immediately drops
    /// them: without this cap, the outer loop would restart the inner retry
    /// loop indefinitely.
    pub max_total_reconnect_rounds: u32,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            keepalive: WsKeepalive::WsPing {
                interval: Duration::from_secs(30),
            },
            max_reconnect_attempts: 3,
            reconnect_delay: Duration::from_secs(1),
            max_total_reconnect_rounds: 5,
        }
    }
}

// ── Connection Status ───────────────────────────────────────────

/// Connection lifecycle status, observable via `watch::Receiver`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsStatus {
    /// Connection is alive and healthy.
    Connected,
    /// Connection was lost; attempting reconnect #`attempt`.
    Reconnecting { attempt: u32 },
    /// Connection was lost and all reconnect attempts exhausted.
    Disconnected { reason: String },
}

// ── Managed Connection ──────────────────────────────────────────

/// A managed WebSocket connection with keepalive and auto-reconnect.
///
/// Created via [`ManagedWsConnection::connect()`].  The caller gets:
/// - `msg_tx` — send outgoing WS messages (text or binary)
/// - `incoming_rx` — receive raw incoming WS messages
/// - `status_rx` — watch connection lifecycle events
pub struct ManagedWsConnection {
    /// Send outgoing WS messages.
    pub msg_tx: mpsc::UnboundedSender<Message>,
    /// Receive raw incoming WS messages (un-parsed).
    pub incoming_rx: mpsc::Receiver<Message>,
    /// Watch connection lifecycle status.
    pub status_rx: watch::Receiver<WsStatus>,
    /// Shared connected flag (cheaply cloneable).
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl ManagedWsConnection {
    /// Connect to a WebSocket server and start the managed connection loop.
    ///
    /// `build_request` is called on each connect/reconnect attempt.  It should
    /// return a fresh `Request` with the target URL and any auth headers.
    ///
    /// Returns `Err` if the initial connection fails.
    pub async fn connect<F>(
        build_request: F,
        config: WsConfig,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn() -> Result<Request<()>, Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        // Verify the initial connection works
        let req = build_request()?;
        let (ws_stream, _) = tokio_tungstenite::connect_async(req).await?;

        let (msg_tx, msg_rx) = mpsc::unbounded_channel::<Message>();
        let (incoming_tx, incoming_rx) = mpsc::channel::<Message>(64);
        let (status_tx, status_rx) = watch::channel(WsStatus::Connected);
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let connected_clone = connected.clone();

        tokio::spawn(Self::managed_loop(
            build_request,
            config,
            ws_stream,
            msg_rx,
            incoming_tx,
            status_tx,
            connected_clone,
        ));

        Ok(Self {
            msg_tx,
            incoming_rx,
            status_rx,
            connected,
        })
    }

    /// Check if the connection is currently alive.
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The main managed connection loop with reconnect support.
    ///
    /// Accepts the initial WS stream; on error, builds a new one via
    /// `build_request()`.  Channels (`msg_rx`, `incoming_tx`) survive
    /// across reconnects — providers see a seamless stream.
    async fn managed_loop<F, S>(
        build_request: F,
        config: WsConfig,
        initial_ws: S,
        mut msg_rx: mpsc::UnboundedReceiver<Message>,
        incoming_tx: mpsc::Sender<Message>,
        status_tx: watch::Sender<WsStatus>,
        connected: Arc<std::sync::atomic::AtomicBool>,
    ) where
        F: Fn() -> Result<Request<()>, Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
        S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
            + Unpin
            + Send,
    {
        // Run the initial connection
        let (mut write, mut read) = initial_ws.split();
        let mut error_reason =
            Self::run_connection_impl(&config, &mut write, &mut read, &mut msg_rx, &incoming_tx)
                .await;

        // Intentional shutdown (sender dropped) — don't reconnect
        if error_reason.contains("sender dropped") || error_reason.contains("receiver dropped") {
            connected.store(false, std::sync::atomic::Ordering::Relaxed);
            let _ = status_tx.send(WsStatus::Disconnected {
                reason: error_reason,
            });
            return;
        }

        // Initial connection lost — enter reconnect loop.
        // Track total rounds to prevent infinite reconnect cycles when the
        // server accepts connections but immediately drops them.
        let mut total_rounds: u32 = 0;
        loop {
            connected.store(false, std::sync::atomic::Ordering::Relaxed);
            if error_reason == "connection closed by server" {
                tracing::debug!("[managed-ws] Connection lost: {}", error_reason);
            } else {
                warn!("[managed-ws] Connection lost: {}", error_reason);
            }

            total_rounds += 1;
            if config.max_reconnect_attempts == 0 {
                let _ = status_tx.send(WsStatus::Disconnected {
                    reason: error_reason,
                });
                return;
            }
            if total_rounds > config.max_total_reconnect_rounds {
                let _ = status_tx.send(WsStatus::Disconnected {
                    reason: format!(
                        "exhausted {} total reconnect rounds after: {}",
                        config.max_total_reconnect_rounds, error_reason
                    ),
                });
                return;
            }

            // Reconnect with retries
            let mut reconnected = false;
            for attempt in 1..=config.max_reconnect_attempts {
                let _ = status_tx.send(WsStatus::Reconnecting { attempt });
                info!(
                    "[managed-ws] Reconnecting (round {}/{}, attempt {}/{})",
                    total_rounds,
                    config.max_total_reconnect_rounds,
                    attempt,
                    config.max_reconnect_attempts
                );
                tokio::time::sleep(config.reconnect_delay).await;

                let req = match build_request() {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("[managed-ws] Request build error: {}", e);
                        continue;
                    }
                };

                match tokio_tungstenite::connect_async(req).await {
                    Ok((ws_stream, _)) => {
                        let (new_write, new_read) = ws_stream.split();
                        info!("[managed-ws] Reconnected successfully");
                        let _ = status_tx.send(WsStatus::Connected);
                        connected.store(true, std::sync::atomic::Ordering::Relaxed);

                        // Run the new connection
                        error_reason = Self::run_connection_erased(
                            &config,
                            new_write,
                            new_read,
                            &mut msg_rx,
                            &incoming_tx,
                        )
                        .await;

                        // Check for intentional shutdown
                        if error_reason.contains("sender dropped")
                            || error_reason.contains("receiver dropped")
                        {
                            connected.store(false, std::sync::atomic::Ordering::Relaxed);
                            let _ = status_tx.send(WsStatus::Disconnected {
                                reason: error_reason,
                            });
                            return;
                        }

                        reconnected = true;
                        break; // Break inner retry loop, outer loop will handle next error
                    }
                    Err(e) => {
                        warn!(
                            "[managed-ws] Reconnect attempt {}/{} failed: {}",
                            attempt, config.max_reconnect_attempts, e
                        );
                    }
                }
            }

            if !reconnected {
                let _ = status_tx.send(WsStatus::Disconnected {
                    reason: format!(
                        "exhausted {} reconnect attempts after: {}",
                        config.max_reconnect_attempts, error_reason
                    ),
                });
                return;
            }

            // error_reason was updated by the reconnected session — loop again
        }
    }

    /// Run the read/write/keepalive loop on an active connection (generic version).
    ///
    /// Returns a reason string when the connection drops.
    async fn run_connection_impl<W, R>(
        config: &WsConfig,
        write: &mut W,
        read: &mut R,
        msg_rx: &mut mpsc::UnboundedReceiver<Message>,
        incoming_tx: &mpsc::Sender<Message>,
    ) -> String
    where
        W: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send,
        R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send,
    {
        // Build optional keepalive interval
        let keepalive_interval = config.keepalive.interval();
        let keepalive_msg = config.keepalive.message();

        let mut interval = keepalive_interval.map(tokio::time::interval);
        if let Some(ref mut i) = interval {
            i.tick().await; // skip immediate tick
        }

        loop {
            tokio::select! {
                // Outgoing messages
                msg = msg_rx.recv() => {
                    match msg {
                        Some(m) => {
                            if write.send(m).await.is_err() {
                                return "write error".to_string();
                            }
                        }
                        None => {
                            // msg_tx dropped — intentional shutdown
                            return "sender dropped (shutdown)".to_string();
                        }
                    }
                }

                // Keepalive tick (only if configured)
                _ = async {
                    if let Some(ref mut i) = interval {
                        i.tick().await
                    } else {
                        std::future::pending::<tokio::time::Instant>().await
                    }
                } => {
                    if let Some(ref ka) = keepalive_msg {
                        if write.send(ka.clone()).await.is_err() {
                            return "keepalive write error".to_string();
                        }
                    }
                }

                // Incoming messages
                msg = read.next() => {
                    match msg {
                        Some(Ok(m)) => {
                            // Filter out Pong frames — not useful for providers
                            if matches!(m, Message::Pong(_)) {
                                continue;
                            }
                            if incoming_tx.send(m).await.is_err() {
                                return "incoming channel closed (receiver dropped)".to_string();
                            }
                        }
                        Some(Err(e)) => {
                            return format!("read error: {}", e);
                        }
                        None => {
                            return "connection closed by server".to_string();
                        }
                    }
                }
            }
        }
    }

    /// Type-erased version of `run_connection_impl` that takes ownership
    /// of the write/read halves (needed for reconnect, where the concrete
    /// type differs from the initial connection).
    async fn run_connection_erased<W, R>(
        config: &WsConfig,
        mut write: W,
        mut read: R,
        msg_rx: &mut mpsc::UnboundedReceiver<Message>,
        incoming_tx: &mpsc::Sender<Message>,
    ) -> String
    where
        W: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send,
        R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send,
    {
        Self::run_connection_impl(config, &mut write, &mut read, msg_rx, incoming_tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_ws_ping_produces_ping_message() {
        let ka = WsKeepalive::WsPing {
            interval: Duration::from_secs(30),
        };
        assert!(matches!(ka.message(), Some(Message::Ping(_))));
        assert_eq!(ka.interval(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn keepalive_text_produces_text_message() {
        let ka = WsKeepalive::TextMessage {
            interval: Duration::from_secs(10),
            message: r#"{"type":"KeepAlive"}"#.to_string(),
        };
        if let Some(Message::Text(t)) = ka.message() {
            assert!(t.contains("KeepAlive"));
        } else {
            panic!("expected Text message");
        }
    }

    #[test]
    fn keepalive_none_produces_no_message() {
        let ka = WsKeepalive::None;
        assert!(ka.message().is_none());
        assert!(ka.interval().is_none());
    }

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = WsConfig::default();
        assert_eq!(cfg.max_reconnect_attempts, 3);
        assert_eq!(cfg.reconnect_delay, Duration::from_secs(1));
        assert!(matches!(cfg.keepalive, WsKeepalive::WsPing { .. }));
    }

    #[test]
    fn ws_status_eq() {
        assert_eq!(WsStatus::Connected, WsStatus::Connected);
        assert_ne!(WsStatus::Connected, WsStatus::Reconnecting { attempt: 1 });
    }
}
