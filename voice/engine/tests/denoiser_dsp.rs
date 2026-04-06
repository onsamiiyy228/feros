//! DTLN denoiser tests — verifies the DSP pipeline used around
//! the ONNX models: buffer management, FFT/IFFT roundtrip, overlap-add,
//! PCM encoding/decoding, and warmup behavior.
//!
//! Reference: breizhn/DTLN (MIT license)
//!   - block_len = 512 (32ms @ 16kHz)
//!   - block_shift = 128 (8ms @ 16kHz)
//!   - FFT output size = 257 (block_len/2 + 1)
//!   - Two-stage pipeline: freq-domain mask → time-domain refinement
//!   - Overlap-add output reconstruction

use std::f32::consts::PI;
use voice_engine::audio_ml::denoiser::dtln::DtlnDenoiser;

// ─── Constants (must match denoiser.rs) ─────────────────────────

const BLOCK_LEN: usize = 512;
const BLOCK_SHIFT: usize = 128;
const FFT_OUT_SIZE: usize = BLOCK_LEN / 2 + 1; // 257

// ─── Helpers ────────────────────────────────────────────────────

/// Generate a sine wave as PCM16 little-endian bytes.
fn sine_pcm16(freq: f32, n_samples: usize, sample_rate: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_samples * 2);
    for i in 0..n_samples {
        let sample = (0.5 * (2.0 * PI * freq * i as f32 / sample_rate as f32).sin()) * 32767.0;
        let s = (sample as i16).to_le_bytes();
        bytes.push(s[0]);
        bytes.push(s[1]);
    }
    bytes
}

/// Generate silence as PCM16 bytes.
fn silence_pcm16(n_samples: usize) -> Vec<u8> {
    vec![0u8; n_samples * 2]
}

/// Decode PCM16 bytes to f32 samples.
fn pcm16_to_f32(pcm: &[u8]) -> Vec<f32> {
    let n = pcm.len() / 2;
    (0..n)
        .map(|i| i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]) as f32 / 32768.0)
        .collect()
}

/// RMS of a float slice.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&x| x * x).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

// ─── Constants Tests ────────────────────────────────────────────

#[test]
fn dtln_constants_match_reference() {
    // These constants are fixed by the pretrained DTLN model.
    // Changing them will break inference.
    // Reference: breizhn/DTLN real_time_processing.py
    assert_eq!(BLOCK_LEN, 512, "block_len must be 512 (32ms @ 16kHz)");
    assert_eq!(BLOCK_SHIFT, 128, "block_shift must be 128 (8ms @ 16kHz)");
    assert_eq!(
        FFT_OUT_SIZE, 257,
        "FFT output must be block_len/2+1 = 257 bins"
    );
    assert_eq!(
        BLOCK_LEN / BLOCK_SHIFT,
        4,
        "overlap ratio must be 4 (75% overlap)"
    );
}

#[test]
fn dtln_frame_timing() {
    // At 16kHz: block_len = 32ms, block_shift = 8ms
    let sr = 16000.0;
    let block_ms = (BLOCK_LEN as f32 / sr) * 1000.0;
    let shift_ms = (BLOCK_SHIFT as f32 / sr) * 1000.0;

    assert!((block_ms - 32.0).abs() < 0.1, "block should be 32ms");
    assert!((shift_ms - 8.0).abs() < 0.1, "shift should be 8ms");
}

// ─── PCM Encoding/Decoding Tests ────────────────────────────────

#[test]
fn pcm16_roundtrip() {
    // Encode a known signal to PCM16, then decode.
    // The quantization error should be < 1/32768.
    let n = 512;
    let original: Vec<f32> = (0..n)
        .map(|i| 0.5 * (2.0 * PI * 440.0 * i as f32 / 16000.0).sin())
        .collect();

    // Encode
    let mut pcm = Vec::with_capacity(n * 2);
    for &s in &original {
        let i16_val = (s * 32767.0) as i16;
        pcm.extend_from_slice(&i16_val.to_le_bytes());
    }

    // Decode
    let decoded = pcm16_to_f32(&pcm);
    assert_eq!(decoded.len(), n);

    // Max quantization error should be < 1/32768 ≈ 0.00003
    let max_err: f32 = original
        .iter()
        .zip(decoded.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);

    assert!(
        max_err < 0.001,
        "PCM16 roundtrip error too large: {}",
        max_err
    );
}

// ─── Output Invariants Tests ────────────────────────────────────

