//! DeepFilterNet3 denoiser — three-model encoder/decoder pipeline via tract.
//!
//! The model operates at 48 kHz (hop_size = 480, fft_size = 960).
//! Since our voice pipeline runs at 16 kHz, this module:
//!   1. Buffers incoming 16 kHz PCM
//!   2. Resamples 16 kHz → 48 kHz via soxr (sinc interpolation)
//!   3. Feeds through the 3 models (enc → erb_dec + df_dec)
//!   4. Applies ERB gains and deep-filter coefficients
//!   5. Reconstructs audio via ISTFT + overlap-add
//!   6. Downsamples 48 kHz → 16 kHz via soxr
//!   7. Returns cleaned 16 kHz PCM bytes
//!
//! Inference uses tract's PulsedModel for O(1) per-frame stateful processing.
//! The pulsed transform converts the ONNX model so that:
//!   - GRU (Scan) ops maintain hidden state across calls
//!   - Delay ops buffer past frames for convolutions
//!
//! This gives perfect temporal context at constant compute cost per frame.
//!
//! Model I/O (all f32, pulse=1 per frame):
//!   enc.onnx:     feat_erb [1,1,1,32] + feat_spec [1,2,1,96] → e0-e3, emb, c0, lsnr
//!   erb_dec.onnx: emb [1,1,512] + e3 [1,64,1,8] + e2 [1,64,1,8]
//!                 + e1 [1,64,1,16] + e0 [1,64,1,32] → mask [1,1,1,32]
//!   df_dec.onnx:  emb [1,1,512] + c0 [1,64,1,96] → coefs [1,1,96,10]
//!
//! Cross-checked against official libDF:
//!   - ERB filterbank: libDF/src/lib.rs::erb_fb()
//!   - Window: Vorbis window sin(π/2 * sin²(π*n/N))
//!   - Analysis normalization: wnorm = 1 / (fft_size² / (2 * hop_size))
//!   - ERB features: band_corr → 10*log10 → mean_norm → /40
//!   - Complex features: unit_norm_t → divide by sqrt(running_mag)
//!   - Deep filtering: complex multiply-accumulate over df_order taps
//!   - Model init: tract PulsedModel pattern from libDF/src/tract.rs

use std::path::Path;
use std::sync::Arc;

use num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use soxr::SoxrStreamResampler;
use tracing::{info, warn};
use tract_core::prelude::*;
use tract_onnx::prelude::*;
use tract_onnx::tract_hir::shapefactoid;
use tract_pulse::model::{PulsedModel, PulsedModelExt};

/// Frozen tract state — IS Send (stores FrozenOpState).
/// We unfreeze before each inference call and re-freeze after.
type FrozenState = TypedFrozenSimpleState<TypedModel, Arc<TypedSimplePlan<TypedModel>>>;

// ── Config from DeepFilterNet3 config.ini ───────────────────────────────────
const SR: usize = 48_000;
const FFT_SIZE: usize = 960;
const HOP_SIZE: usize = 480;
const N_FREQS: usize = FFT_SIZE / 2 + 1; // 481
const NB_ERB: usize = 32;
const NB_DF: usize = 96;
const MIN_NB_ERB_FREQS: usize = 2;
const DF_ORDER: usize = 5;
const CONV_LOOKAHEAD: usize = 2;

const CONV_CH: usize = 64;
const NORM_TAU: f32 = 1.0;

// Normalization state initialization values (from libDF MEAN_NORM_INIT / UNIT_NORM_INIT)
const MEAN_NORM_INIT: [f32; 2] = [-60.0, -90.0];
const UNIT_NORM_INIT: [f32; 2] = [0.001, 0.0001];

// Pipeline sample rate
const PIPELINE_SR: usize = crate::utils::SAMPLE_RATE as usize;

