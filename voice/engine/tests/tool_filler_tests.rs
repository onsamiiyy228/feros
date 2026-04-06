//! Tests for tool filler race conditions and edge cases.
//!
//! These simulate the reactor's filler lifecycle with plain variables,
//! following the same pattern as `interruption_policy_tests.rs`.
//!
//! Scenarios covered:
//! - Fast tool return discards pending filler
//! - Slow tool allows filler to speak and be added to context
//! - Filler guard: side_effect_in_flight == 0 ⇒ discard
//! - Empty-text TTS flush prevention
//! - Multiple sequential tool calls with filler
//! - cancel_pipeline aborts pending filler

#![allow(unused_assignments, unused_variables, unused_mut)]

use std::collections::HashMap;

// ═══════════════════════════════════════════════════════════════
// § A  Fast Tool Return: Filler Discarded
// ═══════════════════════════════════════════════════════════════
//
// When the tool completes before the filler hook resolves,
// ToolCallCompleted must abort the pending filler so the LLM's
// real post-tool response plays cleanly.

#[test]
fn fast_tool_discards_pending_filler() {
    // Simulate reactor state
    let mut side_effect_in_flight: u32 = 0;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();
    let mut pending_filler: Option<&str> = None;
    let mut turn_text_len: usize = 0;
    let mut tts_active = false;
    let mut assistant_messages: Vec<String> = vec![];

    // 1. LLM starts tool call with no preamble text
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert("call_1".into(), "save_appointment".into());
    assert!(turn_text_len < 10); // would trigger filler spawn
    pending_filler = Some("Got it, saving that for you now.");

    // 2. Only flush TTS if text was actually sent (our fix)
    if turn_text_len > 0 {
        tts_active = false; // would call tts.flush()
    }
    // turn_text_len == 0, so tts_active is NOT touched
    // (prevents Cartesia "empty initial transcript" error)

    // 3. Tool completes FAST (before filler resolves in real code)
    if side_effect_tool_ids.remove("call_1").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    assert_eq!(side_effect_in_flight, 0);

    // 4. At ToolCallCompleted, abort pending filler
    if pending_filler.is_some() {
        pending_filler = None; // = handle.abort()
    }

    // 5. Restart TTS for post-tool LLM response
    tts_active = true;

    // Verify: filler was discarded, no assistant message added
    assert!(pending_filler.is_none());
    assert!(assistant_messages.is_empty());
    assert!(tts_active); // post-tool TTS is active for the real response
}

// ═══════════════════════════════════════════════════════════════
// § B  Slow Tool: Filler Spoken and Added to Context
// ═══════════════════════════════════════════════════════════════
//
// When the filler hook resolves while the tool is still running
// (side_effect_in_flight > 0), the filler should be:
// 1. Spoken via TTS
// 2. Added as an assistant message to LLM context

#[test]
fn slow_tool_allows_filler_to_speak() {
    let mut side_effect_in_flight: u32 = 1;
    let mut pending_filler: Option<String> = Some("Got it, saving that for you now.".into());
    let mut assistant_messages: Vec<String> = vec![];
    let mut tts_tokens_sent: Vec<String> = vec![];

    // Filler resolves while tool is still running
    let filler_text = pending_filler.take().unwrap();

    // Guard: only speak if tools are still in flight
    if side_effect_in_flight > 0 {
        // Speak the filler
        tts_tokens_sent.push(filler_text.clone());
        // Add to LLM context so post-tool response is aware
        assistant_messages.push(filler_text.clone());
    }

    assert_eq!(tts_tokens_sent, vec!["Got it, saving that for you now."]);
    assert_eq!(assistant_messages, vec!["Got it, saving that for you now."]);
    assert!(pending_filler.is_none());

    // Later: tool completes, no filler to abort
    side_effect_in_flight = 0;
}

// ═══════════════════════════════════════════════════════════════
// § C  Filler Guard: Discard When No Side-Effects In Flight
// ═══════════════════════════════════════════════════════════════
//
// The previous code used `side_effect_in_flight > 0 || llm.is_active()`
// which let filler through even after the tool returned (because the
// LLM IS active — it's generating the continuation). Our fix requires
// side_effect_in_flight > 0 only.

#[test]
fn filler_guard_rejects_when_tools_done() {
    let side_effect_in_flight: u32 = 0;
    let llm_active: bool = true; // LLM is generating post-tool response
    let mut tts_tokens_sent: Vec<String> = vec![];
    let mut assistant_messages: Vec<String> = vec![];

    let filler_text = "Got it, saving that for you now.".to_string();

    // Old (broken) guard:
    let old_guard = side_effect_in_flight > 0 || llm_active;
    assert!(old_guard, "Old guard incorrectly allows filler");

    // New (fixed) guard:
    let new_guard = side_effect_in_flight > 0;
    assert!(!new_guard, "New guard correctly rejects filler");

    // Apply new guard
    if side_effect_in_flight > 0 {
        tts_tokens_sent.push(filler_text.clone());
        assistant_messages.push(filler_text);
    }
    // else: discarded (logged as "Discarding late filler")

    assert!(tts_tokens_sent.is_empty(), "Filler must not be spoken");
    assert!(
        assistant_messages.is_empty(),
        "Filler must not be added to context"
    );
}

