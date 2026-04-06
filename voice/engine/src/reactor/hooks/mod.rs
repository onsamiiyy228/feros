//! Reactor lifecycle and behavior hooks.
//!
//! Implement [`ReactorHook`] to observe or augment reactor behavior
//! without modifying the reactor core.

pub mod hook;
pub use hook::ReactorHook;
