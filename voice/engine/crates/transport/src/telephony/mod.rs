//! Telephony transport — Twilio and Telnyx WebSocket Media Streams.
//!
//! Both providers open a WebSocket to our server carrying base64-encoded
//! G.711 (μ-law/A-law) 8 kHz audio in JSON frames. This module provides
//! the transport implementation, codec utilities, and call control.
//!
//! # Supported Providers
//!
//! - **Twilio**: Media Streams WebSocket protocol (μ-law 8 kHz)
//! - **Telnyx**: WebSocket media protocol (μ-law or A-law 8 kHz)
//!
//! # Architecture
//!
//! The telephony transport follows the same pattern as the WebSocket and
//! WebRTC transports: it produces a [`TransportHandle`](crate::TransportHandle)
//! that the [`VoiceSession`] consumes transport-agnostically.
//!
//! Provider-specific logic (JSON framing, call control) lives in the
//! `providers/` submodule. Adding a new provider means implementing
//! [`TelephonyProviderImpl`](provider::TelephonyProviderImpl) and registering
//! it in the factory.
//!
//! Users provide their own API keys and phone numbers — no credentials
//! are stored beyond the session lifetime.

pub mod codec;
pub mod config;
pub mod providers;
mod transport;

pub use config::{TelephonyConfig, TelephonyCredentials, TelephonyEncoding, TelephonyProviderKind};
pub use transport::TelephonyTransport;
