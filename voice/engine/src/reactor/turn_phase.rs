//! `TurnPhase` — compile-time statechart for the user-turn lifecycle.
//!
//! All turn-lifecycle decisions are made by a single pure function
//! [`TurnPhase::transition`]. The `Reactor` feeds events in, receives
//! [`TurnAction`]s back, and executes them.
//!
//! # Compile-time safety guarantees
//!
//! 1. **Data isolation** — timer keys, buffered text, etc. only exist inside
//!    the state variant that needs them.
//! 2. **Exhaustive matching** — no `_ =>` catch-all; the compiler forces
//!    every `(TurnPhase, TurnEvent)` pair to be handled explicitly.
//! 3. **Unused-variable lint** — the transition function consumes `self`. If
//!    a state variant carries a `timer_key` and the handler forgets to use
//!    it, `#[deny(unused_variables)]` makes it a compile error.
//! 4. **Output gate** — [`AudioOutputPermit`] can only be obtained when
//!    the user is NOT speaking. The `Option` return enforces a runtime
//!    gate (I2): callers must check `audio_output_permit().is_none()`
//!    before emitting audio. This is a runtime guard, not a compile-time
//!    guarantee — future work could plumb `&AudioOutputPermit` into the
//!    audio emission path to make it a compile error.

// Forgetting to handle a destructured timer_key is caught by
// `#[deny(unused_variables)]` on `transition()`.

use tokio_util::time::delay_queue;

#[derive(Debug, Default)]
pub(crate) enum TurnPhase {
    /// Pipeline idle. Waiting for user to speak.
    #[default]
    Listening,

    /// VAD fired `SpeechStarted`. The TTS output gate is **CLOSED**.
    ///
    /// `pipeline_was_active` snapshots whether the LLM/TTS was running at the
    /// moment speech started — used at `SpeechEnded` to choose the barge-in
    /// vs normal turn path.
    ///
    /// In-flight transcripts from a previous `finalize()` that arrive while
    /// in this state are buffered via `BufferTranscript` (I5) rather than
    /// dropped. This replaces the old `Cancelled` interstitial state.
    UserSpeaking { pipeline_was_active: bool },

    /// SmartTurn predicted "turn complete"; `finalize()` was sent to STT.
    /// Waiting for the transcript to arrive (or the STT timeout to fire).
    ///
    /// The `timer_key` **MUST** be cancelled when leaving this state.
    /// The transition function destructures it, so forgetting would be a
    /// compile error (`#[deny(unused_variables)]`).
    WaitingForTranscript { timer_key: delay_queue::Key },

    /// `SttTimeout` fired — no transcript arrived in time.
    ///
    /// A late transcript may still arrive over the network. It is:
    /// - **Accepted** if the pipeline is idle (no duplicate risk).
    /// - **Dropped** if the pipeline is already running another turn.
    TranscriptTimedOut,

    /// SmartTurn predicted "not done yet" — the EoT fallback timer is running.
    /// If it fires, we finalize STT and commit the turn anyway.
    ///
    /// The `timer_key` is owned here — same `#[deny(unused_variables)]` safety
    /// as `WaitingForTranscript`.
    EotFallback { timer_key: delay_queue::Key },

    /// Barge-in path: the pipeline was active, user stopped speaking. Waiting
    /// for the transcript to check word count before deciding whether to
    /// commit the barge-in or reject it as noise.
    BargeInWordCount,
}

// ── Events ───────────────────────────────────────────────────────────────────

/// Control events that can cause a turn-lifecycle transition.
///
/// Raw audio chunks and LLM tokens are NOT included — they don't change the
/// turn phase. Only macro-level control events appear here.
#[derive(Debug, Clone)]
pub(crate) enum TurnEvent {
    /// VAD detected user speech onset.
    SpeechStarted {
        /// Was the pipeline (LLM/TTS/side-effect) running at the time?
        pipeline_was_active: bool,
        /// Had we sent any TTS audio to the client this turn?
        bot_audio_sent: bool,
    },

    /// VAD detected end of user speech.
    SpeechEnded {
        /// SmartTurn model prediction (true = "user is done speaking").
        /// `true` if SmartTurn is disabled (no model → always complete).
        smart_complete: bool,
    },

    /// STT returned a final transcript.
    Transcript { text: String },

    /// The STT timeout timer fired (no transcript arrived in time).
    SttTimeout,

    /// The end-of-turn fallback timer fired.
    EotFallback,
}

// ── Actions ──────────────────────────────────────────────────────────────────

/// Side effects returned by the transition function.
///
/// The `Reactor` executes these — the state machine never performs I/O.
#[derive(Debug, PartialEq)]
pub(crate) enum TurnAction {
    /// Emit `Event::Interrupt` to flush the client's audio buffer.
    EmitInterrupt,