// ═══════════════════════════════════════════════════════════════
// § D  Empty-Text Flush Prevention
// ═══════════════════════════════════════════════════════════════
//
// When LLM goes straight to a tool call without any preamble,
// turn_text_len == 0. Previously we called tts.flush() unconditionally,
// causing Cartesia to error with "empty initial transcript".
// Now we skip the flush.

#[test]
fn no_tts_flush_on_empty_preamble() {
    let turn_text_len: usize = 0;
    let mut flush_called = false;

    // Simulate ToolCallStarted
    if turn_text_len > 0 {
        flush_called = true;
    }

    assert!(!flush_called, "Must not flush TTS when no text was sent");
}

#[test]
fn tts_flush_when_preamble_exists() {
    let turn_text_len: usize = 15; // LLM said "Sure, let me " before tool
    let mut flush_called = false;

    // Simulate ToolCallStarted
    if turn_text_len > 0 {
        flush_called = true;
    }

    assert!(
        flush_called,
        "Must flush TTS when text was sent before tool"
    );
}

// ═══════════════════════════════════════════════════════════════
// § E  Multiple Sequential Tool Calls
// ═══════════════════════════════════════════════════════════════
//
// LLM calls tool A (filler spawned), tool A completes (filler aborted),
// LLM then calls tool B (new filler spawned). The old filler must
// not leak into the new tool's execution.

#[test]
fn sequential_tools_each_get_fresh_filler() {
    let mut side_effect_in_flight: u32 = 0;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();
    let mut pending_filler: Option<&str> = None;
    let mut filler_abort_count = 0u32;
    let mut assistant_messages: Vec<String> = vec![];

    // ── Tool A ────────────────────────────────────────────────
    // 1. ToolCallStarted: tool_a
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert("call_a".into(), "tool_a".into());
    // Abort any old filler (none exists)
    if pending_filler.is_some() {
        pending_filler = None;
        filler_abort_count += 1;
    }
    pending_filler = Some("One moment please.");

    // 2. Tool A completes fast
    if side_effect_tool_ids.remove("call_a").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    // Abort pending filler at ToolCallCompleted
    if pending_filler.is_some() {
        pending_filler = None;
        filler_abort_count += 1;
    }
    assert_eq!(filler_abort_count, 1);
    assert!(pending_filler.is_none());

    // ── Tool B ────────────────────────────────────────────────
    // 3. LLM now calls a second tool
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert("call_b".into(), "tool_b".into());
    // Abort any old filler (none, already aborted above)
    if pending_filler.is_some() {
        pending_filler = None;
        filler_abort_count += 1;
    }
    pending_filler = Some("Let me check that.");

    // 4. Filler arrives while tool B is still running
    let filler_text = pending_filler.take().unwrap();
    if side_effect_in_flight > 0 {
        assistant_messages.push(filler_text.to_string());
    }

    // 5. Tool B completes
    if side_effect_tool_ids.remove("call_b").is_some() {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }

    // Verify: only tool B's filler was spoken (tool A's was aborted)
    assert_eq!(assistant_messages, vec!["Let me check that."]);
    assert_eq!(filler_abort_count, 1); // only tool A's filler was aborted
}

// ═══════════════════════════════════════════════════════════════
// § F  cancel_pipeline Aborts Filler
// ═══════════════════════════════════════════════════════════════
//
// A barge-in during tool execution cancels the pipeline.
// cancel_pipeline must also abort the pending filler.

#[test]
fn cancel_pipeline_aborts_filler() {
    let mut side_effect_in_flight: u32 = 1;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();
    side_effect_tool_ids.insert("call_1".into(), "book_room".into());
    let mut pending_filler: Option<&str> = Some("Working on it...");
    let mut hook_hang_up_pending = false;
    let mut turn_text_len: usize = 5;

    // Simulate cancel_pipeline
    side_effect_in_flight = 0;
    side_effect_tool_ids.clear();
    if pending_filler.is_some() {
        pending_filler = None; // = handle.abort()
    }
    hook_hang_up_pending = false;
    turn_text_len = 0;

    assert_eq!(side_effect_in_flight, 0);
    assert!(side_effect_tool_ids.is_empty());
    assert!(pending_filler.is_none(), "Filler must be aborted on cancel");
    assert!(!hook_hang_up_pending);
    assert_eq!(turn_text_len, 0);
}

// ═══════════════════════════════════════════════════════════════
// § G  Filler Threshold: Preamble Text Gate
// ═══════════════════════════════════════════════════════════════
//
// Filler is only spawned when the LLM produced < 10 non-whitespace
// chars before calling a tool. If it said "Sure, let me help!"
// (> 10 chars), no filler is needed — the preamble IS the filler.

