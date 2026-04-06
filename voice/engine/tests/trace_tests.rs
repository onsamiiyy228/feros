//! Tests for the voice-trace crate — EventBus, TurnTracker, and Tracer.

use std::collections::HashSet;
use std::time::Duration;

use voice_trace::{Event, EventBus, EventCategory, Tracer, TurnTracker};

// ── EventBus ─────────────────────────────────────────────────────

#[test]
fn bus_emit_without_subscribers_does_not_panic() {
    let bus = EventBus::new();
    bus.emit(Event::Trace {
        seq: 0,
        elapsed_us: 0,
        label: "Test".to_string(),
    });
    // No subscriber — should silently drop.
}

#[tokio::test]
async fn bus_subscriber_receives_events() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    bus.emit(Event::Trace {
        seq: 1,
        elapsed_us: 100,
        label: "SpeechStarted".to_string(),
    });

    let event = rx.recv().await.unwrap();
    match event {
        Event::Trace { seq, label, .. } => {
            assert_eq!(seq, 1);
            assert_eq!(label, "SpeechStarted");
        }
        _ => panic!("Expected Trace event"),
    }
}

#[tokio::test]
async fn bus_multiple_subscribers() {
    let bus = EventBus::new();
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();

    bus.emit(Event::Trace {
        seq: 42,
        elapsed_us: 0,
        label: "BargeIn".to_string(),
    });

    // Both subscribers should receive the event.
    let e1 = rx1.recv().await.unwrap();
    let e2 = rx2.recv().await.unwrap();
    match (e1, e2) {
        (Event::Trace { seq: s1, .. }, Event::Trace { seq: s2, .. }) => {
            assert_eq!(s1, 42);
            assert_eq!(s2, 42);
        }
        _ => panic!("Expected Trace events"),
    }
}

#[tokio::test]
async fn bus_filtered_subscriber_only_receives_matching_categories() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe_filtered(HashSet::from([EventCategory::Metrics]));

    // Emit a Trace event (should be filtered out)
    bus.emit(Event::Trace {
        seq: 1,
        elapsed_us: 0,
        label: "ignored".to_string(),
    });

    // Emit a TurnMetrics event (should be received)
    bus.emit(Event::TurnMetrics(voice_trace::TurnMetrics {
        turn_id: 1,
        vad_silence_ms: 192.0,
        eou_delay_ms: 0.0,
        stt_ms: 10.0,
        llm_first_token_ms: 20.0,
        tts_first_audio_ms: 5.0,
        ttfa_ms: 35.0,
        total_ms: 50.0,
        tts_duration_ms: 40.0,
        user_agent_latency_ms: Some(227.0),
        input_audio_duration_ms: 1500.0,
        output_audio_duration_ms: 2000.0,
        stt_total_duration_ms: 10.0,
        stt_ttfb_ms: Some(5.0),
        tts_ttfb_ms: Some(3.0),
        tts_total_duration_ms: 40.0,
        text_aggregation_ms: Some(2.0),
    }));

    let event = rx.recv().await.unwrap();
    match event {
        Event::TurnMetrics(m) => {
            assert_eq!(m.turn_id, 1);
        }
        _ => panic!("Expected TurnMetrics event"),
    }
}

// ── TurnTracker ──────────────────────────────────────────────────

#[test]
fn turn_tracker_finish_without_start_returns_none() {
    let mut tracker = TurnTracker::new();
    // Never called mark_speech_ended — finish should return None.
    assert!(tracker.finish(1).is_none());
}

#[test]
fn turn_tracker_basic_turn() {
    let mut tracker = TurnTracker::new();

    tracker.mark_speech_ended();
    std::thread::sleep(Duration::from_millis(5));
    tracker.mark_transcript();
    std::thread::sleep(Duration::from_millis(5));
    tracker.mark_llm_first_token();
    tracker.mark_tts_start();
    std::thread::sleep(Duration::from_millis(5));
    tracker.mark_tts_first_audio();

    let metrics = tracker.finish(1).expect("Should return metrics");

    assert_eq!(metrics.turn_id, 1);
    assert!(metrics.stt_ms > 0.0);
    assert!(metrics.llm_first_token_ms > 0.0);
    assert!(metrics.tts_first_audio_ms > 0.0);
    assert!(metrics.ttfa_ms > 0.0);
    assert!(metrics.total_ms >= metrics.ttfa_ms);
}

