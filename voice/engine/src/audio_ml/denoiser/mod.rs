//! Denoiser implementations — pluggable noise suppression backends.
//!
//! Currently supports:
//!   - **DTLN** – two-stage LSTM in the frequency domain (16 kHz native)
//!   - **DeepFilterNet3** – three-model encoder/decoder with deep filtering (48 kHz native)
//!   - **RNNoise** – lightweight RNN-based noise suppression (48 kHz native)

pub mod dfn3;
pub mod dtln;
pub mod rnnoise;

/// Which denoiser backend to use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DenoiserBackend {
    /// Two-stage LSTM denoiser (DTLN). Lightweight, low latency, 16 kHz native.
    Dtln,
    /// DeepFilterNet3 — higher quality, 48 kHz native (resampled internally).
    DeepFilterNet3,
    /// RNNoise — lightweight RNN-based, 48 kHz native (resampled internally).
    #[default]
    RNNoise,
}
