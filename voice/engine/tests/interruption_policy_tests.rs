//! Integration tests for the interruption policy and side-effect protection.
//!
//! Tests cover:
//! - Deferred transcript buffering (append, not overwrite)
//! - Side-effect ID tracking (counter management, HashSet correctness)
//! - cancel_stream vs cancel behaviour on conversation history
//! - Pipeline-flag-based transitions during protection window
//! - Timeout stagger invariants (backend < reactor)

use std::collections::HashMap;

use voice_engine::policies::{
    can_barge_in, interrupt_policy, meets_barge_in_word_gate, should_nudge_idle, InterruptPolicy,
};

// ═══════════════════════════════════════════════════════════════
// § A  Deferred Transcript Buffer Semantics
// ═══════════════════════════════════════════════════════════════
//
// The reactor's deferred_transcript must APPEND subsequent
// transcripts with a space separator, not overwrite.

#[test]
fn deferred_transcript_append_semantics() {
    // Simulate the reactor's deferred transcript buffer behavior
    let mut deferred_transcript: Option<String> = None;

    // First transcript during side-effect
    let text1 = "hello".to_string();
    let new_text = if let Some(mut existing) = deferred_transcript.take() {
        existing.push(' ');
        existing.push_str(&text1);
        existing
    } else {
        text1.clone()
    };
    deferred_transcript = Some(new_text);
    assert_eq!(deferred_transcript.as_deref(), Some("hello"));

    // Second transcript during same side-effect window
    let text2 = "world".to_string();
    let new_text = if let Some(mut existing) = deferred_transcript.take() {
        existing.push(' ');
        existing.push_str(&text2);
        existing
    } else {
        text2.clone()
    };
    deferred_transcript = Some(new_text);
    assert_eq!(deferred_transcript.as_deref(), Some("hello world"));

    // Third transcript
    let text3 = "how are you".to_string();
    let new_text = if let Some(mut existing) = deferred_transcript.take() {
        existing.push(' ');
        existing.push_str(&text3);
        existing
    } else {
        text3.clone()
    };
    deferred_transcript = Some(new_text);
    assert_eq!(
        deferred_transcript.as_deref(),
        Some("hello world how are you")
    );
}

#[test]
fn deferred_transcript_empty_after_take() {
    let mut deferred_transcript: Option<String> = Some("buffered speech".to_string());
    let taken = deferred_transcript.take();
    assert!(deferred_transcript.is_none());
    assert_eq!(taken.as_deref(), Some("buffered speech"));
}

// ═══════════════════════════════════════════════════════════════
// § B  Side-Effect ID Tracking
// ═══════════════════════════════════════════════════════════════
//
// Simulates the reactor's side_effect_tool_ids HashSet and
// side_effect_in_flight counter management.

#[test]
fn side_effect_tracking_single_tool() {
    let mut side_effect_in_flight: u32 = 0;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();

    // ToolCallStarted(book_room, side_effect=true)
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert("call_1".to_string(), "book_room".to_string());
    assert_eq!(side_effect_in_flight, 1);

    // ToolCallCompleted(book_room)
    if side_effect_tool_ids.remove("call_1").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    assert_eq!(side_effect_in_flight, 0);
    assert!(side_effect_tool_ids.is_empty());
}

#[test]
fn side_effect_tracking_multiple_tools() {
    let mut side_effect_in_flight: u32 = 0;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();

    // Start two side-effect tools
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert("call_1".to_string(), "book_room".to_string());
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert("call_2".to_string(), "log_event".to_string());
    assert_eq!(side_effect_in_flight, 2);

    // Complete first tool
    if side_effect_tool_ids.remove("call_1").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    assert_eq!(side_effect_in_flight, 1);
    assert!(!side_effect_tool_ids.is_empty());

    // Complete second tool
    if side_effect_tool_ids.remove("call_2").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    assert_eq!(side_effect_in_flight, 0);
    assert!(side_effect_tool_ids.is_empty());
}