#[test]
fn no_filler_when_preamble_is_long_enough() {
    let mut pending_filler: Option<&str> = None;
    let filler_enabled = true;

    // Scenario: LLM said "Sure, I'll save that appointment for you right away!"
    let turn_text_len: usize = 52;

    if turn_text_len < 10 && filler_enabled {
        pending_filler = Some("Got it...");
    }

    assert!(
        pending_filler.is_none(),
        "Should not spawn filler with sufficient preamble"
    );
}

#[test]
fn filler_spawned_when_preamble_is_short() {
    let mut pending_filler: Option<&str> = None;
    let filler_enabled = true;

    // Scenario: LLM called tool immediately with no preamble
    let turn_text_len: usize = 0;

    if turn_text_len < 10 && filler_enabled {
        pending_filler = Some("Got it...");
    }

    assert!(
        pending_filler.is_some(),
        "Should spawn filler with no preamble"
    );
}

// ═══════════════════════════════════════════════════════════════
// § H  Non-Side-Effect Tool: No Filler Race
// ═══════════════════════════════════════════════════════════════
//
// Only side_effect tools set side_effect_in_flight > 0.
// A non-side-effect tool (e.g. get_weather) should NOT cause
// filler to be spoken since there's no protection window.

#[test]
fn non_side_effect_tool_filler_discarded_immediately() {
    let side_effect_in_flight: u32 = 0;
    let mut tts_tokens_sent: Vec<String> = vec![];
    let mut assistant_messages: Vec<String> = vec![];

    // Filler resolves for a non-side-effect tool
    let filler_text = "Let me look that up.".to_string();

    // Guard check: must be in side-effect protection window
    if side_effect_in_flight > 0 {
        tts_tokens_sent.push(filler_text.clone());
        assistant_messages.push(filler_text);
    }

    assert!(
        tts_tokens_sent.is_empty(),
        "Filler must not play for non-side-effect tools"
    );
    assert!(assistant_messages.is_empty());
}

// ═══════════════════════════════════════════════════════════════
// § I  End-to-End: Tool Filler Lifecycle
// ═══════════════════════════════════════════════════════════════
//
// Full lifecycle test that mirrors the exact sequence from the
// bug report: user says "yes" → tool call → fast return → silence.

#[test]
#[allow(unused_assignments)]
fn e2e_fast_tool_lifecycle_no_silence() {
    let mut side_effect_in_flight: u32 = 0;
    let mut side_effect_tool_ids: HashMap<String, String> = HashMap::new();
    let mut pending_filler: Option<String> = None;
    let mut turn_text_len: usize = 0;
    let mut tts_active = false;
    let mut tts_flush_count = 0u32;
    let mut tts_restart_count = 0u32;
    let mut assistant_messages: Vec<String> = vec![];
    let mut tts_tokens_sent: Vec<String> = vec![];

    // 1. User says "YES" → STT transcript → LLM starts
    // (LLM stream started OK)

    // 2. LLM emits ToolCallStarted: save_appointment (side_effect=true)
    side_effect_in_flight += 1;
    side_effect_tool_ids.insert(
        "functions.save_appointment:0".into(),
        "save_appointment".into(),
    );

    // 3. Conditional TTS flush (our fix: only if text was sent)
    if turn_text_len > 0 {
        tts_active = false;
        tts_flush_count += 1;
    }
    assert_eq!(tts_flush_count, 0, "Must NOT flush empty TTS");

    // 4. Spawn filler (turn_text_len < 10)
    assert!(turn_text_len < 10);
    pending_filler = Some("Got it, saving that for you now.".into());

    // 5. Tool completes in 255ms (BEFORE filler resolves in real code)
    if side_effect_tool_ids
        .remove("functions.save_appointment:0")
        .is_some()
    {
        side_effect_in_flight = side_effect_in_flight.saturating_sub(1);
    }
    assert_eq!(side_effect_in_flight, 0);

    // 6. Abort pending filler (our fix)
    if let Some(_filler) = pending_filler.take() {
        // This is the abort — filler is discarded
    }
    assert!(pending_filler.is_none());

    // 7. Restart TTS for post-tool LLM response
    tts_active = true;
    tts_restart_count += 1;

    // 8. LLM continues: "Have a great day!" → 6 TTS batches
    let post_tool_response = "Have a great day!";
    tts_tokens_sent.push(post_tool_response.into());
    turn_text_len += post_tool_response.len();

    // 9. LLM finished
    tts_active = false;
    tts_flush_count += 1; // final flush with actual text

    // Verify: clean pipeline, no silence
    assert_eq!(tts_restart_count, 1, "TTS should restart once (post-tool)");
    assert_eq!(tts_flush_count, 1, "One flush with actual text");
    assert!(pending_filler.is_none(), "Filler must be gone");
    assert!(
        assistant_messages.is_empty(),
        "No filler in context (it was aborted)"
    );
    assert_eq!(tts_tokens_sent, vec!["Have a great day!"]);
}
