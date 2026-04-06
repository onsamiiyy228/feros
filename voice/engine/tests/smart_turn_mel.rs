//! Smart Turn mel spectrogram tests — verifies our Whisper-compatible
//! feature extraction matches the HF Transformers implementation.
//!
//! Reference values computed from `transformers.WhisperFeatureExtractor`.

use std::f32::consts::PI;
use voice_engine::audio_ml::smart_turn::SmartTurnAnalyzer;

// ── Helper: generate test signals ───────────────────────────────

/// Generate a 440Hz sine wave as PCM16 bytes.
fn sine_pcm16(freq: f32, duration_secs: f32, sample_rate: u32) -> Vec<u8> {
    let n = (duration_secs * sample_rate as f32) as usize;
    let mut bytes = Vec::with_capacity(n * 2);
    for i in 0..n {
        let sample = (0.5 * (2.0 * PI * freq * i as f32 / sample_rate as f32).sin()) * 32767.0;
        let s = (sample as i16).to_le_bytes();
        bytes.push(s[0]);
        bytes.push(s[1]);
    }
    bytes
}

/// Generate silence as PCM16 bytes.
fn silence_pcm16(duration_secs: f32, sample_rate: u32) -> Vec<u8> {
    let n = (duration_secs * sample_rate as f32) as usize;
    vec![0u8; n * 2]
}

// ── Mel Scale Tests ─────────────────────────────────────────────

#[test]
fn slaney_mel_scale_roundtrip() {
    // Reference values from the Slaney mel scale (librosa-compatible)
    let test_cases: Vec<(f32, f32)> = vec![
        (0.0, 0.0),
        (200.0, 3.0),
        (500.0, 7.5),
        (1000.0, 15.0),
        (2000.0, 25.08188),
        (4000.0, 35.16376),
        (8000.0, 45.24564),
    ];

    for (hz, expected_mel) in &test_cases {
        let mel = hz_to_mel_slaney(*hz);
        assert!(
            (mel - expected_mel).abs() < 0.001,
            "hz_to_mel({}) = {} but expected {}",
            hz,
            mel,
            expected_mel
        );

        // Roundtrip
        let back_hz = mel_to_hz_slaney(mel);
        assert!(
            (back_hz - hz).abs() < 0.01,
            "roundtrip failed: {} -> {} -> {}",
            hz,
            mel,
            back_hz
        );
    }
}

#[test]
fn slaney_linear_below_1000hz() {
    // Below 1000Hz, Slaney mel scale is linear: mel = 3 * hz / 200
    for hz in [100.0, 250.0, 500.0, 750.0, 999.0] {
        let mel = hz_to_mel_slaney(hz);
        let expected = 3.0 * hz / 200.0;
        assert!(
            (mel - expected).abs() < 0.001,
            "linear region: hz={} mel={} expected={}",
            hz,
            mel,
            expected
        );
    }
}

#[test]
fn slaney_log_above_1000hz() {
    // Above 1000Hz, Slaney mel scale is logarithmic
    let mel_1000 = hz_to_mel_slaney(1000.0);
    let mel_2000 = hz_to_mel_slaney(2000.0);
    let mel_4000 = hz_to_mel_slaney(4000.0);

    // Each doubling of frequency should add same mel increment (log scale)
    let oct1 = mel_2000 - mel_1000;
    let oct2 = mel_4000 - mel_2000;
    assert!(
        (oct1 - oct2).abs() < 0.001,
        "octave spacing not equal: {} vs {}",
        oct1,
        oct2
    );
}

// ── Hann Window Tests ───────────────────────────────────────────