// ── ERB filterbank (matches libDF/src/lib.rs::erb_fb exactly) ───────────────
fn erb_fb() -> Vec<usize> {
    let freq_width = SR as f32 / FFT_SIZE as f32;
    let erb_low: f32 = freq2erb(0.0);
    let erb_high: f32 = freq2erb((SR / 2) as f32);
    let step = (erb_high - erb_low) / NB_ERB as f32;
    let min_nb_freqs = MIN_NB_ERB_FREQS as i32;

    let mut erb = vec![0usize; NB_ERB];
    let mut prev_freq: usize = 0;
    let mut freq_over: i32 = 0;

    for i in 1..=NB_ERB {
        let f = erb2freq(erb_low + i as f32 * step);
        let fb = (f / freq_width).round() as usize;
        let mut nb_freqs = fb as i32 - prev_freq as i32 - freq_over;
        if nb_freqs < min_nb_freqs {
            freq_over = min_nb_freqs - nb_freqs;
            nb_freqs = min_nb_freqs;
        } else {
            freq_over = 0;
        }
        erb[i - 1] = nb_freqs as usize;
        prev_freq = fb;
    }
    erb[NB_ERB - 1] += 1; // N_FREQS = fft_size/2 + 1

    let total: usize = erb.iter().sum();
    if total > N_FREQS {
        erb[NB_ERB - 1] -= total - N_FREQS;
    }
    debug_assert_eq!(erb.iter().sum::<usize>(), N_FREQS);
    erb
}

fn freq2erb(freq_hz: f32) -> f32 {
    9.265 * (freq_hz / (24.7 * 9.265)).ln_1p()
}

fn erb2freq(n_erb: f32) -> f32 {
    24.7 * 9.265 * ((n_erb / 9.265).exp() - 1.0)
}

fn calc_norm_alpha(sr: usize, hop_size: usize, tau: f32) -> f32 {
    let dt = hop_size as f32 / sr as f32;
    let alpha = (-dt / tau).exp();
    // Match libDF rounding
    let mut a = 1.0f32;
    let mut precision = 3u32;
    while a >= 1.0 {
        a = (alpha * 10f32.powi(precision as i32)).round() / 10f32.powi(precision as i32);
        precision += 1;
    }
    a
}

// ── Tract model initialization (matches libDF/src/tract.rs) ─────────────────

/// Create a plan and frozen initial state from an optimized TypedModel.
fn build_frozen_state(model: TypedModel) -> TractResult<FrozenState> {
    let plan = Arc::new(model.into_runnable()?);
    let state = TypedSimpleState::new(plan)?;
    Ok(state.freeze())
}

/// Initialize pulsed encoder model.
/// Input: feat_erb [1,1,S,32], feat_spec [1,2,S,96]
/// Output: e0-e3, emb, c0, lsnr
fn init_encoder(path: &Path) -> TractResult<FrozenState> {
    let mut m = tract_onnx::onnx()
        .with_ignore_output_shapes(true)
        .model_for_path(path)?;
    let s = m.sym("S");

    let feat_erb = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, 1, s, NB_ERB));
    let feat_spec = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, 2, s, NB_DF));

    m = m
        .with_input_fact(0, feat_erb)?
        .with_input_fact(1, feat_spec)?
        .with_input_names(["feat_erb", "feat_spec"])?
        .with_output_names(["e0", "e1", "e2", "e3", "emb", "c0", "lsnr"])?;

    m.analyse(true)?;
    let mut m = m.into_typed()?;
    m.declutter()?;

    let pulsed = PulsedModel::new(&m, s, &1.to_dim())?;
    let optimized = pulsed.into_typed()?.into_optimized()?;
    build_frozen_state(optimized)
}

