//! Simulation / replay harness for the Reactor.
//!
//! [`SimReactor`] drives a real [`Reactor`] with a scripted sequence of
//! [`ReactorInput`] events instead of live audio / network I/O. A full
//! session runs in microseconds — no tokio runtime required for most tests.
//!
//! ## How to write a simulation test
//!
//! Implement [`Simulatable`] for your reactor type, then drive it with a
//! scripted event list:
//!
//! ```rust,ignore
//! use voice_engine::reactor::testing::sim::{SimReactor, SimEvent};
//! use voice_engine::types::{LlmEvent, SttEvent};
//!
//! #[tokio::test]
//! async fn happy_path_one_turn() {
//!     let events = vec![
//!         SimEvent::stt(SttEvent::Transcript("book a table".into())),
//!         SimEvent::llm(LlmEvent::Token("Sure!".into())),
//!         SimEvent::llm(LlmEvent::Finished { content: Some("Sure!".into()) }),
//!         SimEvent::tts_finished(),
//!         SimEvent::close(),
//!     ];
//!
//!     let mut reactor = MySimulatableReactor::new();
//!     let outcome = SimReactor::new(events).run(&mut reactor).await;
//!
//!     assert_eq!(outcome.turn_count, 1);
//!     assert!(!outcome.should_exit);
//! }
//! ```
//!
//! ## Design notes
//!
//! - **No actual audio**: audio-frame bypass is not yet modelled — all events
//!   operate at the post-VAD level (STT, LLM, TTS, timers).
//! - **Deterministic**: events are processed in-order with no scheduler.
//! - **Trait-based**: any type implementing [`Simulatable`] can be driven,
//!   keeping the harness independent of production reactor internals.

// This module is integration-test infrastructure. None of its items are
// called from production code, which triggers dead_code warnings while
// the first real reactor integration tests are still being written.
#![allow(dead_code)]

use crate::types::{LlmEvent, SttEvent, TimerKey, TtsEvent};

/// A single scripted event for use with [`SimReactor`].
///
/// Build these from the convenience constructors rather than naming variants directly.
#[derive(Debug, Clone)]
pub enum SimEvent {
    /// Drive an STT event directly into `on_stt_event`.
    Stt(SttEvent),
    /// Drive an LLM event directly into `on_llm_event`.
    Llm(LlmEvent),
    /// Drive a TTS event directly into `on_tts_event`.
    Tts(TtsEvent),
    /// Fire a specific timer key.
    Timer(TimerKey),
    /// Signal transport closed (causes the reactor loop to exit).
    TransportClosed,
}

impl SimEvent {
    /// Convenience: create an STT event.
    pub fn stt(ev: SttEvent) -> Self {
        SimEvent::Stt(ev)
    }
    /// Convenience: create an LLM event.
    pub fn llm(ev: LlmEvent) -> Self {
        SimEvent::Llm(ev)
    }
    /// Convenience: TTS finished.
    pub fn tts_finished() -> Self {
        SimEvent::Tts(TtsEvent::Finished)
    }
    /// Convenience: TTS error.
    pub fn tts_error(msg: impl Into<String>) -> Self {
        SimEvent::Tts(TtsEvent::Error(msg.into()))
    }
    /// Convenience: fire a timer.
    pub fn timer(key: TimerKey) -> Self {
        SimEvent::Timer(key)
    }
    /// Convenience: close transport.
    pub fn close() -> Self {
        SimEvent::TransportClosed
    }
}

/// Outcome recorded after running a simulation.
///
/// Returned by [`SimReactor::run`] so tests can assert on session-level state.
#[derive(Debug)]
pub struct SimOutcome {
    /// Number of complete LLM turns that occurred.
    pub turn_count: u32,
    /// Whether `should_exit` was set (agent-initiated hang-up).
    pub should_exit: bool,
    /// Whether the transport closed during the run.
    pub transport_closed: bool,
}

/// Simulation harness for a Reactor.
///
/// Drives the reactor with a scripted sequence of events, capturing the
/// resulting state for assertion in tests. See module-level docs for usage.
pub struct SimReactor {
    events: Vec<SimEvent>,
}

impl SimReactor {
    /// Create a simulation from a sequence of events.
    ///
    /// Events are processed in-order. The simulation stops after the last
    /// event or when `TransportClosed` is encountered.
    pub fn new(events: Vec<SimEvent>) -> Self {
        Self { events }
    }

