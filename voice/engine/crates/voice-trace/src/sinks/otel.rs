//! OpenTelemetry subscriber — exports engine events over standard OTLP protocol.
//!
//! Feature-gated behind `otel`. When enabled, call [`spawn_otel_subscriber`]
//! to start a background task that consumes events from the bus and exports
//! them using gRPC to standard APM backends like Datadog, Jaeger, Grafana Tempo,
//! or New Relic.
//!
//! # OTel vs. Langfuse
//! 
//! Conceptually, `otel.rs` and `langfuse.rs` run in parallel but serve entirely 
//! different personas:
//!
//! - **Langfuse Sink (`sinks/langfuse.rs`)**: For AI / Product Engineers.
//!   Converses via the Langfuse HTTP API. Emits deep, hierarchical AI traces
//!   (`Turn -> STT -> LLM -> TTS`). Captures prompts, LLM generations, tool results,
//!   and cost-model metrics designed for debugging bot intelligence.
//! - **OTel Sink (this module)**: For Infrastructure / SRE Engineers.
//!   Converses via the OpenTelemetry OTLP standard. Emits flat, stateless 
//!   metrics spanning the pipeline (e.g. `voice.turn` latency, `voice.error`).
//!   Designed for general system health, server uptime, and microservice monitoring.
//!
//! **Note:** Detailed LLM prompt tracing is strictly routed to Langfuse 
//! and is deliberately omitted from OTel to avoid noise and PII leaks in standard APMs.

#[cfg(feature = "otel")]
use std::collections::HashSet;

#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otel")]
use opentelemetry::KeyValue;
#[cfg(feature = "otel")]
use opentelemetry_sdk::trace::SdkTracerProvider;
#[cfg(feature = "otel")]
use tracing::info;

#[cfg(feature = "otel")]
use crate::event::{Event, EventCategory};
#[cfg(feature = "otel")]
use crate::tracer::Tracer;

