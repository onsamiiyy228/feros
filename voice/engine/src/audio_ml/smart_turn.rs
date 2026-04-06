//! Smart Turn — end-of-turn detection via ONNX Runtime 2.0.
//! STFT via `rustfft`, mel filterbank computed at init.

use std::f32::consts::PI;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use num_complex::Complex32;
use ort::session::Session;
use ort::value::Tensor;
use rustfft::{Fft, FftPlanner};
use tracing::{debug, info};

const MEL_SR: usize = 16000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 80;
const CHUNK_SECS: usize = 8;
const N_FRAMES: usize = CHUNK_SECS * MEL_SR / HOP;

pub struct SmartTurnAnalyzer {
    session: Option<Session>,
    model_path: String,
    pub threshold: f32,
    mel_filters: Vec<Vec<f32>>,
    hann_window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
}

impl SmartTurnAnalyzer {
    pub fn new(model_path: &str, threshold: f32) -> Self {
        let mut p = FftPlanner::new();
        Self {
            session: None,
            model_path: model_path.to_string(),
            threshold,
            mel_filters: compute_mel_filterbank(),
            hann_window: compute_hann_window(N_FFT),
            fft: p.plan_fft_forward(N_FFT),
        }
    }

    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.session.is_some() {
            return Ok(());
        }
        let path = Path::new(&self.model_path);
        if !path.exists() {
            return Err(format!("Smart Turn not found: {}", self.model_path).into());
        }
        let session = Session::builder()?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_file(path)?;
        info!(
            "Smart Turn loaded (input: '{}')",
            session.inputs()[0].name()
        );
        self.session = Some(session);
        Ok(())
    }

    pub fn predict(
        &mut self,
        audio: &[u8],
    ) -> Result<(bool, f32), Box<dyn std::error::Error + Send + Sync>> {
        let n = audio.len() / 2;
        let mut af: Vec<f32> = (0..n)
            .map(|i| i16::from_le_bytes([audio[i * 2], audio[i * 2 + 1]]) as f32 / 32768.0)
            .collect();

        let max = CHUNK_SECS * MEL_SR;
        if af.len() > max {
            let off = af.len() - max;
            af = af[off..].to_vec();
        } else if af.len() < max {
            let mut p = vec![0.0f32; max - af.len()];
            p.extend_from_slice(&af);
            af = p;
        }

        // zero-mean unit-variance normalization (like WhisperFeatureExtractor do_normalize=True)
        let mean = af.iter().sum::<f32>() / af.len() as f32;
        let var = af.iter().map(|&x| (x - mean) * (x - mean)).sum::<f32>() / af.len() as f32;
        let std = (var + 1e-7).sqrt();
        for v in &mut af {
            *v = (*v - mean) / std;
        }

        let start = Instant::now();
        let mel = self.compute_log_mel_spectrogram(&af);

        let input_t = Tensor::from_array(([1usize, N_MELS, N_FRAMES], mel.into_boxed_slice()))?;

        let vals: Vec<ort::session::SessionInputValue> = vec![input_t.into()];

        let session = self.session.as_mut().ok_or("Smart Turn not init")?;
        let outputs = session.run(vals.as_slice())?;

        let prob = outputs[0]
            .try_extract_tensor::<f32>()?
            .1
            .first()
            .copied()
            .unwrap_or(0.0);
        let complete = prob > self.threshold;

        debug!(
            "[smart_turn] {} prob={:.3} time={:.1}ms",
            if complete { "COMPLETE" } else { "INCOMPLETE" },
            prob,
            start.elapsed().as_secs_f64() * 1000.0
        );

        Ok((complete, prob))
    }
    /// Compute log-mel spectrogram from raw PCM16 bytes.
    /// Does truncation/padding, zero-mean normalization, and mel extraction.
    /// Useful for testing the feature extraction pipeline without ONNX.
    pub fn compute_log_mel_spectrogram_from_pcm(&self, audio: &[u8]) -> Vec<f32> {
        let n = audio.len() / 2;
        let mut af: Vec<f32> = (0..n)
            .map(|i| i16::from_le_bytes([audio[i * 2], audio[i * 2 + 1]]) as f32 / 32768.0)
            .collect();

        let max = CHUNK_SECS * MEL_SR;
        if af.len() > max {
            let off = af.len() - max;
            af = af[off..].to_vec();
        } else if af.len() < max {
            let mut p = vec![0.0f32; max - af.len()];
            p.extend_from_slice(&af);
            af = p;
        }

        // zero-mean unit-variance normalization
        let mean = af.iter().sum::<f32>() / af.len() as f32;
        let var = af.iter().map(|&x| (x - mean) * (x - mean)).sum::<f32>() / af.len() as f32;
        let std = (var + 1e-7).sqrt();
        for v in &mut af {
            *v = (*v - mean) / std;
        }

        self.compute_log_mel_spectrogram(&af)
    }

    fn compute_log_mel_spectrogram(&self, audio: &[f32]) -> Vec<f32> {
        let nfo = N_FFT / 2 + 1;
        let half = N_FFT / 2; // 200

        // Center padding with reflect mode (like Whisper center=True)
        let mut padded = Vec::with_capacity(audio.len() + 2 * half);
        for i in (1..=half).rev() {
            padded.push(audio[i.min(audio.len() - 1)]);
        }
        padded.extend_from_slice(audio);
        for i in 0..half {
            let idx = audio.len().saturating_sub(2 + i);
            padded.push(audio[idx]);
        }

        let total_frames = if padded.len() >= N_FFT {
            (padded.len() - N_FFT) / HOP + 1
        } else {
            0
        };
        // Drop last frame (Whisper convention: log_spec[:, :-1])
        let nw = total_frames.saturating_sub(1).min(N_FRAMES);
        let mut mags = Vec::with_capacity(nw);

        for t in 0..nw {
            let ws = t * HOP;
            let mut buf: Vec<Complex32> = (0..N_FFT)
                .map(|n| {
                    Complex32::new(
                        if ws + n < padded.len() {
                            padded[ws + n] * self.hann_window[n]
                        } else {
                            0.0
                        },
                        0.0,
                    )
                })
                .collect();
            self.fft.process(&mut buf);
            // Power spectrogram: |z|^2 (Whisper uses power=2.0)
            mags.push((0..nfo).map(|k| buf[k].norm_sqr()).collect::<Vec<f32>>());
        }

        let mut mel = vec![0.0f32; N_MELS * N_FRAMES];
        for m in 0..N_MELS {
            for t in 0..nw {
                let mut s = 0.0f32;
                for (k, &mag_val) in mags[t].iter().enumerate() {
                    s += self.mel_filters[m][k] * mag_val;
                }
                mel[m * N_FRAMES + t] = s;
            }
        }

        // Whisper normalization: log10, clip 80dB floor, (v+4)/4
        for v in &mut mel {
            *v = (*v).max(1e-10).log10();
        }
        let mx = mel.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let floor = mx - 8.0;
        for v in &mut mel {
            *v = (*v).max(floor);
        }
        for v in &mut mel {
            *v = (*v + 4.0) / 4.0;
        }

        mel
    }
}

