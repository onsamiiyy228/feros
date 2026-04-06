//! Unified event bus — single broadcast channel for all voice engine events.
//!
//! The Reactor emits every event through the bus. Consumers subscribe with
//! optional category-based filtering — see [`EventCategory`](crate::EventCategory)
//! docs for the full subscriber-to-category mapping.
//!
//! The bus is lossy — slow consumers miss events rather than blocking the
//! reactor's hot loop.

use std::collections::HashSet;

use tokio::sync::broadcast;
use tracing::warn;

use crate::event::{Event, EventCategory};

/// Bounded capacity for the broadcast channel.
///
/// Must be large enough to absorb bursts of high-frequency audio events
/// without the subscriber lagging. At 48 kHz with 512-sample chunks,
/// 2048 slots ≈ ~22 seconds of audio headroom plus control events.
const BUS_CAPACITY: usize = 2048;

/// Unified event bus backed by a `tokio::sync::broadcast` channel.
///
/// Create one per session. The Reactor pushes events via `emit()`.
/// External consumers call `subscribe()` or `subscribe_filtered()`.
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Emit an event to all subscribers. Never blocks.
    ///
    /// If no subscribers are attached, the event is silently dropped.
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    /// Subscribe to ALL events (unfiltered).
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Get the underlying broadcast sender.
    ///
    /// External code can clone this sender and call `.subscribe()` on it
    /// to create additional receivers without holding a reference to the bus.
    pub fn sender(&self) -> broadcast::Sender<Event> {
        self.tx.clone()
    }

    /// Subscribe with a category filter.
    ///
    /// The returned [`FilteredReceiver`] only yields events whose
    /// category is in the provided set. Non-matching events are
    /// consumed and discarded internally.
    pub fn subscribe_filtered(&self, categories: HashSet<EventCategory>) -> FilteredReceiver {
        FilteredReceiver {
            rx: self.tx.subscribe(),
            categories,
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// A filtered view of the event bus.
///
/// Wraps a `broadcast::Receiver<Event>` and only surfaces events
/// whose category is in the filter set. Non-matching events are
/// silently consumed.
pub struct FilteredReceiver {
    rx: broadcast::Receiver<Event>,
    categories: HashSet<EventCategory>,
}

impl FilteredReceiver {
    /// Create a filtered receiver from an existing broadcast receiver.
    ///
    /// Use this when you have a `broadcast::Sender` (e.g. stored in
    /// server state) and need to subscribe with category filtering.
    ///
    /// ```ignore
    /// let rx = sender.subscribe();
    /// let filtered = FilteredReceiver::new(rx, HashSet::from([EventCategory::Session]));
    /// ```
    pub fn new(rx: broadcast::Receiver<Event>, categories: HashSet<EventCategory>) -> Self {
        Self { rx, categories }
    }

    /// Receive the next event matching the filter.
    ///
    /// Blocks until a matching event arrives or the bus is closed.
    /// Lagged events are silently skipped (with a warning log).
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            match self.rx.recv().await {
                Ok(event) if self.categories.contains(&event.category()) => {
                    return Some(event);
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Event subscriber lagged by {} events", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}
