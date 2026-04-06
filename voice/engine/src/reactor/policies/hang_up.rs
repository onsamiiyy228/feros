//! Hang-up classifier policy.
//!
//! The [`HangUpPolicy`] trait gates whether the reactor spawns the LLM-based
//! hang-up classifier hook after a turn ends.

/// Controls whether the reactor should run the hang-up classifier hook.
///
/// Called at the end of every LLM turn when the agent did not itself call
/// `hang_up`. The policy decides whether the classifier is worth running given
/// how many turns have elapsed.
///
/// # Contract
///
/// Must be pure and cheap — called on the hot event-loop thread.
pub trait HangUpPolicy: Send + Sync + 'static {
    /// Returns `true` if the hang-up classifier should be spawned.
    ///
    /// - `turn_count`: number of fully completed LLM turns so far (1-based).
    /// - `min_turns`: floor configured by the `HookRunner`.
    fn should_run_classifier(&self, turn_count: u32, min_turns: u32) -> bool;
}

/// Default hang-up policy: run the classifier once `min_turns` have completed.
#[derive(Default)]
pub struct DefaultHangUpPolicy;

impl HangUpPolicy for DefaultHangUpPolicy {
    #[inline]
    fn should_run_classifier(&self, turn_count: u32, min_turns: u32) -> bool {
        turn_count >= min_turns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_blocks_before_min_turns() {
        assert!(!DefaultHangUpPolicy.should_run_classifier(1, 3));
    }

    #[test]
    fn default_passes_at_min_turns() {
        assert!(DefaultHangUpPolicy.should_run_classifier(3, 3));
    }

    #[test]
    fn default_passes_above_min_turns() {
        assert!(DefaultHangUpPolicy.should_run_classifier(10, 3));
    }

    #[test]
    fn custom_policy_can_suppress_entirely() {
        struct NeverHangUp;
        impl HangUpPolicy for NeverHangUp {
            fn should_run_classifier(&self, _: u32, _: u32) -> bool {
                false
            }
        }
        assert!(!NeverHangUp.should_run_classifier(100, 1));
    }
}
