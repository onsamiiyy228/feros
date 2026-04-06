//! Pipeline stage traits and shared types.
//!
//! The Reactor drives two kinds of stages:
//! - [`SyncStage`]: Called inline on every audio frame. Zero async overhead.
//!   Used for: Denoiser, VAD, SmartTurn.
//! - [`AsyncStage`]: Owns an active I/O handle (WebSocket / HTTP stream).
//!   Polled in the central `select!` loop.
//!   Used for: STT, LLM, TTS.

pub mod denoiser;
pub mod llm;
pub mod smart_turn;
pub mod stt;
pub mod tts;
pub mod vad;