/// Initialize pulsed ERB gain decoder.
/// Input: emb [1,S,n_hidden], e3 [1,64,S,8], e2 [1,64,S,8], e1 [1,64,S,16], e0 [1,64,S,32]
/// Output: mask
fn init_erb_decoder(path: &Path) -> TractResult<FrozenState> {
    let mut m = tract_onnx::onnx()
        .with_ignore_output_shapes(true)
        .model_for_path(path)?;
    let s = m.sym("S");

    let n_hidden = CONV_CH * NB_ERB / 4; // 512
    let e3f = NB_ERB / 4; // 8
    let e1f = NB_ERB / 2; // 16

    let emb = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, s, n_hidden));
    let e3 = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, CONV_CH, s, e3f));
    let e2 = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, CONV_CH, s, e3f));
    let e1 = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, CONV_CH, s, e1f));
    let e0 = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, CONV_CH, s, NB_ERB));

    m = m
        .with_input_fact(0, emb)?
        .with_input_fact(1, e3)?
        .with_input_fact(2, e2)?
        .with_input_fact(3, e1)?
        .with_input_fact(4, e0)?
        .with_input_names(["emb", "e3", "e2", "e1", "e0"])?;

    m.analyse(true)?;
    let mut m = m.into_typed()?;
    m.declutter()?;

    let pulsed = PulsedModel::new(&m, s, &1.to_dim())?;
    let optimized = pulsed.into_typed()?.into_optimized()?;
    build_frozen_state(optimized)
}

/// Initialize pulsed DF coefficient decoder.
/// Input: emb [1,S,n_hidden], c0 [1,64,S,96]
/// Output: coefs
fn init_df_decoder(path: &Path) -> TractResult<FrozenState> {
    let mut m = tract_onnx::onnx()
        .with_ignore_output_shapes(true)
        .model_for_path(path)?;
    let s = m.sym("S");

    let n_hidden = CONV_CH * NB_ERB / 4; // 512

    let emb = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, s, n_hidden));
    let c0 = InferenceFact::dt_shape(f32::datum_type(), shapefactoid!(1, CONV_CH, s, NB_DF));

    m = m
        .with_input_fact(0, emb)?
        .with_input_fact(1, c0)?
        .with_input_names(["emb", "c0"])?
        .with_output_names(["coefs"])?;

    m.analyse(true)?;
    let mut m = m.into_typed()?;
    m.declutter()?;

    let pulsed = PulsedModel::new(&m, s, &1.to_dim())?;
    let optimized = pulsed.into_typed()?.into_optimized()?;
    build_frozen_state(optimized)
}

// ── Run a frozen model: unfreeze → run → re-freeze ──────────────────────────

/// Unfreeze a frozen state, run one inference, re-freeze, and return outputs.
fn run_frozen(frozen: &mut FrozenState, inputs: TVec<TValue>) -> TractResult<TVec<TValue>> {
    let mut state = frozen.unfreeze();
    let outputs = state.run(inputs)?;
    *frozen = state.freeze();
    Ok(outputs)
}

// ── Denoiser struct ─────────────────────────────────────────────────────────

pub struct DeepFilterNetDenoiser {
    enc: Option<FrozenState>,
    erb_dec: Option<FrozenState>,
    df_dec: Option<FrozenState>,
    model_dir: String,

    // FFT
    fft_forward: Arc<dyn RealToComplex<f32>>,
    fft_inverse: Arc<dyn ComplexToReal<f32>>,
    fft_scratch_fwd: Vec<Complex32>,
    fft_scratch_inv: Vec<Complex32>,
    fft_out: Vec<Complex32>,
    ifft_in: Vec<Complex32>,

    // Vorbis window: sin(π/2 * sin²(π*(n+0.5)/N_h))
    window: Vec<f32>,
    // Analysis normalization factor: 1 / (fft_size² / (2 * hop_size))
    wnorm: f32,

    // Analysis memory (previous frame overlap)
    analysis_mem: Vec<f32>,
    // Synthesis overlap-add memory
    synthesis_mem: Vec<f32>,

    // Normalization state
    alpha: f32,
    erb_norm_state: Vec<f32>,
    cplx_norm_state: Vec<f32>,

    // ERB filterbank widths
    erb_widths: Vec<usize>,

    // Spectral history for deep filtering (ring buffer of Complex32 spectra)
    spec_history: Vec<Vec<Complex32>>,