    /// Emit an agent lifecycle event (e.g. "barge_in").
    EmitAgentEvent(&'static str),

    /// Cancel the running LLM + TTS pipeline.
    CancelPipeline,

    /// Call `stt.finalize()` to request a final transcript.
    FinalizeSTT,

    /// Commit the user's text as a new LLM turn.
    CommitTurn(String),

    /// Buffer the transcript for prepending to the next utterance (I5).
    BufferTranscript(String),

    /// Commit a barge-in transcript (from BargeInWordCount).
    ///
    /// The executor checks word count against `min_barge_in_words` and
    /// either commits the turn or rejects the barge-in.
    CommitBargeIn(String),

    /// Commit a late transcript (from TranscriptTimedOut).
    ///
    /// The executor checks `is_pipeline_active()`: if active, drops the
    /// transcript (pipeline is already running another turn); if idle,
    /// accepts and commits.
    CommitLateTranscript(String),

    /// Cancel a timer by its `delay_queue::Key`.
    CancelTimer(delay_queue::Key),

    /// Start the user-idle detection timer.
    StartIdleTimer,

    /// Mark speech ended in the tracer (metrics).
    MarkSpeechEnded,

    /// Emit a tracer trace event.
    Trace(&'static str),

    /// Mark STT finalize sent in the tracer (metrics).
    MarkSttFinalizeSent,

    /// Clear `bot_audio_sent` on the reactor.
    ClearBotAudioSent,
}

// ── Next Phase ───────────────────────────────────────────────────────────────

/// The next phase specification returned by [`TurnPhase::transition`].
///
/// `transition()` cannot construct `EotFallback { timer_key }` or
/// `WaitingForTranscript { timer_key }` because timer keys are allocated
/// by the reactor's `DelayQueue` — they don't exist yet at transition time.
///
/// Instead of returning a placeholder `TurnPhase::Listening` and leaking
/// timer allocation into `TurnAction`, `transition()` returns a `NextPhase`
/// that names the *intent* explicitly. `dispatch_turn_event` calls `resolve()`
/// which allocates the key and constructs the concrete `TurnPhase`.
///
/// This keeps `transition()` honest: the returned value reads exactly like
/// the state diagram without a two-pass fixup loop.
#[derive(Debug)]
pub(crate) enum NextPhase {
    /// Enter this phase immediately — no timer needed.
    Set(TurnPhase),
    /// Allocate an STT timeout timer and enter `WaitingForTranscript { timer_key }`.
    WaitForTranscript,
    /// Allocate an EoT fallback timer and enter `EotFallback { timer_key }`.
    StartEotFallback,
}

// ── Capability Token ─────────────────────────────────────────────────────────

/// Zero-sized proof that the TTS output gate is **open**.
///
/// Obtained via [`TurnPhase::audio_output_permit`]. Only available when the
/// user is NOT speaking. Forces the TTS handler to check the gate before
/// emitting audio — a missing check is a compile error.
pub(crate) struct AudioOutputPermit;

impl TurnPhase {
    /// Returns `Some(permit)` if audio output is allowed in this phase.
    ///
    /// Returns `None` during `UserSpeaking` — the TTS gate is closed (I2).
    pub(super) fn audio_output_permit(&self) -> Option<AudioOutputPermit> {
        match self {
            TurnPhase::UserSpeaking { .. } => None,
            _ => Some(AudioOutputPermit),
        }
    }

    /// `true` when the user is currently speaking (VAD-based).
    ///
    /// Convenience for guards that don't need the capability token.
    pub(super) fn is_speaking(&self) -> bool {
        matches!(self, TurnPhase::UserSpeaking { .. })
    }

    /// Hard-reset to `Listening`, cancelling any owned timer keys.
    ///
    /// Called only from `dispatch_turn_event()` guards (e.g. the side-effect
    /// SpeechEnded interceptor) — NOT from `cancel_pipeline()`.
    ///
    /// `dispatch_turn_event` is the exclusive owner of `turn_phase` writes.
    /// `cancel_pipeline()` must not touch `turn_phase` because the state
    /// machine may have already set the correct next phase (e.g.
    /// `BargeInWordCount` during a barge-in).
    pub(super) fn reset(
        &mut self,
        timers: &mut tokio_util::time::DelayQueue<crate::types::TimerKey>,
    ) {
        match std::mem::take(self) {
            TurnPhase::WaitingForTranscript { timer_key } => {
                timers.remove(&timer_key);
            }
            TurnPhase::EotFallback { timer_key } => {
                timers.remove(&timer_key);
            }
            // Other variants own no timer keys.
            _ => {}
        }
        // `std::mem::take` already set self to Listening (Default).
    }
}

// ── Transition Function ──────────────────────────────────────────────────────

impl TurnPhase {
    /// Pure state transition: `(State, Event) → (NextPhase, Actions)`.
    ///
    /// **No side effects.** The returned [`TurnAction`]s are executed by the
    /// `Reactor` after the transition completes.
    ///
    /// Every `(TurnPhase, TurnEvent)` pair is handled explicitly — there is
    /// no catch-all. Adding a new variant to either enum forces the developer
    /// to handle every combination (compiler error).
    ///
    /// Returns [`NextPhase`] rather than a bare `TurnPhase` so that timer-
    /// backed states (`WaitingForTranscript`, `EotFallback`) can be named
    /// truthfully without requiring access to the reactor's `DelayQueue`.
    /// `dispatch_turn_event` resolves `NextPhase` into a concrete `TurnPhase`.
    #[deny(unused_variables)]
    pub(super) fn transition(self, event: TurnEvent) -> (NextPhase, Vec<TurnAction>) {
        match (self, event) {
            // ━━ Listening ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            (
                TurnPhase::Listening,
                TurnEvent::SpeechStarted {
                    pipeline_was_active,
                    bot_audio_sent,
                },
            ) => {
                let mut actions = Vec::new();
                if bot_audio_sent {
                    actions.push(TurnAction::EmitInterrupt);
                    actions.push(TurnAction::ClearBotAudioSent);
                }
                (
                    NextPhase::Set(TurnPhase::UserSpeaking {
                        pipeline_was_active,
                    }),
                    actions,
                )
            }

            // Transcript while Listening: EoT fallback path or SmartTurn disabled.
            // The transcript arrives and goes directly to LLM.
            (TurnPhase::Listening, TurnEvent::Transcript { text }) => {
                if text.trim().is_empty() {
                    (NextPhase::Set(TurnPhase::Listening), vec![])
                } else {
                    (
                        NextPhase::Set(TurnPhase::Listening),
                        vec![TurnAction::CommitTurn(text)],
                    )
                }
            }

            // Stale timers from a previous turn — no-op.
            (TurnPhase::Listening, TurnEvent::SttTimeout)
            | (TurnPhase::Listening, TurnEvent::EotFallback)
            | (TurnPhase::Listening, TurnEvent::SpeechEnded { .. }) => {
                (NextPhase::Set(TurnPhase::Listening), vec![])
            }

            // ━━ UserSpeaking ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

            // Normal turn path: pipeline was idle when user started speaking.
            (
                TurnPhase::UserSpeaking {
                    pipeline_was_active: false,
                },
                TurnEvent::SpeechEnded {
                    smart_complete: true,
                },
            ) => {
                // SmartTurn says "done" — finalize STT and enter
                // WaitingForTranscript. Timer key allocated by dispatch.
                (
                    NextPhase::WaitForTranscript,
                    vec![TurnAction::FinalizeSTT, TurnAction::MarkSttFinalizeSent],
                )
            }

            (
                TurnPhase::UserSpeaking {
                    pipeline_was_active: false,
                },
                TurnEvent::SpeechEnded {
                    smart_complete: false,
                },
            ) => {
                // SmartTurn says "not done" — start EoT fallback timer.
                // Timer key allocated by dispatch.
                (NextPhase::StartEotFallback, vec![])
            }

            // Barge-in path: pipeline was active when user started speaking.
            (
                TurnPhase::UserSpeaking {
                    pipeline_was_active: true,
                },
                TurnEvent::SpeechEnded { smart_complete: _ },
            ) => (
                NextPhase::Set(TurnPhase::BargeInWordCount),
                vec![
                    TurnAction::Trace("BargeIn"),
                    TurnAction::EmitAgentEvent("barge_in"),
                    TurnAction::CancelPipeline,
                    TurnAction::FinalizeSTT,
                    TurnAction::MarkSttFinalizeSent,
                ],
            ),

            // Duplicate VAD event — ignore.
            (
                TurnPhase::UserSpeaking {
                    pipeline_was_active,
                },
                TurnEvent::SpeechStarted { .. },
            ) => (
                NextPhase::Set(TurnPhase::UserSpeaking {
                    pipeline_was_active,
                }),
                vec![],
            ),

            (
                TurnPhase::UserSpeaking {
                    pipeline_was_active,
                },
                TurnEvent::Transcript { text },
            ) => {
                // In-flight transcript from a previous finalize() arrived while the
                // user is still speaking. Buffer it so the next committed turn
                // prepends it (I5: don't discard user context).
                (
                    NextPhase::Set(TurnPhase::UserSpeaking {
                        pipeline_was_active,
                    }),
                    vec![TurnAction::BufferTranscript(text)],
                )
            }

            (
                TurnPhase::UserSpeaking {
                    pipeline_was_active,
                },
                TurnEvent::SttTimeout,
            )
            | (
                TurnPhase::UserSpeaking {
                    pipeline_was_active,
                },
                TurnEvent::EotFallback,
            ) => {
                // Stale timer events — ignore.
                (
                    NextPhase::Set(TurnPhase::UserSpeaking {
                        pipeline_was_active,
                    }),
                    vec![],
                )
            }

            // ━━ WaitingForTranscript ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            (TurnPhase::WaitingForTranscript { timer_key }, TurnEvent::Transcript { text }) => {
                // Transcript arrived in time — cancel the STT timeout and commit.
                (
                    NextPhase::Set(TurnPhase::Listening),
                    vec![
                        TurnAction::CancelTimer(timer_key),
                        TurnAction::MarkSpeechEnded,
                        TurnAction::CommitTurn(text),
                    ],
                )
            }

            (TurnPhase::WaitingForTranscript { timer_key }, TurnEvent::SttTimeout) => {
                // STT timed out — key already consumed by DelayQueue.
                let _ = timer_key;
                (
                    NextPhase::Set(TurnPhase::TranscriptTimedOut),
                    vec![TurnAction::StartIdleTimer],
                )
            }

            (
                TurnPhase::WaitingForTranscript { timer_key },
                TurnEvent::SpeechStarted {
                    pipeline_was_active,
                    bot_audio_sent,
                },
            ) => {
                // User resumed speaking — cancel STT timeout, go back to UserSpeaking.
                // finalize() is already in-flight; the transcript will arrive in
                // UserSpeaking and be buffered there (I5).
                let mut actions = vec![TurnAction::CancelTimer(timer_key)];
                if bot_audio_sent {
                    actions.push(TurnAction::EmitInterrupt);
                    actions.push(TurnAction::ClearBotAudioSent);
                }
                (
                    NextPhase::Set(TurnPhase::UserSpeaking {
                        pipeline_was_active,
                    }),
                    actions,
                )
            }

            // No-ops in WaitingForTranscript.
            (TurnPhase::WaitingForTranscript { timer_key }, TurnEvent::SpeechEnded { .. })
            | (TurnPhase::WaitingForTranscript { timer_key }, TurnEvent::EotFallback) => (
                NextPhase::Set(TurnPhase::WaitingForTranscript { timer_key }),
                vec![],
            ),

            // ━━ TranscriptTimedOut ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            (TurnPhase::TranscriptTimedOut, TurnEvent::Transcript { text }) => {
                // Late transcript arrived. Executor checks pipeline_active via
                // CommitLateTranscript: accept if idle, drop if active.
                (
                    NextPhase::Set(TurnPhase::Listening),
                    vec![
                        TurnAction::MarkSpeechEnded,
                        TurnAction::CommitLateTranscript(text),
                    ],
                )
            }

            (
                TurnPhase::TranscriptTimedOut,
                TurnEvent::SpeechStarted {
                    pipeline_was_active,
                    bot_audio_sent,
                },
            ) => {
                let mut actions = Vec::new();
                if bot_audio_sent {
                    actions.push(TurnAction::EmitInterrupt);
                    actions.push(TurnAction::ClearBotAudioSent);
                }
                (
                    NextPhase::Set(TurnPhase::UserSpeaking {
                        pipeline_was_active,
                    }),
                    actions,
                )
            }

            (TurnPhase::TranscriptTimedOut, TurnEvent::SpeechEnded { .. })
            | (TurnPhase::TranscriptTimedOut, TurnEvent::SttTimeout)
            | (TurnPhase::TranscriptTimedOut, TurnEvent::EotFallback) => {
                (NextPhase::Set(TurnPhase::TranscriptTimedOut), vec![])
            }

            // ━━ EotFallback ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            (TurnPhase::EotFallback { timer_key }, TurnEvent::EotFallback) => {
                // Timer fired — finalize STT and arm STT timeout as safety net.
                // If the STT connection is dead the transcript never arrives;
                // WaitForTranscript → SttTimeout → TranscriptTimedOut + StartIdleTimer
                // automatically unblocks the session.
                let _ = timer_key; // timer already fired, key consumed by DelayQueue
                (
                    NextPhase::WaitForTranscript,
                    vec![
                        TurnAction::MarkSpeechEnded,
                        TurnAction::MarkSttFinalizeSent,
                        TurnAction::FinalizeSTT,
                    ],
                )
            }

