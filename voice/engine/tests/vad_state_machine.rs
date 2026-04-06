//! VAD state machine tests — verifies speech detection logic
//! without requiring the ONNX model file.

use voice_engine::audio_ml::vad::{SileroVad, VadConfig, VadEvent};

/// Create a loud 512-sample frame (amplitude ~5000) that passes the volume gate.
fn loud_frame() -> Vec<u8> {
    let mut frame = vec![0u8; 1024]; // 512 samples × 2 bytes
    for i in 0..512 {
        let sample: i16 =
            ((5000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16000.0).sin()) as i16)
                .clamp(-32767, 32767);
        let bytes = sample.to_le_bytes();
        frame[i * 2] = bytes[0];
        frame[i * 2 + 1] = bytes[1];
    }
    frame
}

/// Create a silent 512-sample frame (all zeros).
fn silent_frame() -> Vec<u8> {
    vec![0u8; 1024]
}

/// Helper: feed N frames with given probability.
fn feed(vad: &mut SileroVad, prob: f32, frame: &[u8], count: usize) -> Vec<VadEvent> {
    let mut events = Vec::new();
    for _ in 0..count {
        if let Some(evt) = vad.process_with_prob(prob, frame) {
            events.push(evt);
        }
    }
    events
}

fn test_vad() -> SileroVad {
    // Use low min_volume so our synthetic frames pass the gate
    let config = VadConfig {
        threshold: 0.7,
        min_volume: 0.001,
        silence_frames: 6,
        min_speech_frames: 3,
        lookback_frames: 20,
        context_size: 64,
    };
    SileroVad::new("dummy.onnx", config)
}

// ── Speech Start Tests ──────────────────────────────────────────

#[test]
fn speech_start_after_min_frames() {
    let mut vad = test_vad();
    let frame = loud_frame();

    // 2 speech frames → no event yet
    let events = feed(&mut vad, 0.9, &frame, 2);
    assert!(
        events.is_empty(),
        "should not fire before min_speech_frames"
    );
    assert!(!vad.is_speaking());

    // 3rd speech frame → SpeechStart
    let evt = vad.process_with_prob(0.9, &frame);
    assert_eq!(evt, Some(VadEvent::SpeechStart));
    assert!(vad.is_speaking());
}

#[test]
fn speech_start_cancelled_by_silence() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    // 2 speech frames, then silence
    feed(&mut vad, 0.9, &loud, 2);
    assert!(!vad.is_speaking());

    // Silence resets the counter
    let events = feed(&mut vad, 0.1, &silent, 1);
    assert!(events.is_empty());

    // Need 3 more speech frames from scratch
    let events = feed(&mut vad, 0.9, &loud, 2);
    assert!(events.is_empty());
    assert!(!vad.is_speaking());

    let evt = vad.process_with_prob(0.9, &loud);
    assert_eq!(evt, Some(VadEvent::SpeechStart));
}

// ── Speech End Tests ────────────────────────────────────────────

#[test]
fn speech_end_after_silence_frames() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    // Start speaking
    feed(&mut vad, 0.9, &loud, 3);
    assert!(vad.is_speaking());

    // 5 silence frames → no end yet
    let events = feed(&mut vad, 0.1, &silent, 5);
    assert!(events.is_empty());
    assert!(vad.is_speaking());

    // 6th silence frame → SpeechEnd
    let evt = vad.process_with_prob(0.1, &silent);
    assert_eq!(evt, Some(VadEvent::SpeechEnd));
    assert!(!vad.is_speaking());
}

#[test]
fn brief_pause_absorbed() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    // Start speaking
    feed(&mut vad, 0.9, &loud, 3);
    assert!(vad.is_speaking());

    // Brief 3-frame pause (less than silence_frames=6)
    let events = feed(&mut vad, 0.1, &silent, 3);
    assert!(events.is_empty());
    assert!(vad.is_speaking());

    // Resume speaking — silence counter should reset
    feed(&mut vad, 0.9, &loud, 1);
    assert!(vad.is_speaking());

    // Another 5-frame pause — still not enough
    let events = feed(&mut vad, 0.1, &silent, 5);
    assert!(events.is_empty());
    assert!(vad.is_speaking());
}

#[test]
fn cancel_stop_on_speech_resume() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    // Start speaking
    feed(&mut vad, 0.9, &loud, 3);
    assert!(vad.is_speaking());

    // 5 silence frames — almost at threshold
    feed(&mut vad, 0.1, &silent, 5);
    assert!(vad.is_speaking());

    // Resume speaking — resets silence counter
    feed(&mut vad, 0.9, &loud, 1);

    // Now need full 6 silence frames again
    let events = feed(&mut vad, 0.1, &silent, 5);
    assert!(events.is_empty());
    assert!(vad.is_speaking());

    // 6th → SpeechEnd
    let evt = vad.process_with_prob(0.1, &silent);
    assert_eq!(evt, Some(VadEvent::SpeechEnd));
}

// ── Volume Gate Tests ───────────────────────────────────────────

#[test]
fn volume_gate_blocks_quiet_speech() {
    let mut vad = test_vad();
    let silent = silent_frame(); // RMS ≈ 0

    // High probability but zero volume → should NOT detect speech
    let events = feed(&mut vad, 0.99, &silent, 10);
    assert!(events.is_empty());
    assert!(!vad.is_speaking());
}