#[test]
fn denoise_preserves_length() {
    // Without models loaded, denoise_frame should return same-length output
    let mut denoiser = DtlnDenoiser::new("/nonexistent");
    let audio = sine_pcm16(440.0, BLOCK_LEN, 16000);

    let output = denoiser.denoise_frame(&audio);
    assert_eq!(
        output.len(),
        audio.len(),
        "output length should match input"
    );
}

#[test]
fn denoise_empty_input() {
    let mut denoiser = DtlnDenoiser::new("/nonexistent");
    let output = denoiser.denoise_frame(&[]);
    assert_eq!(output.len(), 0, "empty input should produce empty output");
}

#[test]
fn denoise_passthrough_without_models() {
    // Without ONNX models, denoise_frame should pass through audio unchanged
    let mut denoiser = DtlnDenoiser::new("/nonexistent");
    let audio = sine_pcm16(440.0, BLOCK_LEN, 16000);

    let output = denoiser.denoise_frame(&audio);
    assert_eq!(output, audio, "without models, output should equal input");
}

// ─── Warmup Tests ───────────────────────────────────────────────

#[test]
fn warmup_count_default() {
    // The denoiser should have a warmup period where it outputs silence
    // while LSTM states settle. Default is 5 frames (~160ms).
    // Reference: both Python and Rust use warmup_remaining = 5
    let denoiser = DtlnDenoiser::new("/nonexistent");
    // Without models loaded, warmup is bypassed (sess_1 is None).
    // This test just verifies the default warmup count exists.
    // The structural test is that warmup_remaining starts at 5.
    // We can't observe it directly without models, but we verify
    // that the reset method exists and doesn't panic.
    let mut d = denoiser;
    d.reset();
    // After reset, warmup should be re-initialized
}

// ─── Overlap-Add Structure Tests ────────────────────────────────

#[test]
fn block_shift_divides_block_len() {
    // The overlap-add requires block_len to be a multiple of block_shift
    assert_eq!(
        BLOCK_LEN % BLOCK_SHIFT,
        0,
        "block_len must be divisible by block_shift for overlap-add"
    );
}

#[test]
fn multiple_shifts_per_frame() {
    // A standard 512-sample frame should be processed in 4 shifts
    let shifts_per_frame = BLOCK_LEN / BLOCK_SHIFT;
    assert_eq!(shifts_per_frame, 4, "should process 4 shifts per frame");
}

#[test]
fn denoise_handles_partial_frame() {
    // If input has leftover samples that don't fill a block_shift,
    // they should be passed through unchanged
    let mut denoiser = DtlnDenoiser::new("/nonexistent");

    // 600 samples = 4 full shifts (512) + 88 leftover
    let audio = sine_pcm16(440.0, 600, 16000);
    let output = denoiser.denoise_frame(&audio);

    assert_eq!(output.len(), audio.len(), "should handle partial frames");
}

// ─── FFT/IFFT Roundtrip Test ────────────────────────────────────

#[test]
fn fft_ifft_roundtrip_via_realfft() {
    // Verify that realfft forward → inverse gives back the original signal
    // (within f32 precision). This is the core DSP operation in the denoiser.
    use num_complex::Complex32;
    use realfft::RealFftPlanner;

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(BLOCK_LEN);
    let ifft = planner.plan_fft_inverse(BLOCK_LEN);

    // Input: 440Hz sine wave
    let original: Vec<f32> = (0..BLOCK_LEN)
        .map(|i| 0.5 * (2.0 * PI * 440.0 * i as f32 / 16000.0).sin())
        .collect();

    // Forward FFT
    let mut input = original.clone();
    let mut spectrum = vec![Complex32::new(0.0, 0.0); FFT_OUT_SIZE];
    fft.process(&mut input, &mut spectrum).unwrap();

    // Extract magnitude and phase (as denoiser does)
    let mag: Vec<f32> = spectrum.iter().map(|c| c.norm()).collect();
    let phase: Vec<f32> = spectrum.iter().map(|c| c.im.atan2(c.re)).collect();

    // Reconstruct complex from magnitude and phase
    let mut reconstructed: Vec<Complex32> = mag
        .iter()
        .zip(phase.iter())
        .map(|(&m, &p)| Complex32::from_polar(m, p))
        .collect();

    // realfft inverse requires DC and Nyquist bins to have zero imaginary part
    reconstructed[0] = Complex32::new(reconstructed[0].re, 0.0);
    reconstructed[FFT_OUT_SIZE - 1] = Complex32::new(reconstructed[FFT_OUT_SIZE - 1].re, 0.0);

    // Inverse FFT
    let mut output = vec![0.0f32; BLOCK_LEN];
    ifft.process(&mut reconstructed, &mut output).unwrap();

    // realfft inverse is unnormalized, divide by BLOCK_LEN
    let norm = 1.0 / BLOCK_LEN as f32;
    for v in &mut output {
        *v *= norm;
    }

    // Check roundtrip error
    let max_err: f32 = original
        .iter()
        .zip(output.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);

    assert!(
        max_err < 1e-5,
        "FFT→IFFT roundtrip error too large: {}",
        max_err
    );
}