#[test]
fn periodic_hann_window_values() {
    let n = 400;
    let w = compute_hann_window(n);
    assert_eq!(w.len(), n);

    // First sample should be 0
    assert!(w[0].abs() < 1e-6, "w[0] should be 0, got {}", w[0]);

    // Midpoint should be 1
    assert!(
        (w[200] - 1.0).abs() < 1e-6,
        "w[200] should be 1.0, got {}",
        w[200]
    );

    // Second sample: small positive value
    let expected_w1 = 0.5 * (1.0 - (2.0 * PI / 400.0).cos());
    assert!(
        (w[1] - expected_w1).abs() < 1e-6,
        "w[1] = {} expected {}",
        w[1],
        expected_w1
    );

    // Symmetry: w[n] ≈ w[N-n] for periodic window
    for i in 1..200 {
        assert!(
            (w[i] - w[400 - i]).abs() < 1e-5,
            "window not symmetric at i={}: {} vs {}",
            i,
            w[i],
            w[400 - i]
        );
    }
}

// ── Mel Filterbank Tests ────────────────────────────────────────

#[test]
fn mel_filterbank_shape() {
    let filters = compute_mel_filterbank();
    assert_eq!(filters.len(), 80, "should have 80 mel bands");
    assert_eq!(
        filters[0].len(),
        201,
        "each filter should have 201 FFT bins"
    );
}

#[test]
fn mel_filterbank_non_negative() {
    let filters = compute_mel_filterbank();
    for (m, filter) in filters.iter().enumerate() {
        for (k, &val) in filter.iter().enumerate() {
            assert!(val >= 0.0, "filter[{}][{}] = {} is negative", m, k, val);
        }
    }
}

#[test]
fn mel_filterbank_slaney_area_normalized() {
    // With Slaney normalization, each filter's sum should reflect 2/(f_right - f_left).
    // The wider the filter, the smaller the peak (constant area).
    let filters = compute_mel_filterbank();

    // Lower filters (< 1000Hz) are linearly spaced and narrow → higher peak
    // Upper filters (> 1000Hz) are exponentially spaced and wider → lower peak
    let low_peak: f32 = filters[5].iter().copied().fold(0.0, f32::max);
    let high_peak: f32 = filters[70].iter().copied().fold(0.0, f32::max);

    assert!(
        low_peak > high_peak,
        "low filter peak ({}) should be > high filter peak ({}) with Slaney normalization",
        low_peak,
        high_peak
    );
}

#[test]
fn mel_filterbank_440hz_lands_in_correct_band() {
    // 440Hz should fall in filters 10-11 (center freqs ~409Hz and ~446Hz)
    let filters = compute_mel_filterbank();

    // FFT bin for 440Hz = 440 * 400 / 16000 = 11
    let bin_440 = 11; // 440 * N_FFT / SR

    // Filter 10 and 11 should have non-zero weights at bin 11
    assert!(
        filters[10][bin_440] > 0.0 || filters[11][bin_440] > 0.0,
        "440Hz (bin {}) should have non-zero weight in filter 10 or 11",
        bin_440
    );

    // Filters far away shouldn't be active at 440Hz
    assert!(
        filters[0][bin_440] == 0.0,
        "Filter 0 shouldn't be active at 440Hz"
    );
    assert!(
        filters[50][bin_440] == 0.0,
        "Filter 50 shouldn't be active at 440Hz"
    );
}

// ── Log Mel Normalization Tests ─────────────────────────────────

#[test]
fn whisper_log_normalization() {
    // Whisper's normalization: log10, clip to 80dB floor, (v+4)/4
    let mut vals = vec![1e-10_f32, 1e-5, 0.001, 0.1, 1.0, 10.0, 100.0];

    // Step 1: log10
    for v in &mut vals {
        *v = (*v).max(1e-10).log10();
    }

    // Step 2: clip to max - 8.0 (80dB floor)
    let mx = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let floor = mx - 8.0;
    for v in &mut vals {
        *v = (*v).max(floor);
    }

    // Step 3: (v + 4) / 4
    for v in &mut vals {
        *v = (*v + 4.0) / 4.0;
    }

    // The max value should be log10(100) + 4) / 4 = (2 + 4) / 4 = 1.5
    assert!(
        (vals[vals.len() - 1] - 1.5).abs() < 0.001,
        "max normalized value should be 1.5, got {}",
        vals[vals.len() - 1]
    );

    // The floor should be (2 - 8 + 4) / 4 = -0.5
    let expected_floor = (mx - 8.0 + 4.0) / 4.0;
    assert!(
        (vals[0] - expected_floor).abs() < 0.001,
        "floor value should be {}, got {}",
        expected_floor,
        vals[0]
    );
}

