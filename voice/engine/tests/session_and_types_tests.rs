//! Tests for reactor Ledger and session configuration defaults.

use voice_engine::session::SessionConfig;

// ── SessionConfig defaults ───────────────────────────────────────

#[test]
fn session_config_default_values() {
    let config = SessionConfig::default();
    assert_eq!(config.input_sample_rate, 48_000);
    assert_eq!(config.output_sample_rate, 24_000);
    assert!(config.denoise_enabled);
    assert!(config.smart_turn_enabled);
    assert!(config.turn_completion_enabled);
    assert_eq!(config.idle_timeout_secs, 4);
    assert_eq!(config.idle_max_nudges, 2);
    assert_eq!(config.min_barge_in_words, 2);
    assert_eq!(config.barge_in_timeout_ms, 800);
    assert!(config.agent_graph.is_none());
    assert_eq!(config.language, "en");
    assert_eq!(config.models_dir, "./dsp_models");
}

#[test]
fn session_config_clone() {
    let config = SessionConfig::default();
    let cloned = config.clone();
    assert_eq!(config.agent_id, cloned.agent_id);
    assert_eq!(config.system_prompt, cloned.system_prompt);
    assert_eq!(config.min_barge_in_words, cloned.min_barge_in_words);
    assert_eq!(config.barge_in_timeout_ms, cloned.barge_in_timeout_ms);
}

// ── TurnCompletion (supplement the inline tests in types.rs) ─────

use voice_engine::types::TurnCompletion;

#[test]
fn turn_completion_timeout_values() {
    assert_eq!(TurnCompletion::Complete.timeout_secs(), 0);
    assert_eq!(TurnCompletion::IncompleteShort.timeout_secs(), 3);
    assert_eq!(TurnCompletion::IncompleteLong.timeout_secs(), 4);
}

#[test]
fn turn_completion_reprompt_messages() {
    assert!(TurnCompletion::Complete.reprompt_message().is_empty());
    assert!(!TurnCompletion::IncompleteShort
        .reprompt_message()
        .is_empty());
    assert!(!TurnCompletion::IncompleteLong.reprompt_message().is_empty());
}

#[test]
fn turn_completion_detect_with_trailing_text() {
    // Complete marker followed by text
    let (tc, skip) = TurnCompletion::detect("✓ The answer is 42.").unwrap();
    assert_eq!(tc, TurnCompletion::Complete);
    assert_eq!(&"✓ The answer is 42."[skip..], "The answer is 42.");
}

#[test]
fn turn_completion_detect_incomplete_markers_are_full_token() {
    // ○ and ◐ typically appear alone
    let (tc, _) = TurnCompletion::detect("○").unwrap();
    assert_eq!(tc, TurnCompletion::IncompleteShort);

    let (tc, _) = TurnCompletion::detect("◐").unwrap();
    assert_eq!(tc, TurnCompletion::IncompleteLong);
}

#[test]
fn turn_completion_detect_plain_text_returns_none() {
    assert!(TurnCompletion::detect("Hello").is_none());
    assert!(TurnCompletion::detect("").is_none());
    assert!(TurnCompletion::detect("   ").is_none());
}

// ── SessionState ─────────────────────────────────────────────────

use voice_engine::types::SessionState;

#[test]
fn session_state_equality() {
    assert_eq!(SessionState::Idle, SessionState::Idle);
    assert_ne!(SessionState::Idle, SessionState::Listening);
    assert_ne!(SessionState::Speaking, SessionState::Processing);
}

#[test]
fn session_state_is_copy() {
    let a = SessionState::Speaking;
    let b = a; // Copy
    assert_eq!(a, b);
}