    // Resampling via soxr (high-quality sinc interpolation)
    up_resampler: Option<SoxrStreamResampler>, // 16k → 48k
    down_resampler: Option<SoxrStreamResampler>, // 48k → 16k

    // Accumulation buffers for soxr's bursty output
    up_buf: Vec<f32>,  // accumulated upsampled f32 samples (48kHz)
    down_buf: Vec<u8>, // accumulated enhanced PCM16 bytes for downsampling
    out_buf: Vec<u8>,  // accumulated downsampled output bytes (16kHz)

    skip_counter: u32,
}

impl DeepFilterNetDenoiser {
    pub fn new(model_dir: &str) -> Self {
        let mut planner = RealFftPlanner::new();
        let fft_forward = planner.plan_fft_forward(FFT_SIZE);
        let fft_inverse = planner.plan_fft_inverse(FFT_SIZE);

        let fft_scratch_fwd = fft_forward.make_scratch_vec();
        let fft_scratch_inv = fft_inverse.make_scratch_vec();

        let window_size_h = FFT_SIZE / 2;
        let pi = std::f64::consts::PI;

        // Vorbis window: sin(π/2 * sin²(π*(n+0.5)/N_h))
        // Matches libDF: DFState::new()
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let sin_val = (0.5 * pi * (i as f64 + 0.5) / window_size_h as f64).sin();
                (0.5 * pi * sin_val * sin_val).sin() as f32
            })
            .collect();

        // Analysis normalization: wnorm = 1 / (window_size² / (2 * frame_size))
        let wnorm = 1.0 / (FFT_SIZE.pow(2) as f32 / (2 * HOP_SIZE) as f32);

        // Initialize soxr resamplers
        let up_resampler = SoxrStreamResampler::new(PIPELINE_SR as u32, SR as u32).ok();
        let down_resampler = SoxrStreamResampler::new(SR as u32, PIPELINE_SR as u32).ok();

        if up_resampler.is_none() || down_resampler.is_none() {
            warn!("[dfn3] Failed to create soxr resamplers, resampling will be disabled");
        }

        Self {
            enc: None,
            erb_dec: None,
            df_dec: None,
            model_dir: model_dir.to_string(),

            fft_forward,
            fft_inverse,
            fft_scratch_fwd,
            fft_scratch_inv,
            fft_out: vec![Complex32::new(0.0, 0.0); N_FREQS],
            ifft_in: vec![Complex32::new(0.0, 0.0); N_FREQS],

            window,
            wnorm,
            // analysis_mem: fft_size - frame_size = 960 - 480 = 480 samples
            analysis_mem: vec![0.0; FFT_SIZE - HOP_SIZE],
            // synthesis_mem: fft_size - frame_size = 480 samples
            synthesis_mem: vec![0.0; FFT_SIZE - HOP_SIZE],

            alpha: calc_norm_alpha(SR, HOP_SIZE, NORM_TAU),
            erb_norm_state: init_mean_norm_state(NB_ERB),
            cplx_norm_state: init_unit_norm_state(NB_DF),

            erb_widths: erb_fb(),

            // Pre-fill with zero frames (matches libDF rolling_spec_buf_y init)
            spec_history: vec![vec![Complex32::new(0.0, 0.0); N_FREQS]; DF_ORDER + CONV_LOOKAHEAD],

            up_resampler,
            down_resampler,

            up_buf: Vec::new(),
            down_buf: Vec::new(),
            out_buf: Vec::new(),

            skip_counter: 0,
        }
    }

    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = Path::new(&self.model_dir);
        let enc_path = dir.join("enc.onnx");
        let erb_dec_path = dir.join("erb_dec.onnx");
        let df_dec_path = dir.join("df_dec.onnx");

        for p in [&enc_path, &erb_dec_path, &df_dec_path] {
            if !p.exists() {
                return Err(format!("DeepFilterNet3 model not found: {}", p.display()).into());
            }
        }

        self.enc = Some(init_encoder(&enc_path)?);
        self.erb_dec = Some(init_erb_decoder(&erb_dec_path)?);
        self.df_dec = Some(init_df_decoder(&df_dec_path)?);

        info!(
            "[deepfilternet3] Loaded via tract PulsedModel (sr={}, fft={}, hop={}, erb={}, df={}, alpha={:.4})",
            SR, FFT_SIZE, HOP_SIZE, NB_ERB, NB_DF, self.alpha
        );

        Ok(())
    }

    /// Process a 16 kHz PCM-16 LE audio frame.
    pub fn denoise_frame(&mut self, pcm: &[u8]) -> Vec<u8> {
        let n_samples = pcm.len() / 2;
        if n_samples == 0 || self.enc.is_none() {
            return pcm.to_vec();
        }

        // ── Step 1: Upsample 16 kHz → 48 kHz via soxr, accumulate into up_buf ──
        let upsampled = match self.up_resampler.as_mut() {
            Some(r) => r.process(pcm),
            None => return pcm.to_vec(),
        };
        // Convert upsampled PCM16 → f32 and append to accumulation buffer
        for i in 0..(upsampled.len() / 2) {
            let s = i16::from_le_bytes([upsampled[i * 2], upsampled[i * 2 + 1]]) as f32 / 32768.0;
            self.up_buf.push(s);
        }

        // ── Step 2: Process complete HOP_SIZE chunks from up_buf ────────────
        while self.up_buf.len() >= HOP_SIZE {
            let chunk: Vec<f32> = self.up_buf.drain(..HOP_SIZE).collect();
            let enhanced = self.process_hop(&chunk);
            // Convert f32 → PCM16 LE and append to down_buf
            for &s in &enhanced {
                let val = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
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

    /// Process a single hop of 48 kHz audio through analysis → model → synthesis.
    fn process_hop(&mut self, input: &[f32]) -> Vec<f32> {
        debug_assert_eq!(input.len(), HOP_SIZE);

        // ── Frame analysis (matches libDF frame_analysis) ───────────────
        self.frame_analysis(input);

        // Maintain rolling spec buffer (matches libDF rolling_spec_buf_y/x)
        // Pop front, push back — buffer always has exactly DF_ORDER + CONV_LOOKAHEAD entries
        self.spec_history.remove(0);
        self.spec_history.push(self.fft_out.clone());

        // ── Silent frame detection (matches libDF skip_counter) ─────────
        let rms: f32 = input.iter().map(|x| x * x).sum::<f32>() / input.len() as f32;
        if rms < 1e-7 {
            self.skip_counter += 1;
        } else {
            self.skip_counter = 0;
        }
        if self.skip_counter > 5 {
            let mut empty_spec = vec![Complex32::new(0.0, 0.0); N_FREQS];
            return self.frame_synthesis(&mut empty_spec);
        }

        // ── Feature extraction ──────────────────────────────────────────
        let erb_feats = self.compute_erb_features();
        let cplx_feats = self.compute_cplx_features();

        // ── Run encoder (pulse=1, O(1) with stateful GRU) ───────────────
        let enc_result = self.run_encoder(&erb_feats, &cplx_feats);

        // Delayed frame: matches libDF rolling_spec_buf_y[df_order - 1]
        // This is always the (t - CONV_LOOKAHEAD) frame
        let delayed_idx = DF_ORDER - 1;

        let Some((emb, e0, e1, e2, e3, c0, lsnr)) = enc_result else {
            // Passthrough the delayed frame (t - CONV_LOOKAHEAD)
            let mut spec = self.spec_history[delayed_idx].clone();
            return self.frame_synthesis(&mut spec);
        };

        // ── Determine which stages to apply (matches apply_stages) ──────
        let (apply_erb, apply_zeros, apply_df) = apply_stages(lsnr);

        // Working copy of delayed spectrum (matches libDF rolling_spec_buf_y[df_order-1])
        let mut spec = self.spec_history[delayed_idx].clone();

        // ── Stage 1: ERB gain mask ──────────────────────────────────────
        if apply_erb {
            if let Some(gains) = self.run_erb_decoder(&emb, &e0, &e1, &e2, &e3) {
                apply_erb_gains(&mut spec, &gains, &self.erb_widths);
            }
        } else if apply_zeros {
            spec.fill(Complex32::new(0.0, 0.0));
        }

        // ── Stage 2: Deep filtering ─────────────────────────────────────
        if apply_df {
            if let Some(coefs) = self.run_df_decoder(&emb, &c0) {
                self.apply_deep_filter(&mut spec, &coefs);
            }
        }

        // ── Frame synthesis (matches libDF frame_synthesis) ──────────────
        self.frame_synthesis(&mut spec)
    }

    /// STFT analysis — matches libDF frame_analysis exactly.
    fn frame_analysis(&mut self, input: &[f32]) {
        let overlap = FFT_SIZE - HOP_SIZE; // 480

        let mut buf = vec![0.0f32; FFT_SIZE];
        for (i, b) in buf[..overlap].iter_mut().enumerate() {
            *b = self.analysis_mem[i] * self.window[i];
        }
        for (i, b) in buf[overlap..].iter_mut().enumerate() {
            *b = input[i] * self.window[overlap + i];
        }

        self.analysis_mem.copy_from_slice(input);

        self.fft_forward
            .process_with_scratch(&mut buf, &mut self.fft_out, &mut self.fft_scratch_fwd)
            .expect("FFT forward failed");

        for x in self.fft_out.iter_mut() {
            *x *= self.wnorm;
        }
    }

    /// ISTFT synthesis — matches libDF frame_synthesis exactly.
    fn frame_synthesis(&mut self, spec: &mut [Complex32]) -> Vec<f32> {
        debug_assert_eq!(spec.len(), N_FREQS);
        // Ensure DC and Nyquist are perfectly real for realfft
        spec[0].im = 0.0;
        spec[N_FREQS - 1].im = 0.0;

        self.ifft_in.copy_from_slice(spec);
        let mut x = vec![0.0f32; FFT_SIZE];
        self.fft_inverse
            .process_with_scratch(&mut self.ifft_in, &mut x, &mut self.fft_scratch_inv)
            .expect("FFT inverse failed");

        for (xi, wi) in x.iter_mut().zip(self.window.iter()) {
            *xi *= wi;
        }

        let mut output = vec![0.0f32; HOP_SIZE];
        for i in 0..HOP_SIZE {
            output[i] = x[i] + self.synthesis_mem[i];
        }

        self.synthesis_mem.copy_from_slice(&x[HOP_SIZE..]);
        output
    }

    /// Compute ERB features — matches libDF feat_erb pipeline.
    fn compute_erb_features(&mut self) -> Vec<f32> {
        let mut erb = vec![0.0f32; NB_ERB];

        let mut bin = 0usize;
        for (band, &width) in self.erb_widths.iter().enumerate() {
            let k = 1.0 / width as f32;
            let mut energy = 0.0f32;
            for j in 0..width {
                let idx = bin + j;
                if idx < N_FREQS {
                    let c = self.fft_out[idx];
                    energy += (c.re * c.re + c.im * c.im) * k;
                }
            }
            erb[band] = energy;
            bin += width;
        }

        for e in erb.iter_mut() {
            *e = (*e + 1e-10).log10() * 10.0;
        }

        for (i, e) in erb.iter_mut().enumerate() {
            self.erb_norm_state[i] = *e * (1.0 - self.alpha) + self.erb_norm_state[i] * self.alpha;
            *e -= self.erb_norm_state[i];
            *e /= 40.0;
        }

        erb
    }

    /// Compute complex spectral features — matches libDF feat_cplx_t pipeline.
    fn compute_cplx_features(&mut self) -> Vec<f32> {
        let mut feats = vec![0.0f32; 2 * NB_DF];

        for i in 0..NB_DF {
            let c = self.fft_out[i];
            let mag = c.norm();

            self.cplx_norm_state[i] =
                mag * (1.0 - self.alpha) + self.cplx_norm_state[i] * self.alpha;

            let inv_norm = 1.0 / self.cplx_norm_state[i].sqrt();
            feats[i] = c.re * inv_norm;
            feats[NB_DF + i] = c.im * inv_norm;
        }

        feats
    }

    /// Run encoder — unfreeze → run → re-freeze.
    #[allow(clippy::type_complexity)]
    fn run_encoder(
        &mut self,
        erb_feats: &[f32],
        cplx_feats: &[f32],
    ) -> Option<(
        TValue, // emb
        TValue, // e0
        TValue, // e1
        TValue, // e2
        TValue, // e3
        TValue, // c0
        f32,    // lsnr
    )> {
        let enc = self.enc.as_mut()?;

        // feat_erb: [1, 1, 1, 32]
        let erb_tensor =
            tract_ndarray::Array4::from_shape_vec((1, 1, 1, NB_ERB), erb_feats.to_vec())
                .ok()?
                .into_tensor()
                .into_tvalue();

        // feat_spec: [1, 2, 1, 96]
        let mut spec_data = vec![0.0f32; 2 * NB_DF];
        spec_data[..NB_DF].copy_from_slice(&cplx_feats[..NB_DF]);
        spec_data[NB_DF..].copy_from_slice(&cplx_feats[NB_DF..2 * NB_DF]);
        let spec_tensor = tract_ndarray::Array4::from_shape_vec((1, 2, 1, NB_DF), spec_data)
            .ok()?
            .into_tensor()
            .into_tvalue();

        let mut outputs = match run_frozen(enc, tvec!(erb_tensor, spec_tensor)) {
            Ok(o) => o,
            Err(e) => {
                warn!("[dfn3] encoder error: {}", e);
                return None;
            }
        };

        // Outputs: e0, e1, e2, e3, emb, c0, lsnr
        // pop() from end: lsnr, c0, emb, e3, e2, e1, e0
        let lsnr_tv = outputs.pop()?;
        let lsnr = *lsnr_tv.to_scalar::<f32>().ok()?;
        let c0 = outputs.pop()?;
        let emb = outputs.pop()?;
        let e3 = outputs.pop()?;
        let e2 = outputs.pop()?;
        let e1 = outputs.pop()?;
        let e0 = outputs.pop()?;

        Some((emb, e0, e1, e2, e3, c0, lsnr))
    }

    /// Run ERB gain decoder — unfreeze → run → re-freeze.
    fn run_erb_decoder(
        &mut self,
        emb: &TValue,
        e0: &TValue,
        e1: &TValue,
        e2: &TValue,
        e3: &TValue,
    ) -> Option<Vec<f32>> {
        let dec = self.erb_dec.as_mut()?;

        let result = match run_frozen(
            dec,
            tvec!(emb.clone(), e3.clone(), e2.clone(), e1.clone(), e0.clone(),),
        ) {
            Ok(o) => o,
            Err(e) => {
                warn!("[dfn3] erb_dec error: {}", e);
                return None;
            }
        };

        let mask = result[0].to_array_view::<f32>().ok()?;
        Some(mask.iter().copied().collect())
    }

    /// Run DF coefficient decoder — unfreeze → run → re-freeze.
    fn run_df_decoder(&mut self, emb: &TValue, c0: &TValue) -> Option<Vec<f32>> {
        let dec = self.df_dec.as_mut()?;

        let result = match run_frozen(dec, tvec!(emb.clone(), c0.clone())) {
            Ok(o) => o,
            Err(e) => {
                warn!("[dfn3] df_dec error: {}", e);
                return None;
            }
        };

        let coefs = result[0].to_array_view::<f32>().ok()?;
        Some(coefs.iter().copied().collect())
    }

    /// Apply deep filtering — matches libDF df() function.
    fn apply_deep_filter(&self, spec: &mut [Complex32], coefs: &[f32]) {
        if self.spec_history.len() < DF_ORDER {
            return;
        }

        for f in 0..NB_DF.min(spec.len()) {
            spec[f] = Complex32::new(0.0, 0.0);
        }

        let hist_len = self.spec_history.len();
        for tap in 0..DF_ORDER {
            let hist_idx = hist_len.saturating_sub(DF_ORDER) + tap;
            if hist_idx >= hist_len {
                break;
            }
            let hist_frame = &self.spec_history[hist_idx];
            for f in 0..NB_DF.min(spec.len()) {
                let coef_base = f * DF_ORDER * 2 + tap * 2;
                let coef_re = coefs.get(coef_base).copied().unwrap_or(0.0);
                let coef_im = coefs.get(coef_base + 1).copied().unwrap_or(0.0);
                let c = Complex32::new(coef_re, coef_im);
                spec[f] += hist_frame[f] * c;
            }
        }
    }

    pub fn reset(&mut self) {
        self.analysis_mem.fill(0.0);
        self.synthesis_mem.fill(0.0);
        self.erb_norm_state = init_mean_norm_state(NB_ERB);
        self.cplx_norm_state = init_unit_norm_state(NB_DF);
        self.spec_history =
            vec![vec![Complex32::new(0.0, 0.0); N_FREQS]; DF_ORDER + CONV_LOOKAHEAD];
        self.up_buf.clear();
        self.down_buf.clear();
        self.out_buf.clear();

        self.skip_counter = 0;

        // Re-create soxr resamplers to reset their internal state
        self.up_resampler = SoxrStreamResampler::new(PIPELINE_SR as u32, SR as u32).ok();
        self.down_resampler = SoxrStreamResampler::new(SR as u32, PIPELINE_SR as u32).ok();

        // Reset tract states (re-initialize from models)
        self.enc = None;
        self.erb_dec = None;
        self.df_dec = None;
    }
}

/// Determine which processing stages to apply based on local SNR.
fn apply_stages(lsnr: f32) -> (bool, bool, bool) {
    let min_db_thresh = -15.0f32;
    let max_db_erb_thresh = 35.0f32;
    let max_db_df_thresh = 35.0f32;

    if lsnr < min_db_thresh {
        (false, true, false) // Very noisy: apply zero mask
    } else if lsnr > max_db_erb_thresh {
        (false, false, false) // Very clean: skip all processing
    } else if lsnr > max_db_df_thresh {
        (true, false, false) // Slightly noisy: ERB only
    } else {
        (true, false, true) // Regular: both ERB + DF
    }
}

/// Apply ERB-band gains to full spectrum — matches libDF apply_interp_band_gain.
fn apply_erb_gains(spec: &mut [Complex32], gains: &[f32], erb_widths: &[usize]) {
    let mut bin = 0usize;
    for (band, &width) in erb_widths.iter().enumerate() {
        let g = gains.get(band).copied().unwrap_or(1.0);
        for j in 0..width {
            let idx = bin + j;
            if idx < spec.len() {
                spec[idx] *= g;
            }
        }
        bin += width;
    }
}

/// Initialize ERB mean normalization state with a linear ramp.
fn init_mean_norm_state(nb_erb: usize) -> Vec<f32> {
    let min = MEAN_NORM_INIT[0]; // -60
    let max = MEAN_NORM_INIT[1]; // -90
    let step = (max - min) / (nb_erb - 1) as f32;
    (0..nb_erb).map(|i| min + i as f32 * step).collect()
}

/// Initialize unit normalization state with a linear ramp.
fn init_unit_norm_state(nb_df: usize) -> Vec<f32> {
    let min = UNIT_NORM_INIT[0]; // 0.001
    let max = UNIT_NORM_INIT[1]; // 0.0001
    let step = (max - min) / (nb_df - 1) as f32;
    (0..nb_df).map(|i| min + i as f32 * step).collect()
}