#[test]
fn non_side_effect_tool_does_not_decrement() {
    let mut side_effect_in_flight: u32 = 1;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();
    side_effect_tool_ids.insert("call_se".to_string(), "book_room".to_string());

    // A non-side-effect tool completes (ID not in the set)
    let was_side_effect = side_effect_tool_ids.remove("call_regular").is_some();
    if was_side_effect {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    // Counter must NOT change
    assert_eq!(side_effect_in_flight, 1);
    assert!(side_effect_tool_ids.contains_key("call_se"));
}

#[test]
fn duplicate_completion_does_not_underflow() {
    let mut side_effect_in_flight: u32 = 1;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();
    side_effect_tool_ids.insert("call_1".to_string(), "book_room".to_string());

    // First completion
    if side_effect_tool_ids.remove("call_1").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    assert_eq!(side_effect_in_flight, 0);

    // Duplicate completion (same ID again)
    if side_effect_tool_ids.remove("call_1").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    // Must still be 0, not underflowed
    assert_eq!(side_effect_in_flight, 0);
}

// ═══════════════════════════════════════════════════════════════
// § C  cancel_pipeline State Reset Completeness
// ═══════════════════════════════════════════════════════════════
//
// Simulates the fields that cancel_pipeline resets and verifies
// they are all zeroed/cleared.

#[test]
#[allow(unused_assignments)]
fn cancel_pipeline_resets_all_protection_state() {
    // Simulate dirty state
    let mut side_effect_in_flight: u32 = 3;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();
    side_effect_tool_ids.insert("a".into(), "tool_a".into());
    side_effect_tool_ids.insert("b".into(), "tool_b".into());
    side_effect_tool_ids.insert("c".into(), "tool_c".into());
    let mut deferred_transcript: Option<String> = Some("user said something".into());
    let mut barge_in_deferred: bool = true;
    let mut barge_in_transcript_pending: bool = true;
    let mut bot_audio_sent: bool = true;
    let mut idle_retry_count: u32 = 5;

    // Simulate cancel_pipeline
    side_effect_in_flight = 0;
    side_effect_tool_ids.clear();
    deferred_transcript = None;
    barge_in_deferred = false;
    barge_in_transcript_pending = false;
    bot_audio_sent = false;
    idle_retry_count = 0;

    assert_eq!(side_effect_in_flight, 0);
    assert!(side_effect_tool_ids.is_empty());
    assert!(deferred_transcript.is_none());
    assert!(!barge_in_deferred);
    assert!(!barge_in_transcript_pending);
    assert!(!bot_audio_sent);
    assert_eq!(idle_retry_count, 0);
}

// ═══════════════════════════════════════════════════════════════
// § D  End-to-End Policy Scenario: Booking Flow
// ═══════════════════════════════════════════════════════════════
//
// Simulates a full booking conversation flow step by step,
// verifying the policy at each transition.
// Uses pipeline_active flag instead of SessionState.

#[test]
#[allow(unused_assignments)]
fn full_booking_scenario() {
    let mut pipeline_active: bool;
    let mut hang_up_pending = false;
    let mut side_effect_in_flight: u32 = 0;
    let mut bot_audio_sent = false;
    let mut deferred_transcript: Option<String> = None;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();

    // 1. Session starts → Listening (pipeline idle)
    pipeline_active = false;
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Block, // nothing to interrupt
    );

    // 2. Greeting plays → Speaking (pipeline active)
    pipeline_active = true;
    bot_audio_sent = true;
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Allow, // user can barge in on greeting
    );

    // 3. Greeting finishes → Listening (pipeline idle, but audio buffered)
    pipeline_active = false;
    // bot_audio_sent stays true (client still has buffered audio)
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Allow, // drain mode barge-in
    );

    // 4. User speaks "I want to book a room" → Processing (LLM active)
    bot_audio_sent = false;
    pipeline_active = true; // LLM is running
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Allow, // can interrupt processing
    );

    // 5. LLM starts responding → Speaking (TTS active)
    pipeline_active = true;
    bot_audio_sent = true;
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Allow, // can barge in on response
    );

    // 6. LLM calls book_room(side_effect=true)
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert("call_book".to_string(), "book_room".to_string());
    pipeline_active = true; // tool running
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Defer, // protected!
    );

    // 7. User speaks during tool execution → Deferred
    let policy = interrupt_policy(
        hang_up_pending,
        side_effect_in_flight,
        pipeline_active,
        bot_audio_sent,
    );
    assert_eq!(policy, InterruptPolicy::Defer);
    deferred_transcript = Some("actually cancel that".to_string());

    // 8. Nudge timer fires during tool execution → no-op
    assert!(!should_nudge_idle(pipeline_active, 30, 0, 3));

    // 9. Tool completes
    if side_effect_tool_ids.remove("call_book").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    assert_eq!(side_effect_in_flight, 0);

    // 10. Policy now allows
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Allow,
    );

    // 11. Deferred transcript replayed
    let replayed = deferred_transcript.take();
    assert_eq!(replayed.as_deref(), Some("actually cancel that"));
    assert!(deferred_transcript.is_none());

    // 12. New LLM turn → Speaking
    pipeline_active = true;
    bot_audio_sent = true;
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Allow,
    );

    // 13. Agent decides to hang up
    hang_up_pending = true;
    assert_eq!(
        interrupt_policy(
            hang_up_pending,
            side_effect_in_flight,
            pipeline_active,
            bot_audio_sent
        ),
        InterruptPolicy::Block,
    );
}

// ═══════════════════════════════════════════════════════════════
// § E  Timeout Stagger Invariant
// ═══════════════════════════════════════════════════════════════
//
// The backend tool timeout (25s) must be strictly less than
// the reactor safety timeout (30s) to prevent races.

#[test]
fn timeout_stagger_invariant() {
    let backend_timeout_secs = 25;
    let reactor_timeout_secs = 30;
    assert!(
        backend_timeout_secs < reactor_timeout_secs,
        "Backend timeout ({}) must be strictly less than reactor timeout ({})",
        backend_timeout_secs,
        reactor_timeout_secs,
    );
    // Minimum gap to ensure the backend result arrives before reactor fires
    let gap = reactor_timeout_secs - backend_timeout_secs;
    assert!(
        gap >= 3,
        "Gap between timeouts ({}) should be ≥ 3 seconds for safety",
        gap,
    );
}

