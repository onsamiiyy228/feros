//! Shared data types for the voice engine.
//!
//! This crate holds cross-cutting configuration structs and enums that
//! are needed by multiple crates (`agent-kit`, `voice-trace`, `voice-engine`)
//! without introducing circular dependencies.

mod recording;

pub use recording::{AudioFormat, AudioLayout, RecordingConfig};