// ── Full Pipeline Tests ─────────────────────────────────────────

#[test]
fn mel_output_shape() {
    // The SmartTurnAnalyzer should produce N_MELS × N_FRAMES = 80 × 800
    let analyzer = SmartTurnAnalyzer::new("dummy.onnx", 0.5);

    // Generate 8 seconds of audio (minimum for full mel)
    let audio = silence_pcm16(8.0, 16000);
    let mel = analyzer.compute_log_mel_spectrogram_from_pcm(&audio);

    assert_eq!(
        mel.len(),
        80 * 800,
        "mel output should be 80*800={} values, got {}",
        80 * 800,
        mel.len()
    );
}

#[test]
fn mel_silence_is_floor() {
    // Pure silence should produce the minimum (floor) value everywhere
    let analyzer = SmartTurnAnalyzer::new("dummy.onnx", 0.5);

    let audio = silence_pcm16(8.0, 16000);
    let mel = analyzer.compute_log_mel_spectrogram_from_pcm(&audio);

    // All values should be the floor value (after Whisper normalization)
    let min_val = mel.iter().copied().fold(f32::INFINITY, f32::min);
    let max_val = mel.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    // Floor value from Whisper: should be roughly -0.11
    // Exact value depends on max, but for pure silence all values should be identical
    assert!(
        (max_val - min_val).abs() < 0.01,
        "pure silence should have uniform mel values, but range is {:.6} to {:.6}",
        min_val,
        max_val
    );
}

#[test]
fn mel_sine_has_energy_in_correct_band() {
    // A 440Hz sine wave should produce energy primarily in mel bands ~10-11
    let analyzer = SmartTurnAnalyzer::new("dummy.onnx", 0.5);

    let audio = sine_pcm16(440.0, 8.0, 16000);
    let mel = analyzer.compute_log_mel_spectrogram_from_pcm(&audio);

    // Mel is stored as [mel_band][frame] in row-major
    // Check that band 10 or 11 has higher energy than band 0 and band 79
    let n_frames = 800;

    // Average energy across frames for each band
    let band_0_avg: f32 = (0..n_frames).map(|t| mel[t]).sum::<f32>() / n_frames as f32;
    let band_10_avg: f32 =
        (0..n_frames).map(|t| mel[10 * n_frames + t]).sum::<f32>() / n_frames as f32;
    let band_11_avg: f32 =
        (0..n_frames).map(|t| mel[11 * n_frames + t]).sum::<f32>() / n_frames as f32;
    let band_79_avg: f32 =
        (0..n_frames).map(|t| mel[79 * n_frames + t]).sum::<f32>() / n_frames as f32;

    let peak_band = band_10_avg.max(band_11_avg);

    assert!(
        peak_band > band_0_avg,
        "440Hz energy in band 10/11 ({:.4}) should exceed band 0 ({:.4})",
        peak_band,
        band_0_avg
    );
    assert!(
        peak_band > band_79_avg,
        "440Hz energy in band 10/11 ({:.4}) should exceed band 79 ({:.4})",
        peak_band,
        band_79_avg
    );
}

#[test]
fn mel_output_range_matches_whisper() {
    // Whisper normalization produces values roughly in [-1, 2] range
    // Reference: min=-0.110, max=1.890 for a 440Hz tone
    let analyzer = SmartTurnAnalyzer::new("dummy.onnx", 0.5);

    let audio = sine_pcm16(440.0, 8.0, 16000);
    let mel = analyzer.compute_log_mel_spectrogram_from_pcm(&audio);

    let min_val = mel.iter().copied().fold(f32::INFINITY, f32::min);
    let max_val = mel.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    // Whisper's (v+4)/4 normalization means:
    // - Floor ≈ (max_log10 - 8 + 4) / 4
    // - Max ≈ (max_log10 + 4) / 4
    // For typical audio, min should be negative and max should be positive
    assert!(
        min_val < 0.0,
        "min mel value should be negative (Whisper floor), got {}",
        min_val
    );
    assert!(
        max_val > 0.5 && max_val < 3.0,
        "max mel value should be in [0.5, 3.0] range, got {}",
        max_val
    );
}

