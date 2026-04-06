//! DTLN denoiser — two-stage ONNX pipeline via ONNX Runtime 2.0.
//! FFT/IFFT via `realfft` (real-to-complex). State shapes from ONNX metadata.

use std::path::Path;
use std::sync::Arc;

use num_complex::Complex32;
use ort::session::Session;
use ort::value::Tensor;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use tracing::{info, warn};

const BLOCK_LEN: usize = 512;
const BLOCK_SHIFT: usize = 128;
const FFT_OUT_SIZE: usize = BLOCK_LEN / 2 + 1; // 257

struct OnnxState {
    shape: Vec<usize>,
    data: Vec<f32>,
}

pub struct DtlnDenoiser {
    sess_1: Option<Session>,
    sess_2: Option<Session>,
    model_dir: String,
    states_1: Vec<OnnxState>,
    states_2: Vec<OnnxState>,
    in_buffer: Vec<f32>,
    out_buffer: Vec<f32>,
    fft_forward: Arc<dyn RealToComplex<f32>>,
    fft_inverse: Arc<dyn ComplexToReal<f32>>,
    // Pre-allocated scratch buffers to avoid per-frame allocations
    fft_scratch_in: Vec<f32>,
    fft_scratch_out: Vec<Complex32>,
    ifft_scratch_in: Vec<Complex32>,
    ifft_scratch_out: Vec<f32>,
    mag_buf: Vec<f32>,
    phase_buf: Vec<f32>,
    warmup_remaining: u32,
    // Accumulation buffers for non-aligned frame sizes
    sample_buf: Vec<f32>, // accumulated input f32 samples
    output_buf: Vec<u8>,  // accumulated output PCM16 bytes
}

impl DtlnDenoiser {
    pub fn new(model_dir: &str) -> Self {
        let mut p = RealFftPlanner::new();
        let fft_forward = p.plan_fft_forward(BLOCK_LEN);
        let fft_inverse = p.plan_fft_inverse(BLOCK_LEN);
        Self {
            sess_1: None,
            sess_2: None,
            model_dir: model_dir.to_string(),
            states_1: Vec::new(),
            states_2: Vec::new(),
            in_buffer: vec![0.0; BLOCK_LEN],
            out_buffer: vec![0.0; BLOCK_LEN],
            fft_scratch_in: vec![0.0; BLOCK_LEN],
            fft_scratch_out: vec![Complex32::new(0.0, 0.0); FFT_OUT_SIZE],
            ifft_scratch_in: vec![Complex32::new(0.0, 0.0); FFT_OUT_SIZE],
            ifft_scratch_out: vec![0.0; BLOCK_LEN],
            mag_buf: vec![0.0; FFT_OUT_SIZE],
            phase_buf: vec![0.0; FFT_OUT_SIZE],
            fft_forward,
            fft_inverse,
            warmup_remaining: 5,
            sample_buf: Vec::new(),
            output_buf: Vec::new(),
        }
    }

    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let p1 = Path::new(&self.model_dir).join("model_1.onnx");
        let p2 = Path::new(&self.model_dir).join("model_2.onnx");
        if !p1.exists() || !p2.exists() {
            return Err(format!("DTLN models not found in {}", self.model_dir).into());
        }

        let s1 = Session::builder()?
            .with_intra_threads(1)?
            .commit_from_file(&p1)?;
        let s2 = Session::builder()?
            .with_intra_threads(1)?
            .commit_from_file(&p2)?;

        self.states_1 = s1.inputs()[1..]
            .iter()
            .map(|inp| {
                let dims: Vec<usize> = if let Some(shape) = inp.dtype().tensor_shape() {
                    shape
                        .iter()
                        .map(|&d| if d < 0 { 1 } else { d as usize })
                        .collect()
                } else {
                    vec![1]
                };
                let n: usize = dims.iter().product();
                info!("[dtln] model_1 state '{}' {:?}", inp.name(), dims);
                OnnxState {
                    shape: dims,
                    data: vec![0.0; n],
                }
            })
            .collect();

        self.states_2 = s2.inputs()[1..]
            .iter()
            .map(|inp| {
                let dims: Vec<usize> = if let Some(shape) = inp.dtype().tensor_shape() {
                    shape
                        .iter()
                        .map(|&d| if d < 0 { 1 } else { d as usize })
                        .collect()
                } else {
                    vec![1]
                };
                let n: usize = dims.iter().product();
                info!("[dtln] model_2 state '{}' {:?}", inp.name(), dims);
                OnnxState {
                    shape: dims,
                    data: vec![0.0; n],
                }
            })
            .collect();