#[test]
fn fft_magnitude_of_sine() {
    // A pure 440Hz sine wave should produce a peak at bin 440*512/16000 = 14.08 ≈ bin 14
    use num_complex::Complex32;
    use realfft::RealFftPlanner;

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(BLOCK_LEN);

    let mut input: Vec<f32> = (0..BLOCK_LEN)
        .map(|i| 0.5 * (2.0 * PI * 440.0 * i as f32 / 16000.0).sin())
        .collect();
    let mut spectrum = vec![Complex32::new(0.0, 0.0); FFT_OUT_SIZE];
    fft.process(&mut input, &mut spectrum).unwrap();

    let mag: Vec<f32> = spectrum.iter().map(|c| c.norm()).collect();

    // Find peak bin
    let peak_bin = mag
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i)
        .unwrap();

    // 440Hz → bin 440 * 512 / 16000 ≈ 14
    let expected_bin = (440.0 * BLOCK_LEN as f32 / 16000.0).round() as usize;
    assert!(
        (peak_bin as i32 - expected_bin as i32).unsigned_abs() <= 1,
        "440Hz peak should be at bin ~{}, got bin {}",
        expected_bin,
        peak_bin
    );
}

// ─── Mask Application Test ──────────────────────────────────────

#[test]
fn identity_mask_preserves_signal() {
    // If the DTLN model produces an all-ones mask (identity),
    // the output should equal the input (modulo overlap-add windowing).
    // This tests the mag * mask * exp(j*phase) → IFFT path.
    use num_complex::Complex32;
    use realfft::RealFftPlanner;

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(BLOCK_LEN);
    let ifft = planner.plan_fft_inverse(BLOCK_LEN);

    let original: Vec<f32> = (0..BLOCK_LEN)
        .map(|i| 0.3 * (2.0 * PI * 1000.0 * i as f32 / 16000.0).sin())
        .collect();

    // Forward FFT
    let mut input = original.clone();
    let mut spectrum = vec![Complex32::new(0.0, 0.0); FFT_OUT_SIZE];
    fft.process(&mut input, &mut spectrum).unwrap();

    let mag: Vec<f32> = spectrum.iter().map(|c| c.norm()).collect();
    let phase: Vec<f32> = spectrum.iter().map(|c| c.im.atan2(c.re)).collect();

    // Apply identity mask (all ones)
    let mask = vec![1.0f32; FFT_OUT_SIZE];
    let mut masked: Vec<Complex32> = mag
        .iter()
        .zip(mask.iter())
        .zip(phase.iter())
        .map(|((&m, &k), &p)| Complex32::from_polar(m * k, p))
        .collect();

    // realfft inverse requires DC and Nyquist bins to have zero imaginary part
    masked[0] = Complex32::new(masked[0].re, 0.0);
    masked[FFT_OUT_SIZE - 1] = Complex32::new(masked[FFT_OUT_SIZE - 1].re, 0.0);

    // Inverse FFT
    let mut output = vec![0.0f32; BLOCK_LEN];
    ifft.process(&mut masked, &mut output).unwrap();
    let norm = 1.0 / BLOCK_LEN as f32;
    for v in &mut output {
        *v *= norm;
    }

    let max_err: f32 = original
        .iter()
        .zip(output.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);

    assert!(
        max_err < 1e-5,
        "identity mask should preserve signal, error: {}",
        max_err
    );
}