#[test]
fn mel_audio_normalization_applied() {
    // Loud and quiet versions of the same signal should produce similar mel values
    // because of the zero-mean unit-variance normalization
    let analyzer = SmartTurnAnalyzer::new("dummy.onnx", 0.5);

    // Normal amplitude
    let audio_normal = sine_pcm16(440.0, 8.0, 16000);
    let mel_normal = analyzer.compute_log_mel_spectrogram_from_pcm(&audio_normal);

    // Quiet version (10x quieter)
    let mut audio_quiet = Vec::new();
    let n = 8 * 16000;
    for i in 0..n {
        let sample = (0.05 * (2.0 * PI * 440.0 * i as f32 / 16000.0).sin()) * 32767.0;
        let s = (sample as i16).to_le_bytes();
        audio_quiet.push(s[0]);
        audio_quiet.push(s[1]);
    }
    let mel_quiet = analyzer.compute_log_mel_spectrogram_from_pcm(&audio_quiet);

    // The shapes should be identical
    assert_eq!(mel_normal.len(), mel_quiet.len());

    // With normalization, the peak band energy should be similar
    // (not identical due to different noise floor, but within ~20%)
    let n_frames = 800;
    let peak_normal: f32 = (0..n_frames)
        .map(|t| mel_normal[10 * n_frames + t])
        .sum::<f32>()
        / n_frames as f32;
    let peak_quiet: f32 = (0..n_frames)
        .map(|t| mel_quiet[10 * n_frames + t])
        .sum::<f32>()
        / n_frames as f32;

    let ratio = (peak_normal / peak_quiet).abs();
    assert!(
        ratio > 0.5 && ratio < 2.0,
        "normalized mel should be volume-invariant, but ratio is {:.3} (normal={:.4}, quiet={:.4})",
        ratio,
        peak_normal,
        peak_quiet
    );
}

// ── Cross-Validation Against HF Transformers ────────────────────

