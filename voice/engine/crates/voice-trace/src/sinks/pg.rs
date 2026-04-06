use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use sqlx::{PgPool, QueryBuilder};
use tokio::sync::{Notify, Semaphore};
use tracing::warn;
use uuid::Uuid;
use crate::event::{Event, EventCategory};
use crate::Tracer;

/// What to do when the internal event queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropPolicy {
    /// Discard the incoming event (default).
    DropNewest,
    /// Evict the oldest queued event to make room.
    DropOldest,
    /// Back-pressure the subscriber until the writer drains space.
    Block,
    /// Silently discard the incoming event (same behaviour as DropNewest,
    /// distinct name for configuration clarity).
    Ignore,
}

impl DropPolicy {
    pub fn from_str(v: &str) -> Self {
        match v.to_ascii_lowercase().as_str() {
            "drop_oldest" => Self::DropOldest,
            "block"       => Self::Block,
            "ignore"      => Self::Ignore,
            _             => Self::DropNewest, // "drop_newest" + any unrecognised value
        }
    }
}

#[derive(Debug, Clone)]
pub struct DbAdapterConfig {
    pub categories: Vec<String>,
    pub event_types: Vec<String>,
    pub drop_policy: DropPolicy,
    pub queue_size: usize,
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    pub shutdown_flush_timeout_ms: u64,
}

impl DbAdapterConfig {
    pub fn categories_set(&self) -> HashSet<EventCategory> {
        let mut out = HashSet::new();
        for c in &self.categories {
            let parsed = match c.to_ascii_lowercase().as_str() {
                "session"       => Some(EventCategory::Session),
                "transcript"    => Some(EventCategory::Transcript),
                "tool"          => Some(EventCategory::Tool),
                "agent"         => Some(EventCategory::Agent),
                "agent_audio"   => Some(EventCategory::AgentAudio),
                "metrics"       => Some(EventCategory::Metrics),
                "trace"         => Some(EventCategory::Trace),
                "error"         => Some(EventCategory::Error),
                "observability" => Some(EventCategory::Observability),
                "user_audio"    => Some(EventCategory::UserAudio),
                _ => None,
            };
            if let Some(v) = parsed {
                out.insert(v);
            }
        }
        if out.is_empty() {
            out.insert(EventCategory::Session);
            out.insert(EventCategory::Metrics);
            out.insert(EventCategory::Observability);
            out.insert(EventCategory::Tool);
            out.insert(EventCategory::Error);
        }
        out
    }
}

#[derive(Debug, Clone)]
struct CallEventInsert {
    id: Uuid,
    call_id: Uuid,
    session_id: String,
    seq: i64,
    event_type: String,
    event_category: String,
    occurred_at: chrono::DateTime<chrono::Utc>,
    payload_json: serde_json::Value,
}