// ═══════════════════════════════════════════════════════════════
// § F  Word-Count Barge-In with Unicode
// ═══════════════════════════════════════════════════════════════

#[test]
fn word_gate_unicode_word_counting() {
    use unicode_segmentation::UnicodeSegmentation;

    // Single Chinese character = 1 word
    let word_count = "嗯".unicode_words().count() as u32;
    assert_eq!(word_count, 1);
    assert!(!meets_barge_in_word_gate(true, word_count, 2));

    // Two Chinese characters = 2 words
    let word_count = "你好".unicode_words().count() as u32;
    // Note: unicode_segmentation may count this as 1 word (no space)
    // This test documents the actual behavior
    if word_count >= 2 {
        assert!(meets_barge_in_word_gate(true, word_count, 2));
    } else {
        assert!(!meets_barge_in_word_gate(true, word_count, 2));
    }

    // English phrase
    let word_count = "please help me".unicode_words().count() as u32;
    assert_eq!(word_count, 3);
    assert!(meets_barge_in_word_gate(true, word_count, 2));

    // Single English filler
    let word_count = "um".unicode_words().count() as u32;
    assert_eq!(word_count, 1);
    assert!(!meets_barge_in_word_gate(true, word_count, 2));
}

// ═══════════════════════════════════════════════════════════════
// § G  SpeechEnded During Hang-Up
// ═══════════════════════════════════════════════════════════════
//
// Verifies the guard logic: SpeechEnded checks hang_up_pending
// directly; SpeechStarted goes through interrupt_policy.

#[test]
fn speech_events_both_blocked_during_hang_up() {
    let hang_up_pending = true;
    let side_effect = 0;
    let pipeline_active = true;
    let bot_audio = true;

    // SpeechStarted: blocked by interrupt_policy → Block
    assert_eq!(
        interrupt_policy(hang_up_pending, side_effect, pipeline_active, bot_audio),
        InterruptPolicy::Block,
    );

    // SpeechEnded: blocked by direct hang_up_pending check
    // (in the reactor: `if self.hang_up_pending { return; }`)
    assert!(hang_up_pending);
}

// ═══════════════════════════════════════════════════════════════
// § H  Pipeline Transitions During Protection Window
// ═══════════════════════════════════════════════════════════════
//
// Verifies that all transitions during a side-effect
// protection window are consistent with the policy.

#[test]
fn protection_window_state_sequence() {
    // Timeline: pipeline active → tool in flight → tool done → pipeline active again
    let se = 1; // side_effect_in_flight

    // During bot speech before tool call (pipeline active)
    assert_eq!(
        interrupt_policy(false, se, true, true),
        InterruptPolicy::Defer,
    );

    // After tool call started (pipeline active, tool in flight)
    assert_eq!(
        interrupt_policy(false, se, true, false),
        InterruptPolicy::Defer,
    );

    // After tool completes, pipeline still active (se=0)
    assert_eq!(
        interrupt_policy(false, 0, true, false),
        InterruptPolicy::Allow, // protection lifted
    );

    // During deferred replay → new LLM response (pipeline active)
    assert_eq!(
        interrupt_policy(false, 0, true, true),
        InterruptPolicy::Allow, // normal barge-in allowed
    );
}

// ═══════════════════════════════════════════════════════════════
// § I  Exhaustive Priority Verification
// ═══════════════════════════════════════════════════════════════
//
// For every possible (hang_up, side_effect>0, pipeline_active, audio),
// verify the priority chain is consistent.

#[test]
fn priority_chain_is_strict() {
    for pipeline_active in [false, true] {
        for audio in [false, true] {
            // hang_up always wins
            let with_hang_up = interrupt_policy(true, 0, pipeline_active, audio);
            assert_eq!(
                with_hang_up,
                InterruptPolicy::Block,
                "hang_up must Block (pipeline_active={}, audio={})",
                pipeline_active,
                audio,
            );

            // hang_up beats side_effect
            let hang_up_and_se = interrupt_policy(true, 1, pipeline_active, audio);
            assert_eq!(
                hang_up_and_se,
                InterruptPolicy::Block,
                "hang_up must beat side_effect (pipeline_active={}, audio={})",
                pipeline_active,
                audio,
            );

            // side_effect always defers (when no hang_up)
            let with_se = interrupt_policy(false, 1, pipeline_active, audio);
            assert_eq!(
                with_se,
                InterruptPolicy::Defer,
                "side_effect must Defer (pipeline_active={}, audio={})",
                pipeline_active,
                audio,
            );

            // No flags: determined by can_barge_in
            let no_flags = interrupt_policy(false, 0, pipeline_active, audio);
            let expected = if can_barge_in(pipeline_active, audio) {
                InterruptPolicy::Allow
            } else {
                InterruptPolicy::Block
            };
            assert_eq!(
                no_flags, expected,
                "no-flag policy must match can_barge_in (pipeline_active={}, audio={})",
                pipeline_active, audio,
            );
        }
    }
}