fn compute_mel_filterbank() -> Vec<Vec<f32>> {
    let n = N_FFT / 2 + 1;
    let mut f = vec![vec![0.0f32; n]; N_MELS];

    // Slaney scale (linear below 1000Hz, log above)
    let ml = hz_to_mel_slaney(0.0);
    let mh = hz_to_mel_slaney(MEL_SR as f32 / 2.0);
    let mel_pts: Vec<f32> = (0..N_MELS + 2)
        .map(|i| ml + (mh - ml) * i as f32 / (N_MELS + 1) as f32)
        .collect();
    let hz_pts: Vec<f32> = mel_pts.iter().map(|&m| mel_to_hz_slaney(m)).collect();

    // FFT bin frequencies
    let fft_freqs: Vec<f32> = (0..n)
        .map(|i| i as f32 * (MEL_SR as f32) / N_FFT as f32)
        .collect();

    // Create triangular Slaney filters
    for m in 0..N_MELS {
        let f_left = hz_pts[m];
        let f_center = hz_pts[m + 1];
        let f_right = hz_pts[m + 2];

        // Slaney Area Normalization: constant energy per filter
        let enorm = 2.0 / (f_right - f_left);

        for k in 0..n {
            let freq = fft_freqs[k];
            if freq > f_left && freq < f_right {
                let weight = if freq <= f_center {
                    (freq - f_left) / (f_center - f_left)
                } else {
                    (f_right - freq) / (f_right - f_center)
                };
                f[m][k] = weight * enorm;
            }
        }
    }
    f
}

fn hz_to_mel_slaney(hz: f32) -> f32 {
    let min_log_hz = 1000.0;
    let min_log_mel = 15.0;
    let logstep = 27.0 / 6.4f32.ln();
    if hz >= min_log_hz {
        min_log_mel + (hz / min_log_hz).ln() * logstep
    } else {
        3.0 * hz / 200.0
    }
}

fn mel_to_hz_slaney(mel: f32) -> f32 {
    let min_log_hz = 1000.0;
    let min_log_mel = 15.0;
    let logstep = 6.4f32.ln() / 27.0;
    if mel >= min_log_mel {
        min_log_hz * (logstep * (mel - min_log_mel)).exp()
    } else {
        200.0 * mel / 3.0
    }
}

// Periodic Hann Window for Whisper compatibility (ends at sz)
fn compute_hann_window(sz: usize) -> Vec<f32> {
    (0..sz)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f32 / sz as f32).cos()))
        .collect()
}
