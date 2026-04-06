//! Unified input types for the [`Reactor`](super::Reactor) event loop.
//!
//! ## How things fit together
//!
//! ```text
//! transport ─→ audio_rx ─┐
//! STT stage  ────────────┤
//! LLM stage  ────────────┤   select! loop
//! TTS stage  ────────────┼───────────────→ on_*(event) handlers
//! WS TTS rx  ────────────┤
//! DelayQueue ────────────┤
//! HangUp saga ───────────┤
//! ToolFiller saga ───────┘
//! ```
//!
//! [`ReactorInput`] is a plain enum that mirrors the 8 `select!` arms.
//! It is **not** used by the live reactor loop (the loop uses native
//! channels directly for performance); its purpose is:
//!
//! 1. **Type-safe documentation** of every event the reactor can receive.
//! 2. **Event recording** — [`ReplayLog`] captures a session's input stream
//!    for post-mortem debugging.
//! 3. **Simulation** — [`crate::reactor::testing::sim`] replays a `ReplayLog` in
//!    microseconds, without audio hardware, for fast integration tests.
//!
//! ## How to add a new input source
//!
//! 1. Add a variant to `ReactorInput`.
//! 2. Add an arm in the `select!` loop in `reactor/mod.rs`.
//! 3. Call `self.replay_log.record(ReactorInput::YourVariant { .. })` there
//!    (only when recording is enabled).
//! 4. Add a replay arm in `reactor/sim.rs`.

use std::time::Instant;

use crate::types::{LlmEvent, SttEvent, TimerKey, TtsEvent};

/// Every event that can enter the Reactor's `select!` loop.
///
/// Audio frames are represented as metadata only (`AudioIn { samples }`)
/// to avoid storing multi-megabyte PCM buffers in recordings.
#[derive(Debug, Clone)]
pub enum ReactorInput {
    // ── Transport ─────────────────────────────────────────────────
    /// An audio frame arrived from the transport (client microphone audio).
    /// Stored as `samples` count only — raw PCM is not recorded.
    AudioIn {
        /// Number of PCM samples in the frame.
        samples: u32,
    },
    /// The transport closed (WebSocket / WebRTC disconnected).
    TransportClosed,

    // ── STT ───────────────────────────────────────────────────────
    /// An event from the STT stage.
    SttEvent(SttEvent),

    // ── LLM ───────────────────────────────────────────────────────
    /// An event from the LLM stage.
    LlmEvent(LlmEvent),

    // ── TTS ───────────────────────────────────────────────────────
    /// A non-audio event from the TTS stage.
    /// `TtsEvent::Audio` chunks are dropped (too large); only
    /// `Finished` and `Error` carry replay value.
    TtsEvent(TtsEvent),

    // ── Timers ────────────────────────────────────────────────────
    /// A DelayQueue timer fired.
    TimerFired(TimerKey),
}

impl ReactorInput {
    /// Short span label for telemetry.
    ///
    /// Used by [`super::Reactor::observe`] so the trace span and the replay
    /// log record are always derived from the same variant — no manual strings.
    pub fn span_name(&self) -> &'static str {
        match self {
            Self::AudioIn { .. } => "AudioIn",
            Self::TransportClosed => "TransportClosed",
            Self::SttEvent(_) => "SttEvent",
            Self::LlmEvent(_) => "LlmEvent",
            Self::TtsEvent(_) => "TtsEvent",
            Self::TimerFired(_) => "Timer",
        }
    }
}

/// A timestamped recording of a [`ReactorInput`] event.
#[derive(Debug, Clone)]
pub struct ReplayEntry {
    /// Wall-clock offset from the first event in the log.
    pub elapsed_ms: u64,
    /// The event that was received.
    pub input: ReactorInput,
}

/// Append-only recording of a reactor session's input stream.
///
/// Disabled (zero-cost) by default — recording is opt-in via
/// `Reactor::enable_recording()`. When enabled, every `record()` call
/// pushes a [`ReplayEntry`] with a millisecond timestamp offset.
///
/// The log can be serialized or passed to [`crate::reactor::testing::sim`] for replay.
#[derive(Default)]
pub struct ReplayLog {
    events: Vec<ReplayEntry>,
    start: Option<Instant>,
    enabled: bool,
}

impl ReplayLog {
    /// Create a new, **disabled** log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable recording. All subsequent `record()` calls will store events.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Returns true if recording is currently active.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Record one event. No-op when the log is disabled.
    #[inline]
    pub fn record(&mut self, input: ReactorInput) {
        if !self.enabled {
            return;
        }
        let start = self.start.get_or_insert_with(Instant::now);
        let elapsed_ms = start.elapsed().as_millis() as u64;
        self.events.push(ReplayEntry { elapsed_ms, input });
    }

    /// Return a slice of all recorded events.
    pub fn entries(&self) -> &[ReplayEntry] {
        &self.events
    }

    /// Drain all events, consuming the log contents.
    ///
    /// Resets the timestamp origin so the next recorded event starts at `0 ms`.
    /// **Does not disable recording** — subsequent `record()` calls continue
    /// appending. This supports "drain-and-continue" patterns such as
    /// checkpointing mid-session. Call `ReplayLog::new()` or replace with
    /// `Default::default()` if you want to stop recording after draining.
    pub fn drain(&mut self) -> Vec<ReplayEntry> {
        self.start = None;
        std::mem::take(&mut self.events)
    }

    /// Number of recorded events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// True if no events have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TimerKey;

    #[test]
    fn disabled_log_records_nothing() {
        let mut log = ReplayLog::new();
        log.record(ReactorInput::TimerFired(TimerKey::UserIdle));
        assert!(log.is_empty());
    }

    #[test]
    fn enabled_log_records_events() {
        let mut log = ReplayLog::new();
        log.enable();
        log.record(ReactorInput::TimerFired(TimerKey::UserIdle));
        log.record(ReactorInput::SttEvent(SttEvent::Transcript("hi".into())));
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn drain_clears_log() {
        let mut log = ReplayLog::new();
        log.enable();
        log.record(ReactorInput::TransportClosed);
        let drained = log.drain();
        assert_eq!(drained.len(), 1);
        assert!(log.is_empty());
    }

    #[test]
    fn timestamps_are_non_decreasing() {
        let mut log = ReplayLog::new();
        log.enable();
        for _ in 0..5 {
            log.record(ReactorInput::TimerFired(TimerKey::UserIdle));
        }
        let events = log.entries();
        for window in events.windows(2) {
            assert!(window[1].elapsed_ms >= window[0].elapsed_ms);
        }
    }

    #[test]
    fn audio_in_records_samples_not_bytes() {
        let mut log = ReplayLog::new();
        log.enable();
        log.record(ReactorInput::AudioIn { samples: 512 });
        match &log.entries()[0].input {
            ReactorInput::AudioIn { samples } => assert_eq!(*samples, 512),
            _ => panic!("wrong variant"),
        }
    }
}