    /// Run the simulation against a real Reactor, returning outcome state for assertion.
    ///
    /// The reactor is driven by calling each handler method directly:
    /// - `Stt(ev)` → `on_stt_event(ev)`
    /// - `Llm(ev)` → `on_llm_event(ev)`
    /// - `Tts(ev)` → `on_tts_event(ev)`
    /// - `Timer(key)` → `on_timer(key)`
    /// - `TransportClosed` → cancels pipeline, breaks loop
    pub async fn run<R: Simulatable>(self, reactor: &mut R) -> SimOutcome {
        let mut transport_closed = false;

        for event in self.events {
            if reactor.should_exit() {
                break;
            }
            match event {
                SimEvent::Stt(ev) => reactor.drive_stt(ev).await,
                SimEvent::Llm(ev) => reactor.drive_llm(ev).await,
                SimEvent::Tts(ev) => reactor.drive_tts(ev).await,
                SimEvent::Timer(key) => reactor.drive_timer(key).await,
                SimEvent::TransportClosed => {
                    transport_closed = true;
                    reactor.drive_transport_closed();
                    break;
                }
            }
        }

        SimOutcome {
            turn_count: reactor.turn_count(),
            should_exit: reactor.should_exit(),
            transport_closed,
        }
    }
}

/// Trait implemented by types that can be driven by [`SimReactor`].
///
/// Implemented by `Reactor` in the `reactor::tests` module so the sim can
/// call into private handlers. Not part of the public API.
#[async_trait::async_trait]
pub trait Simulatable {
    async fn drive_stt(&mut self, ev: SttEvent);
    async fn drive_llm(&mut self, ev: LlmEvent);
    async fn drive_tts(&mut self, ev: TtsEvent);
    async fn drive_timer(&mut self, key: TimerKey);
    fn drive_transport_closed(&mut self);
    fn turn_count(&self) -> u32;
    fn should_exit(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SttEvent;

    /// Minimal stub that implements Simulatable for unit-testing SimReactor's
    /// event dispatch logic without a full Reactor.
    struct StubReactor {
        stt_count: u32,
        llm_count: u32,
        tts_count: u32,
        timer_count: u32,
        closed: bool,
        exit: bool,
    }

    impl StubReactor {
        fn new() -> Self {
            Self {
                stt_count: 0,
                llm_count: 0,
                tts_count: 0,
                timer_count: 0,
                closed: false,
                exit: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl Simulatable for StubReactor {
        async fn drive_stt(&mut self, _: SttEvent) {
            self.stt_count += 1;
        }
        async fn drive_llm(&mut self, _: LlmEvent) {
            self.llm_count += 1;
        }
        async fn drive_tts(&mut self, _: TtsEvent) {
            self.tts_count += 1;
        }
        async fn drive_timer(&mut self, _: TimerKey) {
            self.timer_count += 1;
        }
        fn drive_transport_closed(&mut self) {
            self.closed = true;
        }
        fn turn_count(&self) -> u32 {
            self.llm_count
        }
        fn should_exit(&self) -> bool {
            self.exit
        }
    }

    #[tokio::test]
    async fn sim_dispatches_all_event_types() {
        let events = vec![
            SimEvent::stt(SttEvent::Transcript("hello".into())),
            SimEvent::llm(LlmEvent::Token("Hi".into())),
            SimEvent::tts_finished(),
            SimEvent::timer(TimerKey::UserIdle),
            SimEvent::close(),
        ];

        let mut stub = StubReactor::new();
        let outcome = SimReactor::new(events).run(&mut stub).await;

        assert_eq!(stub.stt_count, 1);
        assert_eq!(stub.llm_count, 1);
        assert_eq!(stub.tts_count, 1);
        assert_eq!(stub.timer_count, 1);
        assert!(outcome.transport_closed);
    }

    #[tokio::test]
    async fn transport_closed_breaks_loop_immediately() {
        let events = vec![
            SimEvent::close(),
            SimEvent::stt(SttEvent::Transcript("never".into())),
        ];

        let mut stub = StubReactor::new();
        let outcome = SimReactor::new(events).run(&mut stub).await;

        assert!(outcome.transport_closed);
        assert_eq!(stub.stt_count, 0);
    }
}