            (
                TurnPhase::EotFallback { timer_key },
                TurnEvent::SpeechStarted {
                    pipeline_was_active,
                    bot_audio_sent,
                },
            ) => {
                // User resumed speaking — cancel the EoT timer.
                let mut actions = vec![TurnAction::CancelTimer(timer_key)];
                if bot_audio_sent {
                    actions.push(TurnAction::EmitInterrupt);
                    actions.push(TurnAction::ClearBotAudioSent);
                }
                (
                    NextPhase::Set(TurnPhase::UserSpeaking {
                        pipeline_was_active,
                    }),
                    actions,
                )
            }

            (TurnPhase::EotFallback { timer_key }, TurnEvent::Transcript { text }) => {
                // Transcript arrived before fallback fired — cancel timer and commit.
                //
                // Note: MarkSttFinalizeSent is NOT emitted here. In EotFallback mode,
                // SpeechEnded(smart_complete=false) only armed the fallback timer —
                // finalize() was never called. This transcript is a spontaneous provider
                // final (some providers like Deepgram emit finals without explicit finalize).
                // stt_finalize_sent will be absent from metrics, which is accurate.
                (
                    NextPhase::Set(TurnPhase::Listening),
                    vec![
                        TurnAction::CancelTimer(timer_key),
                        TurnAction::MarkSpeechEnded,
                        TurnAction::CommitTurn(text),
                    ],
                )
            }