#[test]
fn turn_tracker_increments_turn_id() {
    let mut tracker = TurnTracker::new();

    // Turn 1
    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();
    tracker.mark_tts_start();
    tracker.mark_tts_first_audio();
    let m1 = tracker.finish(1).unwrap();
    assert_eq!(m1.turn_id, 1);

    // Turn 2
    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();
    tracker.mark_tts_start();
    tracker.mark_tts_first_audio();
    let m2 = tracker.finish(2).unwrap();
    assert_eq!(m2.turn_id, 2);
}

#[test]
fn turn_tracker_reset_clears_state() {
    let mut tracker = TurnTracker::new();

    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.reset();

    // After reset, finish should return None (no active turn).
    assert!(tracker.finish(1).is_none());
}

#[test]
fn turn_tracker_llm_first_token_only_recorded_once() {
    let mut tracker = TurnTracker::new();

    tracker.mark_speech_ended();
    tracker.mark_transcript();

    // First call sets the timestamp
    tracker.mark_llm_first_token();
    std::thread::sleep(Duration::from_millis(10));
    // Second call should be a no-op
    tracker.mark_llm_first_token();

    tracker.mark_tts_start();
    tracker.mark_tts_first_audio();
    let metrics = tracker.finish(1).unwrap();

    // llm_first_token_ms should be small (first call was right after transcript)
    // not re-set by the second call 10ms later
    assert!(metrics.llm_first_token_ms < 5.0);
}

#[test]
fn turn_tracker_tts_first_audio_only_recorded_once() {
    let mut tracker = TurnTracker::new();

    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();

    tracker.mark_tts_start();
    tracker.mark_tts_first_audio();
    std::thread::sleep(Duration::from_millis(10));
    tracker.mark_tts_first_audio(); // should be ignored

    let metrics = tracker.finish(1).unwrap();
    // tts_first_audio_ms should reflect only the first mark
    assert!(metrics.tts_first_audio_ms < 5.0);
}

#[test]
fn turn_tracker_partial_turn_missing_tts() {
    let mut tracker = TurnTracker::new();

    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();
    // No TTS audio — e.g. tool-call-only turn

    let metrics = tracker.finish(1).unwrap();
    assert_eq!(metrics.turn_id, 1);
    assert_eq!(metrics.tts_first_audio_ms, 0.0);
    assert_eq!(metrics.ttfa_ms, 0.0);
    assert!(metrics.total_ms > 0.0);
}

// ── Tracer integration ───────────────────────────────────────────

#[tokio::test]
async fn tracer_finish_turn_emits_to_bus() {
    let mut tracer = Tracer::new();
    let mut rx = tracer.subscribe();

    // Mirror real pipeline flow:
    // 1. VAD SpeechStarted → mark_stt_audio_start
    tracer.mark_stt_audio_start();

    // 2. VAD SpeechEnded → mark_vad_speech_ended + mark_stt_finalize_sent
    tracer.mark_vad_speech_ended();
    tracer.mark_stt_finalize_sent();

    // 3. STT FirstTextReceived → mark_stt_first_text
    tracer.mark_stt_first_text();

    // 4. STT Transcript → mark_transcript + mark_speech_ended
    tracer.mark_transcript();
    tracer.mark_speech_ended();

    // 5. start_turn (emits TurnStarted + SttComplete)
    tracer.start_turn("deepgram", "nova-2", "hello world", "en", true);

    // 6. LLM first token
    tracer.mark_llm_first_token();

    // 7. TTS lifecycle
    tracer.mark_tts_start();
    tracer.mark_tts_text_fed();
    tracer.append_tts_text("Hello!");
    tracer.mark_tts_first_audio();

    // 8. finish_turn (emits TtsComplete + TurnMetrics + TurnEnded)
    tracer.finish_turn(false, "cartesia", "sonic", "voice-1");

    // The bus should have received trace events + TurnMetrics.
    // Drain until we find TurnMetrics.
    loop {
        let event = rx.recv().await.unwrap();
        if let Event::TurnMetrics(m) = event {
            assert_eq!(m.turn_id, 1);
            assert!(m.ttfa_ms > 0.0);
            assert!(
                m.stt_total_duration_ms > 0.0,
                "stt_total_duration_ms should be > 0"
            );
            break;
        }
    }
}