        info!(
            "[dtln] Loaded ({} + {} states)",
            self.states_1.len(),
            self.states_2.len()
        );
        self.sess_1 = Some(s1);
        self.sess_2 = Some(s2);
        Ok(())
    }

    pub fn denoise_frame(&mut self, pcm: &[u8]) -> Vec<u8> {
        let n_samples = pcm.len() / 2;
        if n_samples == 0 || self.sess_1.is_none() {
            return pcm.to_vec();
        }

        // Convert PCM16 → f32 and accumulate into sample_buf
        for i in 0..n_samples {
            let s = i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]) as f32 / 32768.0;
            self.sample_buf.push(s);
        }

        // Process complete BLOCK_SHIFT chunks from sample_buf
        while self.sample_buf.len() >= BLOCK_SHIFT {
            let chunk: Vec<f32> = self.sample_buf.drain(..BLOCK_SHIFT).collect();
            self.process_shift(&chunk);

            if self.warmup_remaining > 0 {
                self.warmup_remaining -= 1;
                if self.warmup_remaining == 0 {
                    self.out_buffer.fill(0.0);
                }
                // Emit silence for warmup shifts
                for _ in 0..BLOCK_SHIFT {
                    self.output_buf.extend_from_slice(&0i16.to_le_bytes());
                }
            } else {
                // Emit denoised BLOCK_SHIFT samples
                for &s in &self.out_buffer[..BLOCK_SHIFT] {
                    let val = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    self.output_buf.extend_from_slice(&val.to_le_bytes());
                }
            }
        }

        // Emit exactly n_samples * 2 bytes from output_buf
        let expected_len = n_samples * 2;
        if self.output_buf.len() >= expected_len {
            self.output_buf.drain(..expected_len).collect()
        } else {
            // Not enough output yet (startup) — return silence
            vec![0u8; expected_len]
        }
    }

    fn process_shift(&mut self, chunk: &[f32]) {
        self.in_buffer.copy_within(BLOCK_SHIFT.., 0);
        self.in_buffer[BLOCK_LEN - BLOCK_SHIFT..].copy_from_slice(chunk);

        // Real-to-complex FFT using pre-allocated buffers
        self.fft_scratch_in.copy_from_slice(&self.in_buffer);
        self.fft_forward
            .process(&mut self.fft_scratch_in, &mut self.fft_scratch_out)
            .expect("FFT forward failed");

        for k in 0..FFT_OUT_SIZE {
            self.mag_buf[k] = self.fft_scratch_out[k].norm();
            self.phase_buf[k] = self.fft_scratch_out[k].im.atan2(self.fft_scratch_out[k].re);
        }

        // Stage 1: build inputs, then run
        let mag_t = Tensor::from_array((
            [1usize, 1, FFT_OUT_SIZE],
            self.mag_buf.clone().into_boxed_slice(),
        ))
        .unwrap();
        let mut v1: Vec<ort::session::SessionInputValue> = vec![mag_t.into()];
        for st in &self.states_1 {
            let t =
                Tensor::from_array((st.shape.clone(), st.data.clone().into_boxed_slice())).unwrap();
            v1.push(t.into());
        }

        let s1 = match self.sess_1.as_mut() {
            Some(s) => s,
            None => return,
        };
        let o1 = match s1.run(v1.as_slice()) {
            Ok(o) => o,
            Err(e) => {
                warn!("DTLN s1: {}", e);
                return;
            }
        };

        let mask: Vec<f32> = o1[0].try_extract_tensor::<f32>().unwrap().1.to_vec();

        for (i, st) in self.states_1.iter_mut().enumerate() {
            if let Ok(t) = o1[i + 1].try_extract_tensor::<f32>() {
                st.data.clear();
                st.data.extend_from_slice(t.1);
            }
        }

        // IFFT with mask applied (using realfft inverse: only N/2+1 bins needed)
        for k in 0..FFT_OUT_SIZE {
            let mm = self.mag_buf[k] * mask.get(k).copied().unwrap_or(0.0);
            self.ifft_scratch_in[k] = Complex32::from_polar(mm, self.phase_buf[k]);
        }
        // realfft requires DC and Nyquist bins to be purely real
        self.ifft_scratch_in[0].im = 0.0;
        self.ifft_scratch_in[FFT_OUT_SIZE - 1].im = 0.0;
        self.fft_inverse
            .process(&mut self.ifft_scratch_in, &mut self.ifft_scratch_out)
            .expect("FFT inverse failed");

        // realfft inverse produces unnormalized output, normalize by BLOCK_LEN
        let norm = 1.0 / BLOCK_LEN as f32;

        // Stage 2
        let est: Box<[f32]> = self
            .ifft_scratch_out
            .iter()
            .map(|&v| v * norm)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let ea = Tensor::from_array(([1usize, 1, BLOCK_LEN], est)).unwrap();
        let mut v2: Vec<ort::session::SessionInputValue> = vec![ea.into()];
        for st in &self.states_2 {
            let t =
                Tensor::from_array((st.shape.clone(), st.data.clone().into_boxed_slice())).unwrap();
            v2.push(t.into());
        }

        let s2 = match self.sess_2.as_mut() {
            Some(s) => s,
            None => return,
        };
        let o2 = match s2.run(v2.as_slice()) {
            Ok(o) => o,
            Err(e) => {
                warn!("DTLN s2: {}", e);
                return;
            }
        };

        let ob: Vec<f32> = o2[0].try_extract_tensor::<f32>().unwrap().1.to_vec();

        for (i, st) in self.states_2.iter_mut().enumerate() {
            if let Ok(t) = o2[i + 1].try_extract_tensor::<f32>() {
                st.data.clear();
                st.data.extend_from_slice(t.1);
            }
        }

        // Overlap-add
        self.out_buffer.copy_within(BLOCK_SHIFT.., 0);
        for i in (BLOCK_LEN - BLOCK_SHIFT)..BLOCK_LEN {
            self.out_buffer[i] = 0.0;
        }
        for (i, &v) in ob.iter().take(BLOCK_LEN).enumerate() {
            self.out_buffer[i] += v;
        }
    }

    pub fn reset(&mut self) {
        self.in_buffer.fill(0.0);
        self.out_buffer.fill(0.0);
        for s in &mut self.states_1 {
            s.data.fill(0.0);
        }
        for s in &mut self.states_2 {
            s.data.fill(0.0);
        }
        self.warmup_remaining = 5;
        self.sample_buf.clear();
        self.output_buf.clear();
    }
}
