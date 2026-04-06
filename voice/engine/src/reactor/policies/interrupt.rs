//! Interrupt policy — priority chain for pipeline interruptions.

/// What the reactor should do when the user tries to interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptPolicy {
    /// Normal: cancel pipeline, process user speech immediately.
    Allow,
    /// Blocked: ignore the interruption entirely (hang-up goodbye playing).
    Block,
    /// Deferred: capture user speech but don't process it yet.
    /// The pipeline continues; speech is replayed when the blocker clears.
    Defer,
}

/// Single source of truth for interruption decisions.
///
/// Every code path that could disrupt the pipeline calls this FIRST.
///  - `hang_up_pending`: agent called hang_up, goodbye TTS is playing
///  - `side_effect_in_flight`: count of active tools with side_effect: true
///  - `pipeline_active`: LLM or TTS is currently running
///  - `bot_audio_sent`: whether TTS audio has been sent this turn
pub fn interrupt_policy(
    hang_up_pending: bool,
    side_effect_in_flight: u32,
    pipeline_active: bool,
    bot_audio_sent: bool,
) -> InterruptPolicy {
    // Rule 1: Hang-up is absolute — ignore everything
    if hang_up_pending {
        return InterruptPolicy::Block;
    }

    // Rule 2: Side-effect tools — defer (capture speech, replay later)
    if side_effect_in_flight > 0 {
        return InterruptPolicy::Defer;
    }

    // Rule 3: Normal barge-in check
    if super::idle::can_barge_in(pipeline_active, bot_audio_sent) {
        return InterruptPolicy::Allow;
    }

    // Rule 4: Nothing to interrupt
    InterruptPolicy::Block
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════
    // § 1  interrupt_policy — Priority Chain
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn hang_up_blocks_when_pipeline_active() {
        assert_eq!(
            interrupt_policy(true, 0, true, true),
            InterruptPolicy::Block
        );
    }

    #[test]
    fn hang_up_beats_side_effect() {
        assert_eq!(
            interrupt_policy(true, 5, true, true),
            InterruptPolicy::Block
        );
    }

    #[test]
    fn hang_up_blocks_regardless_of_pipeline() {
        for pipeline_active in [false, true] {
            for audio in [false, true] {
                assert_eq!(
                    interrupt_policy(true, 0, pipeline_active, audio),
                    InterruptPolicy::Block,
                    "hang_up must block (pipeline_active={}, audio={})",
                    pipeline_active,
                    audio
                );
            }
        }
    }

    #[test]
    fn side_effect_defers_when_active() {
        assert_eq!(
            interrupt_policy(false, 1, true, false),
            InterruptPolicy::Defer
        );
    }

    #[test]
    fn side_effect_defers_regardless_of_pipeline() {
        for pipeline_active in [false, true] {
            for audio in [false, true] {
                assert_eq!(
                    interrupt_policy(false, 1, pipeline_active, audio),
                    InterruptPolicy::Defer,
                    "side_effect must defer (pipeline_active={}, audio={})",
                    pipeline_active,
                    audio
                );
            }
        }
    }

    #[test]
    fn allow_when_pipeline_active_with_audio() {
        assert_eq!(
            interrupt_policy(false, 0, true, true),
            InterruptPolicy::Allow
        );
    }

    #[test]
    fn allow_when_idle_with_buffered_audio() {
        assert_eq!(
            interrupt_policy(false, 0, false, true),
            InterruptPolicy::Allow
        );
    }

    #[test]
    fn block_when_idle_no_audio() {
        assert_eq!(
            interrupt_policy(false, 0, false, false),
            InterruptPolicy::Block
        );
    }

    #[test]
    fn all_combinations_produce_allow_or_block_without_flags() {
        use super::super::idle::can_barge_in;
        for pipeline_active in [false, true] {
            for audio in [false, true] {
                let policy = interrupt_policy(false, 0, pipeline_active, audio);
                let expected = if can_barge_in(pipeline_active, audio) {
                    InterruptPolicy::Allow
                } else {
                    InterruptPolicy::Block
                };
                assert_eq!(
                    policy, expected,
                    "pipeline_active={} audio={}: mismatch",
                    pipeline_active, audio
                );
            }
        }
    }

    #[test]
    fn post_barge_in_policy_is_block() {
        assert_eq!(
            interrupt_policy(false, 0, false, false),
            InterruptPolicy::Block,
            "Post-barge-in state returns Block — CommitBargeIn must not use interrupt_policy()"
        );
    }
}