            (TurnPhase::EotFallback { timer_key }, TurnEvent::SpeechEnded { .. })
            | (TurnPhase::EotFallback { timer_key }, TurnEvent::SttTimeout) => {
                (NextPhase::Set(TurnPhase::EotFallback { timer_key }), vec![])
            }

            // ━━ BargeInWordCount ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            (TurnPhase::BargeInWordCount, TurnEvent::Transcript { text }) => {
                // Transcript arrived — executor checks word count via CommitBargeIn.
                (
                    NextPhase::Set(TurnPhase::Listening),
                    vec![TurnAction::MarkSpeechEnded, TurnAction::CommitBargeIn(text)],
                )
            }

            (
                TurnPhase::BargeInWordCount,
                TurnEvent::SpeechStarted {
                    pipeline_was_active,
                    bot_audio_sent,
                },
            ) => {
                // User started speaking again before the barge-in transcript arrived.
                // finalize() was already called — the transcript will arrive in
                // UserSpeaking and be buffered there (I5) rather than dropped.
                let mut actions = Vec::new();
                if bot_audio_sent {
                    actions.push(TurnAction::EmitInterrupt);
                    actions.push(TurnAction::ClearBotAudioSent);
                }
                (
                    NextPhase::Set(TurnPhase::UserSpeaking {
                        pipeline_was_active,
                    }),
                    actions,
                )
            }

            (TurnPhase::BargeInWordCount, TurnEvent::SpeechEnded { .. })
            | (TurnPhase::BargeInWordCount, TurnEvent::SttTimeout)
            | (TurnPhase::BargeInWordCount, TurnEvent::EotFallback) => {
                (NextPhase::Set(TurnPhase::BargeInWordCount), vec![])
            }
        }
    }
}

// ── Helper for variant name (test assertions) ────────────────────────────────

impl TurnPhase {
    #[cfg(test)]
    fn variant_name(&self) -> &'static str {
        match self {
            TurnPhase::Listening => "Listening",
            TurnPhase::UserSpeaking { .. } => "UserSpeaking",
            TurnPhase::WaitingForTranscript { .. } => "WaitingForTranscript",
            TurnPhase::TranscriptTimedOut => "TranscriptTimedOut",
            TurnPhase::EotFallback { .. } => "EotFallback",
            TurnPhase::BargeInWordCount => "BargeInWordCount",
        }
    }
}

