//! RNNoise denoiser — lightweight RNN-based noise suppression via nnnoiseless.
//!
//! RNNoise operates at 48 kHz natively with a frame size of 480 samples.
//! Since our voice pipeline runs at 16 kHz, this module:
//!   1. Buffers incoming 16 kHz PCM
//!   2. Resamples 16 kHz → 48 kHz via soxr (sinc interpolation)
//!   3. Feeds 480-sample frames through the RNN
//!   4. Downsamples 48 kHz → 16 kHz via soxr
//!   5. Returns cleaned 16 kHz PCM bytes
//!
//! Compared to DeepFilterNet3, RNNoise is significantly lighter (single GRU pass
//! vs. 3-model encoder/decoder pipeline) but lower quality. Good for low-latency
//! scenarios where compute budget is tight.

use nnnoiseless::DenoiseState;
use soxr::SoxrStreamResampler;
use tracing::{info, warn};

/// Native sample rate for RNNoise (48 kHz).
const RNNOISE_SR: usize = 48_000;

/// Frame size for RNNoise (480 samples at 48 kHz = 10 ms).
const FRAME_SIZE: usize = DenoiseState::FRAME_SIZE; // 480

/// Pipeline sample rate (16 kHz).
const PIPELINE_SR: usize = crate::utils::SAMPLE_RATE as usize;

pub struct RnnoiseDenoiser {
    state: Box<DenoiseState<'static>>,

    // Resampling via soxr (high-quality sinc interpolation)
    up_resampler: Option<SoxrStreamResampler>, // 16k → 48k
    down_resampler: Option<SoxrStreamResampler>, // 48k → 16k

    // Accumulation buffers for soxr's bursty output
    up_buf: Vec<f32>,  // accumulated upsampled f32 samples (48kHz)
    down_buf: Vec<u8>, // accumulated enhanced PCM16 bytes for downsampling
    out_buf: Vec<u8>,  // accumulated downsampled output bytes (16kHz)
}

impl Default for RnnoiseDenoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl RnnoiseDenoiser {
    pub fn new() -> Self {
        let up_resampler = SoxrStreamResampler::new(PIPELINE_SR as u32, RNNOISE_SR as u32).ok();
        let down_resampler = SoxrStreamResampler::new(RNNOISE_SR as u32, PIPELINE_SR as u32).ok();

        if up_resampler.is_none() || down_resampler.is_none() {
            warn!("[rnnoise] Failed to create soxr resamplers, resampling will be disabled");
        }

        Self {
            state: DenoiseState::new(),
            up_resampler,
            down_resampler,
            up_buf: Vec::new(),
            down_buf: Vec::new(),
            out_buf: Vec::new(),
        }
    }

    /// Initialize the denoiser. RNNoise has no external model files to load,
    /// so this is a no-op that always succeeds.
    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "[rnnoise] Initialized via nnnoiseless (sr={}, frame_size={})",
            RNNOISE_SR, FRAME_SIZE
        );
        Ok(())
    }

    /// Process a 16 kHz PCM-16 LE audio frame.
    pub fn denoise_frame(&mut self, pcm: &[u8]) -> Vec<u8> {
        let n_samples = pcm.len() / 2;
        if n_samples == 0 {
            return pcm.to_vec();
        }

        // ── Step 1: Upsample 16 kHz → 48 kHz via soxr, accumulate into up_buf ──
        let upsampled = match self.up_resampler.as_mut() {
            Some(r) => r.process(pcm),
            None => return pcm.to_vec(),
        };
        // Convert upsampled PCM16 → f32 (i16 range, as nnnoiseless expects)
        for i in 0..(upsampled.len() / 2) {
            let s = i16::from_le_bytes([upsampled[i * 2], upsampled[i * 2 + 1]]) as f32;
            self.up_buf.push(s); // nnnoiseless expects i16-range f32
        }

        // ── Step 2: Process complete FRAME_SIZE chunks from up_buf ────────────
        let mut out_frame = [0.0f32; FRAME_SIZE];
        while self.up_buf.len() >= FRAME_SIZE {
            let chunk: Vec<f32> = self.up_buf.drain(..FRAME_SIZE).collect();
            let _vad_prob = self.state.process_frame(&mut out_frame, &chunk);

            // Convert f32 (i16 range) → PCM16 LE and append to down_buf
            for &s in &out_frame {
                let val = s.clamp(-32768.0, 32767.0) as i16;
                self.down_buf.extend_from_slice(&val.to_le_bytes());
            }
        }

        // ── Step 3: Downsample accumulated enhanced audio 48 kHz → 16 kHz ──
        if !self.down_buf.is_empty() {
            let downsampled = match self.down_resampler.as_mut() {
                Some(r) => r.process(&self.down_buf),
                None => std::mem::take(&mut self.down_buf),
            };
            self.out_buf.extend_from_slice(&downsampled);
            self.down_buf.clear();
        }

        // ── Step 4: Emit exactly n_samples * 2 bytes from out_buf ───────────
        let expected_len = n_samples * 2;
        if self.out_buf.len() >= expected_len {
            self.out_buf.drain(..expected_len).collect()
        } else {
            // Not enough output yet (startup transient) — return silence
            vec![0u8; expected_len]
        }
    }

    pub fn reset(&mut self) {
        self.state = DenoiseState::new();
        self.up_buf.clear();
        self.down_buf.clear();
        self.out_buf.clear();

        // Re-create soxr resamplers to reset their internal state
        self.up_resampler = SoxrStreamResampler::new(PIPELINE_SR as u32, RNNOISE_SR as u32).ok();
        self.down_resampler = SoxrStreamResampler::new(RNNOISE_SR as u32, PIPELINE_SR as u32).ok();
    }
}