/// Spawn a background task that subscribes to the event bus and exports
/// matching events to an OTLP backend.
///
/// # Exported signals
///
/// | Event | OTel Signal |
/// |---|---|
/// | `Trace { .. }` | Span with `label`, `seq`, `elapsed_us` attributes |
/// | `TurnMetrics(..)` | Span with latency attributes (TTFA, STT, LLM, TTS, total) |
/// | `ToolActivity { .. }` | Span with `tool_name` and `status` attributes |
///
/// # Configuration
///
/// Configured via standard OTel environment variables:
/// - `OTEL_EXPORTER_OTLP_ENDPOINT` (default: `http://localhost:4317`)
/// - `OTEL_SERVICE_NAME` (default: `voice-engine`)
#[cfg(feature = "otel")]
pub fn spawn_otel_subscriber(tracer: &Tracer) {
    let provider = build_provider();
    let otel_tracer = provider.tracer("voice-engine");

    let mut rx = tracer.subscribe_filtered(HashSet::from([
        EventCategory::Session,
        EventCategory::Trace,
        EventCategory::Metrics,
        EventCategory::Tool,
        EventCategory::Transcript,
        EventCategory::Agent,
        EventCategory::Error,
    ]));

    tokio::spawn(async move {
        info!("[otel] Voice engine subscriber started (OTLP export)");

        while let Some(event) = rx.recv().await {
            use opentelemetry::trace::Tracer as OtelTracer;

            match event {
                Event::Trace {
                    seq,
                    elapsed_us,
                    label,
                } => {
                    otel_tracer.in_span(format!("voice.{}", label), |_cx| {
                        opentelemetry::trace::get_active_span(|span| {
                            span.set_attribute(KeyValue::new("voice.seq", seq as i64));
                            span.set_attribute(KeyValue::new(
                                "voice.elapsed_us",
                                elapsed_us as i64,
                            ));
                        });
                    });
                }

                Event::TurnMetrics(m) => {
                    otel_tracer.in_span("voice.turn", |_cx| {
                        opentelemetry::trace::get_active_span(|span| {
                            span.set_attribute(KeyValue::new("voice.turn.id", m.turn_id as i64));
                            span.set_attribute(KeyValue::new("voice.turn.ttfa_ms", m.ttfa_ms));
                            span.set_attribute(KeyValue::new("voice.turn.stt_ms", m.stt_ms));
                            span.set_attribute(KeyValue::new(
                                "voice.turn.llm_ttfb_ms",
                                m.llm_first_token_ms,
                            ));
                            // Pipeline metric: speech_ended → first TTS audio (includes LLM + text aggregation overhead).
                            // Distinct from per-service tts_ttfb_ms below which measures text_fed → first_audio.
                            span.set_attribute(KeyValue::new("voice.turn.tts_first_audio_ms", m.tts_first_audio_ms));
                            span.set_attribute(KeyValue::new("voice.turn.total_ms", m.total_ms));
                            span.set_attribute(KeyValue::new("voice.turn.vad_silence_ms", m.vad_silence_ms));

                            // Per-service metrics (single source of truth)
                            span.set_attribute(KeyValue::new("voice.turn.stt_total_ms", m.stt_total_duration_ms));
                            if let Some(stt_ttfb) = m.stt_ttfb_ms {
                                span.set_attribute(KeyValue::new("voice.turn.stt_ttfb_ms", stt_ttfb));
                            }
                            if let Some(tts_ttfb) = m.tts_ttfb_ms {
                                span.set_attribute(KeyValue::new("voice.turn.tts_ttfb_ms", tts_ttfb));
                            }
                            span.set_attribute(KeyValue::new("voice.turn.tts_total_ms", m.tts_total_duration_ms));
                            if let Some(agg) = m.text_aggregation_ms {
                                span.set_attribute(KeyValue::new("voice.turn.text_aggregation_ms", agg));
                            }

                            if let Some(ual) = m.user_agent_latency_ms {
                                span.set_attribute(KeyValue::new("voice.turn.user_agent_latency_ms", ual));
                            }
                        });
                    });
                }

                Event::ToolActivity {
                    tool_call_id,
                    tool_name,
                    status,
                    error_message,
                } => {
                    otel_tracer.in_span(format!("voice.tool.{}", tool_name), |_cx| {
                        opentelemetry::trace::get_active_span(|span| {
                            span.set_attribute(KeyValue::new("voice.tool.name", tool_name.clone()));
                            span.set_attribute(KeyValue::new("voice.tool.status", status.clone()));
                            if let Some(tool_call_id) = tool_call_id.clone() {
                                span.set_attribute(KeyValue::new(
                                    "voice.tool.call_id",
                                    tool_call_id,
                                ));
                            }
                            if let Some(error_message) = error_message.clone() {
                                span.set_attribute(KeyValue::new(
                                    "voice.tool.error_message",
                                    error_message,
                                ));
                            }
                        });
                    });
                }

                Event::Transcript { role, text } => {
                    let span_name = format!("voice.transcript.{}", role);
                    otel_tracer.in_span(span_name, |_cx| {
                        opentelemetry::trace::get_active_span(|span| {
                            span.set_attribute(KeyValue::new(
                                "voice.transcript.role",
                                role.clone(),
                            ));
                            span.set_attribute(KeyValue::new(
                                "voice.transcript.text",
                                text.clone(),
                            ));
                        });
                    });
                }

                Event::AgentEvent { kind } => {
                    otel_tracer.in_span(format!("voice.agent.{}", kind), |_cx| {
                        opentelemetry::trace::get_active_span(|span| {
                            span.set_attribute(KeyValue::new("voice.agent.kind", kind.clone()));
                        });
                    });
                }

                Event::Error { source, message } => {
                    otel_tracer.in_span(format!("voice.error.{}", source), |_cx| {
                        opentelemetry::trace::get_active_span(|span| {
                            span.set_attribute(KeyValue::new("voice.error.source", source.clone()));
                            span.set_attribute(KeyValue::new(
                                "voice.error.message",
                                message.clone(),
                            ));
                        });
                    });
                }

                Event::SessionEnded => {
                    otel_tracer.in_span("voice.session.ended", |_cx| {});
                }

                _ => {}
            }
        }

        info!("[otel] Voice engine subscriber stopped");

        // Flush remaining spans
        if let Err(e) = provider.shutdown() {
            tracing::warn!("[otel] Shutdown error: {:?}", e);
        }
    });
}

/// Build the OTel tracer provider using the standard OTLP gRPC exporter.
#[cfg(feature = "otel")]
fn build_provider() -> SdkTracerProvider {
    use opentelemetry_otlp::SpanExporter;
    use opentelemetry_sdk::trace::BatchSpanProcessor;

    let exporter = SpanExporter::builder()
        .with_tonic()
        .build()
        .expect("Failed to build OTLP exporter");

    let processor = BatchSpanProcessor::builder(exporter).build();

    SdkTracerProvider::builder()
        .with_span_processor(processor)
        .build()
}