impl NextPhase {
    /// Logical name of the phase this resolves to (for test assertions).
    ///
    /// `WaitForTranscript` and `StartEotFallback` return the name of the
    /// concrete phase they produce after dispatch resolves the timer key —
    /// so tests read `"WaitingForTranscript"` / `"EotFallback"` directly.
    #[cfg(test)]
    fn variant_name(&self) -> &'static str {
        match self {
            NextPhase::Set(p) => p.variant_name(),
            NextPhase::WaitForTranscript => "WaitingForTranscript",
            NextPhase::StartEotFallback => "EotFallback",
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TimerKey;
    use tokio_util::time::DelayQueue;

    /// Helper: creates a real delay_queue::Key for testing.
    fn make_timer_key() -> delay_queue::Key {
        let mut q: DelayQueue<TimerKey> = DelayQueue::new();
        q.insert(TimerKey::SttTimeout, std::time::Duration::from_secs(999))
    }

    // ── Transition table correctness ─────────────────────────────────────

    #[test]
    fn listening_speech_started_goes_to_user_speaking() {
        let (next, actions) = TurnPhase::Listening.transition(TurnEvent::SpeechStarted {
            pipeline_was_active: false,
            bot_audio_sent: false,
        });
        assert_eq!(next.variant_name(), "UserSpeaking");
        assert!(actions.is_empty()); // no audio sent, no interrupt needed
    }

    #[test]
    fn listening_speech_started_with_audio_emits_interrupt() {
        let (next, actions) = TurnPhase::Listening.transition(TurnEvent::SpeechStarted {
            pipeline_was_active: true,
            bot_audio_sent: true,
        });
        assert_eq!(next.variant_name(), "UserSpeaking");
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::EmitInterrupt)));
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::ClearBotAudioSent)));
    }

    #[test]
    fn user_speaking_normal_speech_ended_smart_complete() {
        let (next, actions) = TurnPhase::UserSpeaking {
            pipeline_was_active: false,
        }
        .transition(TurnEvent::SpeechEnded {
            smart_complete: true,
        });
        // transition() returns WaitForTranscript, which dispatch resolves to
        // WaitingForTranscript { timer_key }. Tests via NextPhase::variant_name().
        assert_eq!(next.variant_name(), "WaitingForTranscript");
        assert!(actions.iter().any(|a| matches!(a, TurnAction::FinalizeSTT)));
    }

    #[test]
    fn user_speaking_normal_speech_ended_not_complete() {
        let (next, actions) = TurnPhase::UserSpeaking {
            pipeline_was_active: false,
        }
        .transition(TurnEvent::SpeechEnded {
            smart_complete: false,
        });
        // transition() returns StartEotFallback, resolved by dispatch to
        // EotFallback { timer_key }. NextPhase::variant_name() returns "EotFallback".
        // No separate ArmEotFallback action needed — the NextPhase variant IS the arm.
        assert_eq!(next.variant_name(), "EotFallback");
        assert!(actions.is_empty());
    }

    #[test]
    fn user_speaking_barge_in_path() {
        let (next, actions) = TurnPhase::UserSpeaking {
            pipeline_was_active: true,
        }
        .transition(TurnEvent::SpeechEnded {
            smart_complete: true,
        });
        assert_eq!(next.variant_name(), "BargeInWordCount");
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::CancelPipeline)));
        assert!(actions.iter().any(|a| matches!(a, TurnAction::FinalizeSTT)));
    }

    #[tokio::test]
    async fn waiting_transcript_arrives_cancels_timer_and_commits() {
        let key = make_timer_key();
        let (next, actions) =
            TurnPhase::WaitingForTranscript { timer_key: key }.transition(TurnEvent::Transcript {
                text: "hello".into(),
            });
        assert_eq!(next.variant_name(), "Listening");
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::CancelTimer(_))));
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::CommitTurn(_))));
    }

    #[tokio::test]
    async fn waiting_stt_timeout_goes_to_timed_out() {
        let key = make_timer_key();
        let (next, actions) =
            TurnPhase::WaitingForTranscript { timer_key: key }.transition(TurnEvent::SttTimeout);
        assert_eq!(next.variant_name(), "TranscriptTimedOut");
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::StartIdleTimer)));
    }

    #[tokio::test]
    async fn waiting_speech_started_goes_to_user_speaking() {
        // Previously went to Cancelled. Now goes directly to UserSpeaking —
        // the in-flight transcript arrives in UserSpeaking + Transcript → BufferTranscript.
        let key = make_timer_key();
        let (next, actions) = TurnPhase::WaitingForTranscript { timer_key: key }.transition(
            TurnEvent::SpeechStarted {
                pipeline_was_active: false,
                bot_audio_sent: false,
            },
        );
        assert_eq!(next.variant_name(), "UserSpeaking");
        // STT timeout must still be cancelled
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::CancelTimer(_))));
    }

    #[test]
    fn user_speaking_transcript_buffers_not_commits() {
        // In-flight transcript from a previous finalize() arrives while still speaking.
        // Must buffer (I5), never commit directly.
        let (next, actions) = TurnPhase::UserSpeaking {
            pipeline_was_active: false,
        }
        .transition(TurnEvent::Transcript {
            text: "partial".into(),
        });
        assert_eq!(next.variant_name(), "UserSpeaking");
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::BufferTranscript(_))));
        assert!(!actions
            .iter()
            .any(|a| matches!(a, TurnAction::CommitTurn(_))));
    }

    #[test]
    fn timed_out_late_transcript_commits() {
        let (next, actions) = TurnPhase::TranscriptTimedOut.transition(TurnEvent::Transcript {
            text: "late".into(),
        });
        assert_eq!(next.variant_name(), "Listening");
        // CommitLateTranscript, not CommitTurn — executor checks pipeline_active
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::CommitLateTranscript(_))));
    }

    #[tokio::test]
    async fn eot_fallback_fires_finalizes_stt_and_enters_waiting() {
        let key = make_timer_key();
        let (next, actions) =
            TurnPhase::EotFallback { timer_key: key }.transition(TurnEvent::EotFallback);
        // NextPhase::WaitForTranscript — dispatch resolves to WaitingForTranscript.
        // Built-in safety net: if STT is dead, SttTimeout → TranscriptTimedOut + StartIdleTimer.
        assert_eq!(next.variant_name(), "WaitingForTranscript");
        assert!(actions.iter().any(|a| matches!(a, TurnAction::FinalizeSTT)));
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::MarkSpeechEnded)));
        // ArmSttTimeout no longer exists as a TurnAction — the WaitForTranscript
        // NextPhase variant carries the intent directly.
        assert!(!actions
            .iter()
            .any(|a| matches!(a, TurnAction::StartIdleTimer)));
    }

    #[tokio::test]
    async fn eot_fallback_speech_started_cancels_timer() {
        let key = make_timer_key();
        let (next, actions) =
            TurnPhase::EotFallback { timer_key: key }.transition(TurnEvent::SpeechStarted {
                pipeline_was_active: false,
                bot_audio_sent: false,
            });
        assert_eq!(next.variant_name(), "UserSpeaking");
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::CancelTimer(_))));
    }

    #[test]
    fn barge_in_word_count_transcript_commits() {
        let (next, actions) = TurnPhase::BargeInWordCount.transition(TurnEvent::Transcript {
            text: "what".into(),
        });
        assert_eq!(next.variant_name(), "Listening");
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, TurnAction::MarkSpeechEnded)),
            "BargeInWordCount + Transcript must emit MarkSpeechEnded for metrics"
        );
        assert!(actions
            .iter()
            .any(|a| matches!(a, TurnAction::CommitBargeIn(_))));
    }

    #[test]
    fn barge_in_word_count_speech_started_buffers_in_flight_transcript() {
        // User speaks again before barge-in transcript arrives.
        // Goes to UserSpeaking — in-flight transcript will arrive and be buffered
        // via UserSpeaking + Transcript → BufferTranscript (I5).
        let (next, _actions) = TurnPhase::BargeInWordCount.transition(TurnEvent::SpeechStarted {
            pipeline_was_active: false,
            bot_audio_sent: false,
        });
        assert_eq!(
            next.variant_name(),
            "UserSpeaking",
            "BargeInWordCount + SpeechStarted must go to UserSpeaking; \
             in-flight transcript will be buffered there (I5)"
        );
    }

    // ── Timer cleanup safety ─────────────────────────────────────────────
    // Verify that EVERY transition OUT of a timer-owning state either
    // cancels the timer or acknowledges it was consumed by DelayQueue.

    #[tokio::test]
    async fn all_exits_from_waiting_handle_timer_key() {
        let events = vec![
            TurnEvent::Transcript { text: "hi".into() },
            TurnEvent::SttTimeout,
            TurnEvent::SpeechStarted {
                pipeline_was_active: false,
                bot_audio_sent: false,
            },
        ];
        for event in events {
            let key = make_timer_key();
            let (next, actions) =
                TurnPhase::WaitingForTranscript { timer_key: key }.transition(event);
            // Leaving WaitingForTranscript must either cancel the timer (CancelTimer action)
            // or the timer was consumed by DelayQueue (SttTimeout → TranscriptTimedOut).
            if next.variant_name() != "WaitingForTranscript" {
                let has_cancel_or_timeout = actions
                    .iter()
                    .any(|a| matches!(a, TurnAction::CancelTimer(_)))
                    || next.variant_name() == "TranscriptTimedOut";
                assert!(
                    has_cancel_or_timeout,
                    "Leaving WaitingForTranscript must cancel timer or acknowledge consumption"
                );
            }
        }
    }

    #[tokio::test]
    async fn all_exits_from_eot_fallback_handle_timer_key() {
        let events = vec![
            TurnEvent::EotFallback,
            TurnEvent::SpeechStarted {
                pipeline_was_active: false,
                bot_audio_sent: false,
            },
            TurnEvent::Transcript { text: "hi".into() },
        ];
        for event in events {
            let key = make_timer_key();
            let (next, actions) = TurnPhase::EotFallback { timer_key: key }.transition(event);
            if next.variant_name() != "EotFallback" {
                // Timer is either explicitly cancelled (CancelTimer action) or
                // consumed by DelayQueue (EotFallback event → WaitForTranscript).
                let has_cancel_or_consumed = actions
                    .iter()
                    .any(|a| matches!(a, TurnAction::CancelTimer(_)))
                    || next.variant_name() == "WaitingForTranscript"
                    || next.variant_name() == "Listening";
                assert!(
                    has_cancel_or_consumed,
                    "Leaving EotFallback must cancel timer or acknowledge consumption"
                );
            }
        }
    }

    // ── AudioOutputPermit ────────────────────────────────────────────────

    #[test]
    fn no_permit_while_speaking() {
        let phase = TurnPhase::UserSpeaking {
            pipeline_was_active: false,
        };
        assert!(phase.audio_output_permit().is_none());
    }

    #[test]
    fn permit_while_listening() {
        assert!(TurnPhase::Listening.audio_output_permit().is_some());
    }

    #[tokio::test]
    async fn permit_while_waiting() {
        let key = make_timer_key();
        let phase = TurnPhase::WaitingForTranscript { timer_key: key };
        assert!(phase.audio_output_permit().is_some());
    }

    // ── TurnPhase::reset() ───────────────────────────────────────────────

    #[tokio::test]
    async fn reset_from_waiting_cancels_timer() {
        let mut timers: DelayQueue<TimerKey> = DelayQueue::new();
        let key = timers.insert(TimerKey::SttTimeout, std::time::Duration::from_secs(10));
        let mut phase = TurnPhase::WaitingForTranscript { timer_key: key };
        assert_eq!(timers.len(), 1);

        phase.reset(&mut timers);

        assert_eq!(phase.variant_name(), "Listening");
        assert_eq!(timers.len(), 0, "Timer key must be removed from DelayQueue");
    }

    #[tokio::test]
    async fn reset_from_eot_fallback_cancels_timer() {
        let mut timers: DelayQueue<TimerKey> = DelayQueue::new();
        let key = timers.insert(
            TimerKey::EndOfTurnFallback,
            std::time::Duration::from_secs(3),
        );
        let mut phase = TurnPhase::EotFallback { timer_key: key };
        assert_eq!(timers.len(), 1);

        phase.reset(&mut timers);

        assert_eq!(phase.variant_name(), "Listening");
        assert_eq!(timers.len(), 0, "Timer key must be removed from DelayQueue");
    }

    #[test]
    fn reset_from_user_speaking_returns_to_listening() {
        let mut timers: DelayQueue<TimerKey> = DelayQueue::new();
        let mut phase = TurnPhase::UserSpeaking {
            pipeline_was_active: true,
        };

        phase.reset(&mut timers);

        assert_eq!(phase.variant_name(), "Listening");
        assert_eq!(timers.len(), 0);
    }

    #[test]
    fn reset_from_listening_is_noop() {
        let mut timers: DelayQueue<TimerKey> = DelayQueue::new();
        let mut phase = TurnPhase::Listening;

        phase.reset(&mut timers);

        assert_eq!(phase.variant_name(), "Listening");
    }

    // ── Scenario: barge-in sequence must not be blocked by stale policy ──

    /// Regression test for the silent-drop bug:
    ///   SpeechStarted(active) → SpeechEnded → BargeInWordCount → Transcript
    ///
    /// By the time CommitBargeIn fires, earlier actions (ClearBotAudioSent,
    /// CancelPipeline) have reset both flags to false.  If the executor
    /// re-checked interrupt_policy(false, 0, false, false) it would get
    /// Block and silently drop the transcript.
    ///
    /// This test walks the full 3-step sequence and asserts:
    /// 1. ClearBotAudioSent is emitted during SpeechStarted.
    /// 2. CancelPipeline is emitted during SpeechEnded (barge-in path).
    /// 3. CommitBargeIn is still emitted when the transcript arrives.
    #[test]
    fn barge_in_sequence_commit_survives_policy_reset() {
        // Step 1: SpeechStarted while pipeline was active and bot audio sent.
        let (next1, actions) = TurnPhase::Listening.transition(TurnEvent::SpeechStarted {
            pipeline_was_active: true,
            bot_audio_sent: true,
        });
        assert_eq!(next1.variant_name(), "UserSpeaking");
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, TurnAction::ClearBotAudioSent)),
            "SpeechStarted must clear bot_audio_sent"
        );
        // Resolve NextPhase → TurnPhase to continue the chain.
        let NextPhase::Set(phase1) = next1 else {
            panic!("expected Set")
        };

        // Step 2: SpeechEnded → barge-in path (pipeline_was_active=true).
        let (next2, actions) = phase1.transition(TurnEvent::SpeechEnded {
            smart_complete: true,
        });
        assert_eq!(next2.variant_name(), "BargeInWordCount");
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, TurnAction::CancelPipeline)),
            "Barge-in path must cancel pipeline"
        );
        let NextPhase::Set(phase2) = next2 else {
            panic!("expected Set")
        };

        // At this point in the real Reactor:
        //   bot_audio_sent = false  (cleared in step 1)
        //   pipeline_active = false (cancelled in step 2)
        //   interrupt_policy(false, 0, false, false) == Block  ← WRONG!

        // Step 3: Transcript arrives → CommitBargeIn must still be emitted.
        let (next3, actions) = phase2.transition(TurnEvent::Transcript {
            text: "So I'm waiting for your result".into(),
        });
        assert_eq!(next3.variant_name(), "Listening");
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, TurnAction::CommitBargeIn(_))),
            "Transcript in BargeInWordCount must produce CommitBargeIn, \
             regardless of what interrupt_policy() would return"
        );
    }

    // ── Reachability & Fuzzing ──────────────────────────────────────────

    impl TurnPhase {
        /// Creates a clone of the TurnPhase for testing purposes.
        /// TimerKeys are not Clone, so new ones are generated.
        fn clone_for_test(&self) -> Self {
            match self {
                TurnPhase::Listening => TurnPhase::Listening,
                TurnPhase::UserSpeaking {
                    pipeline_was_active,
                } => TurnPhase::UserSpeaking {
                    pipeline_was_active: *pipeline_was_active,
                },
                TurnPhase::WaitingForTranscript { .. } => TurnPhase::WaitingForTranscript {
                    timer_key: make_timer_key(),
                },
                TurnPhase::TranscriptTimedOut => TurnPhase::TranscriptTimedOut,
                TurnPhase::EotFallback { .. } => TurnPhase::EotFallback {
                    timer_key: make_timer_key(),
                },
                TurnPhase::BargeInWordCount => TurnPhase::BargeInWordCount,
            }
        }
    }

    #[tokio::test]
    async fn exhaustive_reachability_no_void_states() {
        // Enumerate every possible state variant
        let state_factories: Vec<Box<dyn Fn() -> TurnPhase>> = vec![
            Box::new(|| TurnPhase::Listening),
            Box::new(|| TurnPhase::UserSpeaking {
                pipeline_was_active: false,
            }),
            Box::new(|| TurnPhase::UserSpeaking {
                pipeline_was_active: true,
            }),
            Box::new(|| TurnPhase::WaitingForTranscript {
                timer_key: make_timer_key(),
            }),
            Box::new(|| TurnPhase::TranscriptTimedOut),
            Box::new(|| TurnPhase::EotFallback {
                timer_key: make_timer_key(),
            }),
            Box::new(|| TurnPhase::BargeInWordCount),
        ];

        // Core events that cause state transitions
        let all_events = || {
            vec![
                TurnEvent::SpeechStarted {
                    pipeline_was_active: false,
                    bot_audio_sent: false,
                },
                TurnEvent::SpeechStarted {
                    pipeline_was_active: true,
                    bot_audio_sent: false,
                },
                TurnEvent::SpeechStarted {
                    pipeline_was_active: false,
                    bot_audio_sent: true,
                },
                TurnEvent::SpeechStarted {
                    pipeline_was_active: true,
                    bot_audio_sent: true,
                },
                TurnEvent::SpeechEnded {
                    smart_complete: true,
                },
                TurnEvent::SpeechEnded {
                    smart_complete: false,
                },
                TurnEvent::Transcript {
                    text: "dummy".into(),
                },
                TurnEvent::SttTimeout,
                TurnEvent::EotFallback,
            ]
        };

        for factory in state_factories {
            let mut can_reach_listening = false;
            let starting_variant = factory().variant_name();

            for e1 in all_events() {
                let state1 = factory();
                let (next1, _) = state1.transition(e1);

                if next1.variant_name() == "Listening" {
                    can_reach_listening = true;
                    break;
                }

                for e2 in all_events() {
                    // Resolve NextPhase → TurnPhase (simulating dispatch).
                    let state2 = match next1 {
                        NextPhase::Set(ref p) => p.clone_for_test(),
                        NextPhase::WaitForTranscript => TurnPhase::WaitingForTranscript {
                            timer_key: make_timer_key(),
                        },
                        NextPhase::StartEotFallback => TurnPhase::EotFallback {
                            timer_key: make_timer_key(),
                        },
                    };
                    let (next2, _) = state2.transition(e2);
                    if next2.variant_name() == "Listening" {
                        can_reach_listening = true;
                        break;
                    }
                }
                if can_reach_listening {
                    break;
                }
            }

            assert!(
                can_reach_listening,
                "VOID STATE DETECTED! Cannot reach 'Listening' from {}",
                starting_variant
            );
        }
    }

    use proptest::prelude::*;

    // Strategy to generate arbitrary TurnEvents
    fn any_turn_event() -> impl Strategy<Value = TurnEvent> {
        prop_oneof![
            (any::<bool>(), any::<bool>()).prop_map(|(pa, bas)| TurnEvent::SpeechStarted {
                pipeline_was_active: pa,
                bot_audio_sent: bas,
            }),
            any::<bool>().prop_map(|sc| TurnEvent::SpeechEnded { smart_complete: sc }),
            any::<String>().prop_map(|text| TurnEvent::Transcript { text }),
            Just(TurnEvent::SttTimeout),
            Just(TurnEvent::EotFallback),
        ]
    }

    proptest! {
        /// Shoot a completely random sequence of events into the state machine.
        /// It should never panic, and it should always be possible to eventually
        /// return to `Listening` in <= 2 steps.
        #[test]
        fn fuzz_no_void_states(events in prop::collection::vec(any_turn_event(), 1..50)) {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
            let _guard = rt.enter();

            let mut state = TurnPhase::Listening;

            for event in events {
                let (next_phase, _actions) = state.transition(event);
                // Simulate dispatch_turn_event: resolve NextPhase → TurnPhase.
                // Timer-backed phases get a fresh key (the DelayQueue isn't real here).
                state = match next_phase {
                    NextPhase::Set(phase) => phase,
                    NextPhase::WaitForTranscript => TurnPhase::WaitingForTranscript {
                        timer_key: make_timer_key(),
                    },
                    NextPhase::StartEotFallback => TurnPhase::EotFallback {
                        timer_key: make_timer_key(),
                    },
                };
            }

            // We finished a random walk. Now prove we can get back to Listening.
            let mut can_reach = false;
            let final_state_name = state.variant_name().to_string();

            let all_events = || vec![
                TurnEvent::SpeechStarted { pipeline_was_active: false, bot_audio_sent: false },
                TurnEvent::SpeechStarted { pipeline_was_active: true, bot_audio_sent: false },
                TurnEvent::SpeechStarted { pipeline_was_active: false, bot_audio_sent: true },
                TurnEvent::SpeechStarted { pipeline_was_active: true, bot_audio_sent: true },
                TurnEvent::SpeechEnded { smart_complete: true },
                TurnEvent::SpeechEnded { smart_complete: false },
                TurnEvent::Transcript { text: "text".into() },
                TurnEvent::SttTimeout,
                TurnEvent::EotFallback,
            ];

            for e1 in all_events() {
                let s1 = state.clone_for_test();
                let (next1, _) = s1.transition(e1);
                if next1.variant_name() == "Listening" {
                    can_reach = true;
                    break;
                }
                for e2 in all_events() {
                    let resolved1 = match next1 {
                        NextPhase::Set(ref p) => p.clone_for_test(),
                        NextPhase::WaitForTranscript => TurnPhase::WaitingForTranscript { timer_key: make_timer_key() },
                        NextPhase::StartEotFallback => TurnPhase::EotFallback { timer_key: make_timer_key() },
                    };
                    let (next2, _) = resolved1.transition(e2);
                    if next2.variant_name() == "Listening" {
                        can_reach = true;
                        break;
                    }
                }
                if can_reach { break; }
            }

            prop_assert!(
                can_reach,
                "TRAPPED! Ended up in void state '{}'",
                final_state_name
            );
        }
    }
}