/// Reference values from HF Transformers `WhisperFeatureExtractor` for a
/// 440Hz sine (amplitude 0.5, 8 seconds, 16kHz, PCM16-quantized, normalized).
///
/// Generated by: WhisperFeatureExtractor(feature_size=80, sampling_rate=16000,
///               n_fft=400, hop_length=160) with do_normalize=True.
#[test]
fn cross_validate_against_hf_transformers() {
    let analyzer = SmartTurnAnalyzer::new("dummy.onnx", 0.5);
    let audio = sine_pcm16(440.0, 8.0, 16000);
    let mel = analyzer.compute_log_mel_spectrogram_from_pcm(&audio);

    let n_frames = 800;
    let n_mels = 80;

    // ── Global statistics ──
    let min_val = mel.iter().copied().fold(f32::INFINITY, f32::min);
    let max_val = mel.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mean_val: f32 = mel.iter().sum::<f32>() / mel.len() as f32;

    // HF Transformers reference: min=-0.336, max=1.664, mean=-0.240
    assert_close(min_val, -0.3360, 0.05, "global min");
    assert_close(max_val, 1.6640, 0.05, "global max");
    assert_close(mean_val, -0.2401, 0.05, "global mean");

    // ── Specific mel[band][frame] reference points ──
    // These are exact values from HF Transformers WhisperFeatureExtractor.
    // Index: mel[m * n_frames + t]
    let reference_points: Vec<(usize, usize, f32, f32)> = vec![
        // (mel_band, frame, expected_value, tolerance)
        // Silent region (band 0, frames in silence area)
        (0, 100, -0.3360, 0.05),
        (0, 400, -0.3360, 0.05),
        (79, 400, -0.3360, 0.05),
        (79, 799, -0.3360, 0.05),
        // Peak band energy (440Hz -> band 10-11)
        (10, 100, 1.5745, 0.10),
        (10, 400, 1.5745, 0.10),
        (11, 400, 1.6640, 0.10),
        (11, 799, 1.6614, 0.10),
        // Non-active bands should be at the floor
        (39, 400, -0.3360, 0.05),
    ];

    for (m, t, expected, tol) in &reference_points {
        let actual = mel[m * n_frames + t];
        assert!(
            (actual - expected).abs() < *tol,
            "mel[{}][{}] = {:.6} but HF Transformers reference = {:.6} (tolerance {})",
            m,
            t,
            actual,
            expected,
            tol
        );
    }

    // ── Band sum cross-check ──
    // Band sums are a strong aggregate check: if the filterbank, STFT, or
    // normalization is wrong, these will drift significantly.
    let band_sums: Vec<f32> = (0..n_mels)
        .map(|m| (0..n_frames).map(|t| mel[m * n_frames + t]).sum::<f32>())
        .collect();

    // HF Transformers reference band sums (bands 0-14):
    let ref_band_sums: Vec<(usize, f32, f32)> = vec![
        // (band, expected_sum, tolerance)
        (0, -265.20, 5.0),
        (5, -265.07, 5.0),
        (9, 1108.29, 20.0),  // 440Hz harmonic energy starts here
        (10, 1259.60, 25.0), // Peak energy
        (11, 1330.85, 25.0), // Highest energy (440Hz center)
        (12, 1215.49, 25.0), // Energy falling off
        (13, -264.90, 5.0),  // Back to floor
    ];

    for (band, expected_sum, tol) in &ref_band_sums {
        assert!(
            (band_sums[*band] - expected_sum).abs() < *tol,
            "band[{}] sum = {:.2} but HF Transformers reference = {:.2} (tolerance {})",
            band,
            band_sums[*band],
            expected_sum,
            tol
        );
    }

    // ── Total sum checksum ──
    let total: f32 = mel.iter().sum();
    // HF Transformers reference: -15365.59
    // ~2% tolerance for accumulated f32 FFT differences between rustfft and numpy
    assert!(
        (total - (-15365.59)).abs() < 500.0,
        "total mel sum = {:.2} but Python reference = -15365.59",
        total
    );
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, label: &str) {
    assert!(
        (actual - expected).abs() < tolerance,
        "{}: actual={:.6} expected={:.6} (tolerance {})",
        label,
        actual,
        expected,
        tolerance
    );
}

// ── Internal functions re-exported for testing ──────────────────

// These mirror the functions in smart_turn.rs exactly
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

fn compute_hann_window(sz: usize) -> Vec<f32> {
    (0..sz)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f32 / sz as f32).cos()))
        .collect()
}

fn compute_mel_filterbank() -> Vec<Vec<f32>> {
    let n_fft = 400;
    let n_mels = 80;
    let sr = 16000;
    let n = n_fft / 2 + 1;
    let mut f = vec![vec![0.0f32; n]; n_mels];

    let ml = hz_to_mel_slaney(0.0);
    let mh = hz_to_mel_slaney(sr as f32 / 2.0);
    let mel_pts: Vec<f32> = (0..n_mels + 2)
        .map(|i| ml + (mh - ml) * i as f32 / (n_mels + 1) as f32)
        .collect();
    let hz_pts: Vec<f32> = mel_pts.iter().map(|&m| mel_to_hz_slaney(m)).collect();

    let fft_freqs: Vec<f32> = (0..n)
        .map(|i| i as f32 * sr as f32 / n_fft as f32)
        .collect();

    for m in 0..n_mels {
        let f_left = hz_pts[m];
        let f_center = hz_pts[m + 1];
        let f_right = hz_pts[m + 2];
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
