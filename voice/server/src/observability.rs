//! Observability adapter assembly for per-call event collection.
//!
//! Adapters are implemented in `voice-trace-*` crates and activated here based
//! on runtime settings.

use sqlx::PgPool;
use tracing::{debug, info, warn};
use uuid::Uuid;
use voice_trace::Tracer;

use crate::db::ObservabilitySettings;

#[derive(Debug, Clone)]
pub struct ExternalLink {
    pub adapter: String,
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct ObservabilityRunInfo {
    pub active_adapters: Vec<String>,
    pub external_links: Vec<ExternalLink>,
}

struct AdapterCallContext<'a> {
    tracer: &'a Tracer,
    pool: &'a PgPool,
    call_id: Uuid,
    session_id: &'a str,
    settings: &'a ObservabilitySettings,
}

trait ObservabilityAdapter {
    fn name(&self) -> &'static str;
    fn init(&self, settings: &ObservabilitySettings);
    fn attach_call(&self, ctx: &AdapterCallContext<'_>, run: &mut ObservabilityRunInfo);
}

struct DbAdapter;

impl ObservabilityAdapter for DbAdapter {
    fn name(&self) -> &'static str {
        "db"
    }

    fn init(&self, settings: &ObservabilitySettings) {
        info!(
            enabled = settings.db_events_enabled,
            "observability db adapter initialized"
        );
    }

    fn attach_call(&self, ctx: &AdapterCallContext<'_>, run: &mut ObservabilityRunInfo) {
        if !ctx.settings.db_events_enabled {
            return;
        }

        voice_trace::sinks::pg::spawn_db_adapter(
            ctx.tracer,
            ctx.pool.clone(),
            ctx.call_id,
            ctx.session_id.to_string(),
            voice_trace::sinks::pg::DbAdapterConfig {
                categories: ctx.settings.db_categories.clone(),
                event_types: ctx.settings.db_event_types.clone(),
                drop_policy: voice_trace::sinks::pg::DropPolicy::from_str(
                    &ctx.settings.drop_policy,
                ),
                queue_size: ctx.settings.queue_size,
                batch_size: ctx.settings.batch_size,
                flush_interval_ms: ctx.settings.flush_interval_ms,
                shutdown_flush_timeout_ms: ctx.settings.shutdown_flush_timeout_ms,
            },
        );
        run.active_adapters.push(self.name().to_string());
    }
}

struct LangfuseAdapter;

impl ObservabilityAdapter for LangfuseAdapter {
    fn name(&self) -> &'static str {
        "langfuse"
    }

    fn init(&self, settings: &ObservabilitySettings) {
        if !settings.langfuse_enabled {
            info!("observability langfuse adapter disabled");
            return;
        }

        if settings.langfuse_public_key.is_empty() || settings.langfuse_secret_key.is_empty() {
            warn!("langfuse enabled but key/secret is missing; adapter will stay inactive");
            return;
        }

        voice_trace::sinks::langfuse::init(&voice_trace::sinks::langfuse::LangfuseConfig {
            public_key: settings.langfuse_public_key.clone(),
            secret_key: settings.langfuse_secret_key.clone(),
            base_url: settings.langfuse_base_url.clone(),
            trace_public: settings.langfuse_trace_public,
        });
        info!(
            langfuse_base_url = settings.langfuse_base_url.as_str(),
            "observability langfuse adapter initialized"
        );
    }

    fn attach_call(&self, ctx: &AdapterCallContext<'_>, run: &mut ObservabilityRunInfo) {
        if !ctx.settings.langfuse_enabled {
            return;
        }
        if ctx.settings.langfuse_public_key.is_empty()
            || ctx.settings.langfuse_secret_key.is_empty()
        {
            debug!("langfuse enabled but keys are empty, skipping adapter");
            return;
        }

        voice_trace::sinks::langfuse::spawn_langfuse_subscriber(
            ctx.tracer,
            ctx.session_id.to_string(),
        );
        run.active_adapters.push(self.name().to_string());
        run.external_links.push(ExternalLink {
            adapter: self.name().to_string(),
            label: "Open in Langfuse".to_string(),
            url: ctx.settings.langfuse_base_url.clone(),
        });
    }
}

fn adapters() -> [Box<dyn ObservabilityAdapter>; 2] {
    [Box::new(DbAdapter), Box::new(LangfuseAdapter)]
}

pub fn init_global_adapters(settings: &ObservabilitySettings) {
    for adapter in adapters() {
        adapter.init(settings);
    }
}

pub fn spawn_adapter_manager(
    tracer: &Tracer,
    pool: PgPool,
    call_id: Uuid,
    session_id: String,
    settings: ObservabilitySettings,
) -> ObservabilityRunInfo {
    let ctx = AdapterCallContext {
        tracer,
        pool: &pool,
        call_id,
        session_id: &session_id,
        settings: &settings,
    };
    let mut run = ObservabilityRunInfo {
        active_adapters: Vec::new(),
        external_links: Vec::new(),
    };

    for adapter in adapters() {
        adapter.attach_call(&ctx, &mut run);
    }

    run
}