// ── VAD backdating & new metrics ─────────────────────────────────

#[test]
fn turn_tracker_vad_silence_ms_stored() {
    let mut tracker = TurnTracker::new();

    tracker.set_vad_silence_ms(192.0);
    tracker.mark_vad_speech_ended();
    // Small sleep so speech_ended is measurably after the backdated vad timestamp
    std::thread::sleep(Duration::from_millis(5));
    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();
    tracker.mark_tts_start();
    tracker.mark_tts_first_audio();

    let metrics = tracker.finish(1).unwrap();
    assert!((metrics.vad_silence_ms - 192.0).abs() < f64::EPSILON);
    // eou_delay should reflect the backdated gap (≥ 192ms backdate + 5ms sleep)
    assert!(
        metrics.eou_delay_ms >= 190.0,
        "eou_delay_ms should include the backdated silence: {}",
        metrics.eou_delay_ms
    );
}

#[test]
fn turn_tracker_user_agent_latency_computed() {
    let mut tracker = TurnTracker::new();

    tracker.set_vad_silence_ms(0.0);
    tracker.mark_vad_speech_ended();
    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();
    std::thread::sleep(Duration::from_millis(10));
    tracker.mark_tts_start();
    tracker.mark_tts_first_audio();

    let metrics = tracker.finish(1).unwrap();
    // user_agent_latency = vad_speech_ended → tts_first_audio
    let ual = metrics
        .user_agent_latency_ms
        .expect("should be Some when TTS audio present");
    assert!(
        ual >= 10.0,
        "user_agent_latency_ms should be >= 10ms: {}",
        ual
    );
}

#[test]
fn turn_tracker_user_agent_latency_none_without_tts() {
    let mut tracker = TurnTracker::new();

    tracker.set_vad_silence_ms(100.0);
    tracker.mark_vad_speech_ended();
    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();
    // No TTS audio — tool-call-only turn

    let metrics = tracker.finish(1).unwrap();
    assert!(
        metrics.user_agent_latency_ms.is_none(),
        "user_agent_latency_ms should be None without TTS audio"
    );
}

#[test]
fn turn_tracker_reset_preserves_vad_config() {
    let mut tracker = TurnTracker::new();

    tracker.set_vad_silence_ms(192.0);
    tracker.mark_vad_speech_ended();
    tracker.mark_speech_ended();
    tracker.reset();

    // vad_silence_ms is session config — survives reset.
    assert!(
        (tracker.vad_silence_ms() - 192.0).abs() < f64::EPSILON,
        "vad_silence_ms should survive reset: {}",
        tracker.vad_silence_ms(),
    );

    // A new turn after reset should still use the preserved value.
    tracker.mark_vad_speech_ended();
    tracker.mark_speech_ended();
    tracker.mark_transcript();
    tracker.mark_llm_first_token();
    tracker.mark_tts_start();
    tracker.mark_tts_first_audio();

    let metrics = tracker.finish(1).unwrap();
    assert!(
        (metrics.vad_silence_ms - 192.0).abs() < f64::EPSILON,
        "vad_silence_ms should be 192.0 after reset: {}",
        metrics.vad_silence_ms,
    );
}

