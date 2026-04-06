//! Transport error types.

/// Errors that can occur during transport operations.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Failed to send data to the remote end.
    #[error("send failed: {0}")]
    SendFailed(String),

    /// The transport connection is closed.
    #[error("transport closed")]
    Closed,

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Generic transport error.
    #[error("transport error: {0}")]
    Other(String),
}
