//! Behavioral policy traits for the [`Reactor`](crate::reactor::Reactor).
//!
//! Policies separate *behavioral decisions* from the Reactor core loop.
//! Each policy is a small trait object the Reactor calls at a specific
//! decision point. The default implementations preserve the existing
//! hardcoded behavior, so swapping them is purely additive.
//!
//! ## How to add a new policy
//!
//! 1. Create `src/reactor/policies/your_policy.rs` with:
//!    - A `pub trait YourPolicy: Send + Sync + 'static { ... }` (single method, pure).
//!    - A `#[derive(Default)] pub struct DefaultYourPolicy;` impl.
//!    - Unit tests for the default impl.
//! 2. Add `pub mod your_policy;` below and re-export the trait + default struct.
//! 3. Add a `pub(super) your_policy: Box<dyn YourPolicy>` field to `Reactor`
//!    in `reactor/mod.rs`.
//! 4. Initialize it with `Box::new(DefaultYourPolicy)` in `Reactor::new()`.
//! 5. Replace the hardcoded decision in the reactor with
//!    `self.your_policy.method(ctx)`.
//!
//! ## Policy vs Hook
//!
//! |             | Policy                    | Hook                    |
//! |-------------|---------------------------|-------------------------|
//! | Purpose     | **influences** a decision | **observes** an event   |
//! | Return type | `bool` / action enum      | `()` or `HookEffect`    |
//! | Lives in    | `src/reactor/policies/`   | `src/reactor/hooks/`    |
//!
//! If you need to add a side-effect observer (logging, webhooks, analytics)
//! see [`super::hooks`] instead.

pub mod barge_in;
pub mod hang_up;
pub mod idle;
pub mod interrupt;

// ── Flat re-exports (match the old `use crate::policies::Foo` paths) ──────
pub use barge_in::{BargeInPolicy, DefaultBargeInPolicy};
pub use hang_up::{DefaultHangUpPolicy, HangUpPolicy};
pub use idle::{can_barge_in, meets_barge_in_word_gate, should_nudge_idle, WHISPER_FALLBACK_SECS};
pub use interrupt::{interrupt_policy, InterruptPolicy};