pub fn spawn_db_adapter(
    tracer: &Tracer,
    pool: PgPool,
    call_id: Uuid,
    session_id: String,
    config: DbAdapterConfig,
) {
    let categories  = config.categories_set();
    let event_types = config.event_types.iter().map(|s| s.to_string()).collect::<HashSet<_>>();

    let queue_cap        = config.queue_size.max(1);
    let flush_every      = Duration::from_millis(config.flush_interval_ms.max(50));
    let shutdown_timeout = Duration::from_millis(config.shutdown_flush_timeout_ms.max(50));
    let batch_target     = config.batch_size.max(1);
    let drop_policy      = config.drop_policy;

    // Shared bounded queue between the subscriber and writer tasks.
    let queue: Arc<Mutex<VecDeque<CallEventInsert>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(queue_cap)));

    // Writer wakeup: subscriber notifies after each push.
    let data_ready: Arc<Notify> = Arc::new(Notify::new());

    // Session-end signal: subscriber notifies when SessionEnded is processed.
    let session_over: Arc<Notify> = Arc::new(Notify::new());

    // Back-pressure for Block mode only: starts with queue_cap permits; writer
    // replenishes permits as it drains items so the subscriber can make progress.
    let block_sem: Option<Arc<Semaphore>> = if drop_policy == DropPolicy::Block {
        Some(Arc::new(Semaphore::new(queue_cap)))
    } else {
        None
    };

    // ── Writer task ──────────────────────────────────────────────────────────
    {
        let queue        = queue.clone();
        let data_ready   = data_ready.clone();
        let session_over = session_over.clone();
        let block_sem    = block_sem.clone();

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(flush_every);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut batch: Vec<CallEventInsert> = Vec::with_capacity(batch_target);

            loop {
                // Wait for data, a periodic tick, or session end.
                let done = tokio::select! {
                    biased;
                    _ = session_over.notified() => true,
                    _ = data_ready.notified()   => false,
                    _ = ticker.tick()           => false,
                };

                // Drain up to batch_target items and replenish Block semaphore.
                let drained = {
                    let mut q = queue.lock().unwrap();
                    let take = batch_target.saturating_sub(batch.len()).min(q.len());
                    batch.extend(q.drain(..take));
                    take
                };
                if let Some(sem) = &block_sem {
                    if drained > 0 {
                        sem.add_permits(drained);
                    }
                }

                if batch.len() >= batch_target || done {
                    flush_batch(&pool, &mut batch).await;
                }

                // If queue is not empty, wake up the next select! immediately
                // to avoid waiting for the next periodic tick.
                if !done && !queue.lock().unwrap().is_empty() {
                    data_ready.notify_one();
                }

                if done {
                    // Shutdown: drain whatever arrived between the select firing and now.
                    let _ = tokio::time::timeout(shutdown_timeout, async {
                        let remaining: Vec<CallEventInsert> = {
                            let mut q = queue.lock().unwrap();
                            q.drain(..).collect()
                        };
                        if let Some(sem) = &block_sem {
                            sem.add_permits(remaining.len());
                        }
                        batch.extend(remaining);
                        flush_batch(&pool, &mut batch).await;
                    })
                    .await;
                    break;
                }
            }
        });
    }

    // ── Subscriber task ──────────────────────────────────────────────────────
    let mut bus_rx = tracer.subscribe_filtered(categories);
    tokio::spawn(async move {
        let mut seq: i64 = 0;

        while let Some(raw) = bus_rx.recv().await {
            let is_end = matches!(raw, Event::SessionEnded);

            let Some(db_ev) = crate::to_sink_event(&raw) else {
                if is_end { session_over.notify_one(); break; }
                continue;
            };

            let event_type = crate::sink_event::event_type_name(&db_ev).to_string();
            if !event_types.is_empty() && !event_types.contains(&event_type) {
                if is_end { session_over.notify_one(); break; }
                continue;
            }

            seq += 1;
            let payload_json =
                serde_json::to_value(&db_ev).unwrap_or_else(|_| serde_json::json!({}));
            let event = CallEventInsert {
                id: Uuid::new_v4(),
                call_id,
                session_id: session_id.clone(),
                seq,
                event_type,
                event_category: crate::sink_event::event_category_name(&db_ev).to_string(),
                occurred_at: Utc::now(),
                payload_json,
            };

            match drop_policy {
                DropPolicy::DropNewest | DropPolicy::Ignore => {
                    let mut q = queue.lock().unwrap();
                    if q.len() < queue_cap {
                        q.push_back(event);
                    } else {
                        warn!("[observability] queue full — dropping newest event");
                    }
                }
                DropPolicy::DropOldest => {
                    let mut q = queue.lock().unwrap();
                    if q.len() >= queue_cap {
                        q.pop_front();
                        warn!("[observability] queue full — dropping oldest event");
                    }
                    q.push_back(event);
                }
                DropPolicy::Block => {
                    // Acquire a permit: blocks until the writer drains an item and
                    // calls add_permits(). Permit lifecycle is managed manually.
                    if let Some(sem) = &block_sem {
                        let permit = sem.acquire().await.expect("block semaphore closed");
                        std::mem::forget(permit); // replenished by writer via add_permits
                    }
                    queue.lock().unwrap().push_back(event);
                }
            }

            data_ready.notify_one();
            if is_end { session_over.notify_one(); break; }
        }
    });
}

async fn flush_batch(pool: &PgPool, batch: &mut Vec<CallEventInsert>) {
    if batch.is_empty() {
        return;
    }
    let flushing = std::mem::take(batch);
    if let Err(e) = insert_call_events_batch(pool, &flushing).await {
        warn!(
            "[observability] failed to write {} call events: {}",
            flushing.len(),
            e
        );
    }
}

async fn insert_call_events_batch(
    pool: &PgPool,
    events: &[CallEventInsert],
) -> Result<(), sqlx::Error> {
    if events.is_empty() {
        return Ok(());
    }
    let mut qb = QueryBuilder::new(
        "INSERT INTO call_events \
        (id, call_id, session_id, seq, event_type, event_category, occurred_at, payload_json) ",
    );
    qb.push_values(events, |mut b, event| {
        b.push_bind(event.id)
            .push_bind(event.call_id)
            .push_bind(&event.session_id)
            .push_bind(event.seq)
            .push_bind(&event.event_type)
            .push_bind(&event.event_category)
            .push_bind(event.occurred_at)
            .push_bind(&event.payload_json);
    });
    qb.build().execute(pool).await?;
    Ok(())
}
