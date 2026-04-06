//! Barge-in interruption policy.
//!
//! The [`BargeInPolicy`] trait is the word-count gate evaluated after the
//! `TurnPhase` state machine approves a barge-in transition.

use super::idle::meets_barge_in_word_gate;

/// Controls whether a user barge-in (interruption) should be accepted.
///
/// Evaluated in `CommitBargeIn` after the `TurnPhase` state machine has
/// already approved the transition. This is the final word-count gate.
///
/// # Contract
///
/// Must be pure and cheap — called on the hot event-loop thread.
pub trait BargeInPolicy: Send + Sync + 'static {
    /// Returns `true` if the barge-in should be accepted.
    ///
    /// - `tts_active`: always `true` by construction (TTS was active when the
    ///   barge-in word count started accumulating; by the time `CommitBargeIn`
    ///   fires, `cancel_pipeline` has already cleared the live TTS flag).
    /// - `word_count`: number of unicode words in the user's utterance.
    /// - `min_words`: configured minimum from `SessionConfig`.
    fn should_accept(&self, tts_active: bool, word_count: u32, min_words: u32) -> bool;
}

/// Default barge-in policy: delegates to [`meets_barge_in_word_gate`].
///
/// Blocks filler-word interruptions when `min_barge_in_words > 0` and TTS is
/// active.  Always passes through when TTS is idle.
#[derive(Default)]
pub struct DefaultBargeInPolicy;

impl BargeInPolicy for DefaultBargeInPolicy {
    #[inline]
    fn should_accept(&self, tts_active: bool, word_count: u32, min_words: u32) -> bool {
        meets_barge_in_word_gate(tts_active, word_count, min_words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_passes_when_tts_inactive() {
        // Gate is bypassed when TTS is not active — always allow.
        assert!(DefaultBargeInPolicy.should_accept(false, 0, 5));
    }

    #[test]
    fn default_blocks_below_min_words() {
        assert!(!DefaultBargeInPolicy.should_accept(true, 1, 2));
    }

    #[test]
    fn default_passes_at_exact_min_words() {
        assert!(DefaultBargeInPolicy.should_accept(true, 2, 2));
    }

    #[test]
    fn custom_policy_can_override() {
        struct AlwaysAllow;
        impl BargeInPolicy for AlwaysAllow {
            fn should_accept(&self, _: bool, _: u32, _: u32) -> bool {
                true
            }
        }
        // Even with tts_active + 0 words, custom policy wins.
        assert!(AlwaysAllow.should_accept(true, 0, 10));
    }
}
