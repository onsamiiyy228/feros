//! Idle / nudge policy — pure decision functions for silence reengagement.

/// Timeout for whisper-fallback: if STT detects speech but VAD hasn't
/// fired within this window, treat it as a whisper turn.
pub const WHISPER_FALLBACK_SECS: f64 = 0.8;

/// Whether a barge-in should be allowed.
///
/// True when the pipeline is actively working (LLM generating, TTS
/// speaking, or tools running) OR when the client may still have
/// buffered TTS audio to play.
///
/// Used internally by [`super::interrupt::interrupt_policy`] and by
/// [`super::barge_in::DefaultBargeInPolicy`].
#[inline]
pub fn can_barge_in(pipeline_active: bool, bot_audio_sent: bool) -> bool {
    bot_audio_sent || pipeline_active
}

/// Check whether a barge-in should be gated by the min-words requirement.
///
/// When `min_barge_in_words > 0` and TTS is actively sending audio, we
/// require the user to have said at least that many words (from the full
/// STT utterance) before triggering barge-in. This prevents filler words
/// ("um", "uh", "yeah") from accidentally interrupting the bot.
///
/// Returns `true` if the barge-in should proceed.
#[inline]
pub fn meets_barge_in_word_gate(tts_active: bool, word_count: u32, min_words: u32) -> bool {
    // Gate only applies during bot speech. Processing / ToolCalling
    // always allow immediate barge-in regardless of word count.
    if !tts_active || min_words == 0 {
        return true;
    }
    word_count >= min_words
}

/// Whether an idle nudge should fire.
///
/// Returns `true` only when the pipeline is idle (not running),
/// idle timeouts are enabled, and nudges remain.
///
/// # Convention
///
/// `nudge_count` must be the number of nudges **already sent** (1-based,
/// incremented by the caller before this check).  Correct usage:
///
/// ```text
/// on_user_idle_fired() → increments idle_retries → returns new count
/// should_nudge_idle(…, count, max) → true while count <= max
/// ```
///
/// With `max_nudges = N`: nudges 1 … N are sent, then the call hangs up.
///
/// # Edge case
///
/// `nudge_count = 0` always returns `false` (the `> 0` guard).  The normal
/// call path increments before checking, so 0 is unreachable in production,
/// but the guard prevents a semantic violation when `max_nudges = 0`.
#[inline]
pub fn should_nudge_idle(
    pipeline_active: bool,
    idle_timeout_secs: u32,
    nudge_count: u32,
    max_nudges: u32,
) -> bool {
    !pipeline_active && idle_timeout_secs > 0 && nudge_count > 0 && nudge_count <= max_nudges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_barge_in_when_pipeline_active() {
        assert!(can_barge_in(true, false));
    }

    #[test]
    fn no_barge_in_when_idle() {
        assert!(!can_barge_in(false, false));
    }

    #[test]
    fn barge_in_audio_overrides_pipeline() {
        assert!(can_barge_in(false, true));
        assert!(can_barge_in(true, true));
    }

    #[test]
    fn word_gate_disabled_when_zero() {
        assert!(meets_barge_in_word_gate(true, 0, 0));
    }

    #[test]
    fn word_gate_blocks_filler_words() {
        assert!(!meets_barge_in_word_gate(true, 1, 2));
    }

    #[test]
    fn word_gate_exact_threshold() {
        assert!(meets_barge_in_word_gate(true, 2, 2));
    }

    #[test]
    fn word_gate_bypassed_when_not_speaking() {
        assert!(meets_barge_in_word_gate(false, 0, 5));
    }

    #[test]
    fn nudge_fires_for_first_nudge() {
        assert!(should_nudge_idle(false, 30, 1, 3));
    }

    #[test]
    fn nudge_stops_after_max() {
        assert!(!should_nudge_idle(false, 30, 4, 3));
    }

    #[test]
    fn nudge_blocked_when_pipeline_active() {
        assert!(!should_nudge_idle(true, 30, 1, 3));
    }

    #[test]
    fn nudge_skipped_when_disabled() {
        assert!(!should_nudge_idle(false, 0, 1, 3));
    }

    #[test]
    fn nudge_count_zero_always_false() {
        assert!(!should_nudge_idle(false, 30, 0, 3));
        assert!(!should_nudge_idle(false, 30, 0, 0));
    }
}