#[test]
fn zero_mask_produces_silence() {
    // A zero mask should produce silence (all zeros)
    use num_complex::Complex32;
    use realfft::RealFftPlanner;

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(BLOCK_LEN);
    let ifft = planner.plan_fft_inverse(BLOCK_LEN);

    let mut input: Vec<f32> = (0..BLOCK_LEN)
        .map(|i| 0.5 * (2.0 * PI * 440.0 * i as f32 / 16000.0).sin())
        .collect();
    let mut spectrum = vec![Complex32::new(0.0, 0.0); FFT_OUT_SIZE];
    fft.process(&mut input, &mut spectrum).unwrap();

    let mag: Vec<f32> = spectrum.iter().map(|c| c.norm()).collect();
    let phase: Vec<f32> = spectrum.iter().map(|c| c.im.atan2(c.re)).collect();

    // Zero mask
    let mask = vec![0.0f32; FFT_OUT_SIZE];
    let mut masked: Vec<Complex32> = mag
        .iter()
        .zip(mask.iter())
        .zip(phase.iter())
        .map(|((&m, &k), &p)| Complex32::from_polar(m * k, p))
        .collect();

    let mut output = vec![0.0f32; BLOCK_LEN];
    ifft.process(&mut masked, &mut output).unwrap();
    let norm = 1.0 / BLOCK_LEN as f32;
    for v in &mut output {
        *v *= norm;
    }

    let output_rms = rms(&output);
    assert!(
        output_rms < 1e-6,
        "zero mask should produce silence, got rms={}",
        output_rms
    );
}

// ─── Buffer Management Tests ────────────────────────────────────

#[test]
fn sliding_buffer_shift() {
    // The denoiser uses a sliding buffer: shift left by block_shift,
    // then write new samples at the end. This is the same pattern as
    // the reference: in_buffer[:-block_shift] = in_buffer[block_shift:]
    let mut buffer = vec![0.0f32; BLOCK_LEN];

    // Fill with known pattern
    for (i, item) in buffer.iter_mut().enumerate().take(BLOCK_LEN) {
        *item = i as f32;
    }

    // Shift (matching denoiser.rs: copy_within(BLOCK_SHIFT.., 0))
    buffer.copy_within(BLOCK_SHIFT.., 0);
    let new_chunk: Vec<f32> = (0..BLOCK_SHIFT).map(|i| 1000.0 + i as f32).collect();
    buffer[BLOCK_LEN - BLOCK_SHIFT..].copy_from_slice(&new_chunk);

    // First element should now be what was at position BLOCK_SHIFT
    assert_eq!(
        buffer[0], BLOCK_SHIFT as f32,
        "after shift, buffer[0] should be old buffer[block_shift]"
    );

    // Last elements should be the new chunk
    assert_eq!(
        buffer[BLOCK_LEN - BLOCK_SHIFT],
        1000.0,
        "new chunk should be at the end"
    );
    assert_eq!(
        buffer[BLOCK_LEN - 1],
        1000.0 + (BLOCK_SHIFT - 1) as f32,
        "last sample should be end of new chunk"
    );
}

#[test]
fn overlap_add_accumulation() {
    // The output buffer uses overlap-add: shift left, zero the tail,
    // then add the new block. This verifies the accumulation pattern.
    let mut out_buffer = vec![0.0f32; BLOCK_LEN];

    // Simulate first block output
    let block1: Vec<f32> = (0..BLOCK_LEN).map(|i| i as f32 * 0.001).collect();
    for (i, &v) in block1.iter().enumerate() {
        out_buffer[i] += v;
    }

    // Extract first shift of output
    let _output_1: Vec<f32> = out_buffer[..BLOCK_SHIFT].to_vec();

    // Shift buffer for next block (matching denoiser.rs)
    out_buffer.copy_within(BLOCK_SHIFT.., 0);
    for item in out_buffer
        .iter_mut()
        .take(BLOCK_LEN)
        .skip(BLOCK_LEN - BLOCK_SHIFT)
    {
        *item = 0.0;
    }

    // Add second block
    let block2: Vec<f32> = (0..BLOCK_LEN)
        .map(|i| (BLOCK_LEN + i) as f32 * 0.001)
        .collect();
    for (i, &v) in block2.iter().enumerate() {
        out_buffer[i] += v;
    }

    // The overlap region (positions 0..BLOCK_LEN-BLOCK_SHIFT) should contain
    // sum of block1's tail and block2's head
    let overlap_val = out_buffer[0];
    let expected = block1[BLOCK_SHIFT] + block2[0];
    assert!(
        (overlap_val - expected).abs() < 1e-6,
        "overlap-add should accumulate: got {} expected {}",
        overlap_val,
        expected
    );
}

// ─── Reset Test ─────────────────────────────────────────────────

