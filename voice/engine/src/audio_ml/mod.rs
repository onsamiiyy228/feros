//! ONNX inference implementations.
//!
//! Contains the raw ONNX Runtime logic for each local ML model used in the
//! voice pipeline. These are pure processing units — they know nothing about
//! actors, reactors, or the session lifecycle.
//!
//! The `stages/` module provides thin reactor-aware wrappers around these types.

pub mod denoiser;
pub mod smart_turn;
pub mod vad;