#[tokio::test]
async fn tracer_barge_in_preserves_stt_metrics() {
    let mut tracer = Tracer::new();
    let mut rx = tracer.subscribe();

    // 1. Set up Turn 1 with realistic STT anchors (mirrors reactor flow)
    tracer.mark_stt_audio_start();
    tracer.mark_vad_speech_ended();
    tracer.mark_stt_finalize_sent();
    tracer.mark_stt_first_text();
    tracer.mark_transcript();
    tracer.mark_speech_ended();
    tracer.start_turn("dg", "m1", "h1", "en", true);
    tracer.mark_tts_text_fed();

    // 2. Interruption begins — STT anchors for the barge-in arrive BEFORE
    //    the old turn is cancelled. This is the real-world timing order.
    tracer.mark_stt_audio_start();
    std::thread::sleep(Duration::from_millis(5));
    tracer.mark_stt_finalize_sent();
    std::thread::sleep(Duration::from_millis(2));
    tracer.mark_stt_first_text();
    tracer.mark_transcript();

    // 3. Barge-in accepted → cancel_turn()
    //    Must preserve the input anchors recorded above.
    tracer.cancel_turn();

    // 4. Start Turn 2 — commit_input_anchors() snapshots the barge-in's STT
    tracer.start_turn("dg", "m1", "interruption", "en", true);

    // 5. Verify SttComplete for Turn 2 has BOTH duration and TTFB
    while let Ok(event) = rx.recv().await {
        if let Event::SttComplete {
            transcript,
            duration_ms,
            ttfb_ms,
            ..
        } = event
        {
            if transcript == "interruption" {
                assert!(
                    duration_ms >= 5.0,
                    "STT duration should be preserved across barge-in: {}",
                    duration_ms,
                );
                assert!(
                    ttfb_ms.is_some(),
                    "STT TTFB should be preserved across barge-in (was the original bug)",
                );
                assert!(
                    ttfb_ms.unwrap() >= 1.0,
                    "STT TTFB should reflect finalize→first_text gap: {}",
                    ttfb_ms.unwrap(),
                );
                return;
            }
        }
    }
}

#[tokio::test]
async fn tracer_overlapping_speech_preserves_stt_audio_start() {
    let mut tracer = Tracer::new();
    let mut rx = tracer.subscribe();

    // 1. Turn 1: normal lifecycle
    tracer.mark_stt_audio_start();
    tracer.mark_vad_speech_ended();
    tracer.mark_stt_finalize_sent();
    tracer.mark_stt_first_text();
    tracer.mark_transcript();
    tracer.mark_speech_ended();
    tracer.start_turn("dg", "m1", "turn1", "en", true);
    tracer.mark_tts_text_fed();
    tracer.mark_tts_first_audio();

    // 2. User starts speaking WHILE Turn 1 TTS is still playing.
    //    SpeechStarted fires → stt_audio_start recorded on self.input.
    tracer.mark_stt_audio_start();
    std::thread::sleep(Duration::from_millis(5));

    // 3. Turn 1 TTS finishes → finish_turn() resets pipeline but must
    //    NOT wipe self.input (stt_audio_start lives there).
    tracer.mark_tts_finished();
    tracer.finish_turn(false, "cartesia", "sonic", "v1");

    // 4. Rest of Turn 2's STT anchors arrive after Turn 1 completed.
    tracer.mark_stt_finalize_sent();
    std::thread::sleep(Duration::from_millis(2));
    tracer.mark_stt_first_text();
    tracer.mark_transcript();
    tracer.mark_speech_ended();

    // 5. Start Turn 2 — should have full STT metrics.
    tracer.start_turn("dg", "m1", "turn2", "en", true);

    // 6. Verify SttComplete for Turn 2 has non-zero duration.
    while let Ok(event) = rx.recv().await {
        if let Event::SttComplete {
            transcript,
            duration_ms,
            ttfb_ms,
            ..
        } = event
        {
            if transcript == "turn2" {
                assert!(
                    duration_ms >= 5.0,
                    "STT duration should include stt_audio_start from overlapping speech: {}",
                    duration_ms,
                );
                assert!(ttfb_ms.is_some(), "STT TTFB should be present",);
                return;
            }
        }
    }
}