#[test]
fn volume_gate_passes_loud_speech() {
    let mut vad = test_vad();
    let loud = loud_frame();

    // High probability AND loud volume → speech detected
    let events = feed(&mut vad, 0.9, &loud, 3);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], VadEvent::SpeechStart);
}

// ── Lookback Buffer Tests ───────────────────────────────────────

#[test]
fn lookback_buffer_included_in_speech_audio() {
    let mut vad = test_vad();
    let loud = loud_frame();

    // Feed 5 frames of silence (with volume — they go to lookback)
    for _ in 0..5 {
        vad.process_with_prob(0.1, &loud);
    }

    // Now trigger speech start
    feed(&mut vad, 0.9, &loud, 3);

    // speech_audio should include lookback + speech frames
    let audio = vad.get_speech_audio();
    // 5 lookback frames + 3 speech frames = 8 frames × 1024 bytes each
    assert_eq!(audio.len(), 8 * 1024);
}

#[test]
fn lookback_buffer_capped_at_max() {
    let config = VadConfig {
        lookback_frames: 3,
        min_volume: 0.001,
        min_speech_frames: 3,
        ..VadConfig::default()
    };
    let mut vad = SileroVad::new("dummy.onnx", config);
    let loud = loud_frame();

    // Feed 10 frames of silence
    for _ in 0..10 {
        vad.process_with_prob(0.1, &loud);
    }

    // Trigger speech (3 frames during STARTING phase push into lookback too,
    // but cap is 3 so only last 3 are kept; then trigger frame is appended)
    feed(&mut vad, 0.9, &loud, 3);

    // lookback(3 frames) + trigger frame(1) = 4 frames
    let audio = vad.get_speech_audio();
    assert_eq!(audio.len(), 4 * 1024);
}

// ── Force End Tests ─────────────────────────────────────────────

#[test]
fn force_speech_end_resets_state() {
    let mut vad = test_vad();
    let loud = loud_frame();

    // Start speaking
    feed(&mut vad, 0.9, &loud, 3);
    assert!(vad.is_speaking());

    // Force end (as if SmartTurnComplete)
    vad.force_speech_end();
    assert!(!vad.is_speaking());
}

// ── Event Sequence Tests ────────────────────────────────────────

#[test]
fn full_utterance_produces_start_then_end() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    let mut all_events = Vec::new();

    // Speech
    all_events.extend(feed(&mut vad, 0.9, &loud, 10));
    // Silence
    all_events.extend(feed(&mut vad, 0.1, &silent, 10));

    assert_eq!(all_events.len(), 2);
    assert_eq!(all_events[0], VadEvent::SpeechStart);
    assert_eq!(all_events[1], VadEvent::SpeechEnd);
}

#[test]
fn two_utterances_with_gap() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    let mut all_events = Vec::new();

    // First utterance
    all_events.extend(feed(&mut vad, 0.9, &loud, 5));
    all_events.extend(feed(&mut vad, 0.1, &silent, 10));

    // Second utterance
    all_events.extend(feed(&mut vad, 0.9, &loud, 5));
    all_events.extend(feed(&mut vad, 0.1, &silent, 10));

    assert_eq!(all_events.len(), 4);
    assert_eq!(all_events[0], VadEvent::SpeechStart);
    assert_eq!(all_events[1], VadEvent::SpeechEnd);
    assert_eq!(all_events[2], VadEvent::SpeechStart);
    assert_eq!(all_events[3], VadEvent::SpeechEnd);
}

#[test]
fn speech_with_brief_pause_produces_single_utterance() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    let mut all_events = Vec::new();

    // "I want to" [brief pause] "order pizza"
    all_events.extend(feed(&mut vad, 0.9, &loud, 5)); // "I want to"
    all_events.extend(feed(&mut vad, 0.1, &silent, 3)); // brief pause (< 6 frames)
    all_events.extend(feed(&mut vad, 0.9, &loud, 5)); // "order pizza"
    all_events.extend(feed(&mut vad, 0.1, &silent, 10)); // final silence

    // Should be ONE utterance, not two
    assert_eq!(all_events.len(), 2);
    assert_eq!(all_events[0], VadEvent::SpeechStart);
    assert_eq!(all_events[1], VadEvent::SpeechEnd);
}

#[test]
fn boundary_pause_exactly_at_threshold_produces_two_utterances() {
    let mut vad = test_vad();
    let loud = loud_frame();
    let silent = silent_frame();

    let mut all_events = Vec::new();

    // Speech then exactly 6 frames silence (= silence_frames threshold)
    all_events.extend(feed(&mut vad, 0.9, &loud, 5));
    all_events.extend(feed(&mut vad, 0.1, &silent, 6)); // exactly at threshold
    all_events.extend(feed(&mut vad, 0.9, &loud, 5));
    all_events.extend(feed(&mut vad, 0.1, &silent, 10));

    // Should be TWO utterances
    assert_eq!(all_events.len(), 4);
    assert_eq!(all_events[0], VadEvent::SpeechStart);
    assert_eq!(all_events[1], VadEvent::SpeechEnd);
    assert_eq!(all_events[2], VadEvent::SpeechStart);
    assert_eq!(all_events[3], VadEvent::SpeechEnd);
}