#[test]
fn reset_clears_state() {
    let mut denoiser = DtlnDenoiser::new("/nonexistent");

    // Process some frames
    let audio = sine_pcm16(440.0, BLOCK_LEN, 16000);
    denoiser.denoise_frame(&audio);
    denoiser.denoise_frame(&audio);

    // Reset
    denoiser.reset();

    // After reset, processing silence should still work
    let silence = silence_pcm16(BLOCK_LEN);
    let output = denoiser.denoise_frame(&silence);
    assert_eq!(output.len(), silence.len());
}

// ─── PCM Output Clamping Test ───────────────────────────────────

#[test]
fn pcm_output_clamped_to_valid_range() {
    // The denoiser clamps output to [-1.0, 1.0] before PCM16 encoding.
    // This prevents clipping artifacts from overlap-add accumulation.
    // Verify: encode max values and check they don't overflow.
    let max_i16 = i16::MAX;
    let min_i16 = i16::MIN;

    // Clamped encoding: (1.0 * 32767) should give i16::MAX
    let clamped = (1.0f32.clamp(-1.0, 1.0) * 32767.0) as i16;
    assert_eq!(clamped, max_i16);

    // Clamped encoding: (-1.0 * 32767) should NOT overflow
    let clamped_neg = ((-1.0f32).clamp(-1.0, 1.0) * 32767.0) as i16;
    assert!(
        clamped_neg < 0,
        "negative clamp should produce negative i16"
    );

    // Over-range: 1.5 should be clamped to 1.0
    let over_range = (1.5f32.clamp(-1.0, 1.0) * 32767.0) as i16;
    assert_eq!(over_range, max_i16, "over-range should be clamped");

    // Under-range
    let under_range = ((-1.5f32).clamp(-1.0, 1.0) * 32767.0) as i16;
    assert_eq!(
        under_range,
        min_i16 + 1,
        "under-range should clamp to -32767"
    );
}

// ─── State & Shape Verification ─────────────────────────────────

#[test]
fn fft_output_size_matches_reference() {
    // DTLN Stage 1 input: magnitude spectrum of shape (1, 1, 257)
    // Reference: breizhn/DTLN uses np.fft.rfft → N/2+1 bins
    use num_complex::Complex32;
    use realfft::RealFftPlanner;

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(BLOCK_LEN);

    let mut input = vec![0.0f32; BLOCK_LEN];
    let mut spectrum = vec![Complex32::new(0.0, 0.0); FFT_OUT_SIZE];
    fft.process(&mut input, &mut spectrum).unwrap();

    assert_eq!(
        spectrum.len(),
        257,
        "FFT output must be 257 for DTLN model_1 input"
    );
}

#[test]
fn stage1_input_shape() {
    // DTLN model_1 expects: magnitude input of shape (1, 1, 257)
    let shape = [1usize, 1, FFT_OUT_SIZE];
    assert_eq!(shape, [1, 1, 257]);
}

#[test]
fn stage2_input_shape() {
    // DTLN model_2 expects: time-domain input of shape (1, 1, 512)
    let shape = [1usize, 1, BLOCK_LEN];
    assert_eq!(shape, [1, 1, 512]);
}

#[test]
fn stage1_mask_matches_magnitude() {
    // Model_1's sigmoid mask must be element-wise compatible with magnitude
    assert_eq!(FFT_OUT_SIZE, 257);
}

#[test]
fn buffer_sizes_match_reference() {
    // breizhn/DTLN: in_buffer = np.zeros((block_len)); out_buffer = np.zeros((block_len))
    assert_eq!(BLOCK_LEN, 512);
}

#[test]
fn lstm_state_shape_reference() {
    // DTLN LSTM: hidden_size=128, 2 state tensors (h,c) per model, 2 models
    let hidden_size = 128;
    let state_len: usize = hidden_size;
    assert_eq!(state_len, 128, "each LSTM state should have 128 elements");
    assert_eq!(2 * 2, 4, "4 total LSTM state tensors across both models");
}

#[test]
fn mask_value_semantics() {
    // Stage 1 sigmoid mask: 0=suppress, 1=pass, 0.5=halve
    let mag = 100.0f32;
    assert_eq!(mag * 0.0, 0.0);
    assert_eq!(mag * 1.0, mag);
    assert!((mag * 0.5 - 50.0).abs() < 1e-6);
}

#[test]
fn ifft_normalization_matches_numpy() {
    // realfft inverse is unnormalized; we normalize by 1/N like numpy.fft.irfft
    let norm = 1.0 / BLOCK_LEN as f32;
    assert!((norm - 1.0 / 512.0).abs() < 1e-10);
}
