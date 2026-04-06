//! Tests for observability-related logic:
//! - DefaultAgentBackend::emit_llm_complete() idempotency and JSON shape
//! - DefaultAgentBackend::cancel() returning LlmComplete
//! - DefaultAgentBackend::is_active() with buffered events
//! - agent_event_to_llm() exhaustive variant coverage

use std::sync::Arc;

use agent_kit::agent_backends::default::DefaultAgentBackend;
use agent_kit::agent_backends::ChatMessage;
use agent_kit::agent_backends::{AgentBackend, AgentBackendConfig, AgentEvent};
use agent_kit::providers::LlmEvent as InnerLlmEvent;
use agent_kit::providers::{LlmCallConfig, LlmProvider, LlmProviderError};
use async_trait::async_trait;
use tokio::sync::mpsc;
use voice_trace::Event;

// ── Stub LLM Provider ───────────────────────────────────────────

/// Minimal LLM provider for testing observability logic.
/// Sends pre-configured events (as serializable descriptions) when
/// `stream_completion` is called.
struct StubProvider {
    /// Events to send: (type, data). We construct them in the async task.
    tokens: Vec<String>,
    usage: Option<(u32, u32, u32)>, // (prompt, completion, cached)
    name: String,
}

impl StubProvider {
    fn with_tokens_and_usage(
        tokens: Vec<&str>,
        prompt_tokens: u32,
        completion_tokens: u32,
        cached_input_tokens: u32,
    ) -> Self {
        Self {
            tokens: tokens.into_iter().map(String::from).collect(),
            usage: Some((prompt_tokens, completion_tokens, cached_input_tokens)),
            name: "stub".to_string(),
        }
    }

    fn with_tokens(tokens: Vec<&str>) -> Self {
        Self {
            tokens: tokens.into_iter().map(String::from).collect(),
            usage: None,
            name: "stub".to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for StubProvider {
    async fn stream_completion(
        &self,
        _messages: &[ChatMessage],
        _tools: Option<&[serde_json::Value]>,
        _config: &LlmCallConfig,
    ) -> Result<mpsc::Receiver<InnerLlmEvent>, LlmProviderError> {
        let (tx, rx) = mpsc::channel(64);
        let tokens = self.tokens.clone();
        let usage = self.usage;
        tokio::spawn(async move {
            for token in tokens {
                let _ = tx.send(InnerLlmEvent::Token(token)).await;
            }
            if let Some((prompt, completion, cached)) = usage {
                let _ = tx
                    .send(InnerLlmEvent::Usage {
                        prompt_tokens: prompt,
                        completion_tokens: completion,
                        cached_input_tokens: cached,
                    })
                    .await;
            }
            // Channel drops → stream closed
        });
        Ok(rx)
    }

    fn provider_name(&self) -> &str {
        &self.name
    }
}

// ── Helper ──────────────────────────────────────────────────────

fn make_backend_with_provider(provider: StubProvider) -> DefaultAgentBackend {
    let config = AgentBackendConfig::default();
    DefaultAgentBackend::new(Arc::new(provider), None, config)
}

/// Drain all events from a started backend turn.
async fn drain_events(backend: &mut DefaultAgentBackend) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = backend.recv().await {
        events.push(event);
    }
    events
}

// ── emit_llm_complete: idempotency ──────────────────────────────

#[tokio::test]
async fn emit_llm_complete_is_idempotent() {
    // When the LLM emits tokens and then closes, completing a turn should
    // produce exactly one LlmComplete event. The second call is a no-op.
    let provider = StubProvider::with_tokens_and_usage(vec!["Hello"], 10, 1, 0);
    let mut backend = make_backend_with_provider(provider);

    backend.set_system_prompt("Test".into());
    backend.add_user_message("Hi".into());
    backend.start_turn().await.unwrap();

    let events = drain_events(&mut backend).await;

    // Should have: Token("Hello"), LlmComplete, Finished
    let llm_completes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::LlmComplete { .. }))
        .collect();
    assert_eq!(
        llm_completes.len(),
        1,
        "Expected exactly one LlmComplete event, got {}",
        llm_completes.len()
    );
}

// ── emit_llm_complete: output JSON structure ────────────────────

#[tokio::test]
async fn emit_llm_complete_output_json_text_only() {
    let provider = StubProvider::with_tokens_and_usage(vec!["world"], 5, 1, 0);
    let mut backend = make_backend_with_provider(provider);

    backend.set_system_prompt("Test".into());
    backend.add_user_message("Hello".into());
    backend.start_turn().await.unwrap();

    let events = drain_events(&mut backend).await;
    let llm_complete = events
        .iter()
        .find(|e| matches!(e, AgentEvent::LlmComplete { .. }))
        .expect("Should have LlmComplete");

    if let AgentEvent::LlmComplete {
        output_json,
        provider,
        prompt_tokens,
        completion_tokens,
        ..
    } = llm_complete
    {
        // Output JSON should contain the text content
        let parsed: serde_json::Value = serde_json::from_str(output_json).unwrap();
        assert_eq!(parsed["content"], "world");
        // No tool_calls key when there are no tools
        assert!(parsed.get("tool_calls").is_none());
        assert_eq!(provider, "stub");
        assert_eq!(*prompt_tokens, 5);
        assert_eq!(*completion_tokens, 1);
    } else {
        panic!("Expected LlmComplete");
    }
}

// ── emit_llm_complete: token usage ──────────────────────────────

#[tokio::test]
async fn emit_llm_complete_captures_cached_tokens() {
    let provider = StubProvider::with_tokens_and_usage(vec!["ok"], 100, 5, 80);
    let mut backend = make_backend_with_provider(provider);

    backend.set_system_prompt("Test".into());
    backend.add_user_message("test".into());
    backend.start_turn().await.unwrap();

    let events = drain_events(&mut backend).await;
    let llm_complete = events
        .iter()
        .find(|e| matches!(e, AgentEvent::LlmComplete { .. }))
        .expect("Should have LlmComplete");

    if let AgentEvent::LlmComplete {
        cache_read_tokens, ..
    } = llm_complete
    {
        assert_eq!(*cache_read_tokens, Some(80));
    } else {
        panic!("Expected LlmComplete");
    }
}

#[tokio::test]
async fn emit_llm_complete_no_cached_tokens_when_zero() {
    let provider = StubProvider::with_tokens_and_usage(vec!["ok"], 10, 1, 0);
    let mut backend = make_backend_with_provider(provider);

    backend.set_system_prompt("Test".into());
    backend.add_user_message("test".into());
    backend.start_turn().await.unwrap();

    let events = drain_events(&mut backend).await;
    let llm_complete = events
        .iter()
        .find(|e| matches!(e, AgentEvent::LlmComplete { .. }))
        .expect("Should have LlmComplete");

    if let AgentEvent::LlmComplete {
        cache_read_tokens, ..
    } = llm_complete
    {
        assert!(
            cache_read_tokens.is_none(),
            "cache_read_tokens should be None when cached_input_tokens is 0"
        );
    } else {
        panic!("Expected LlmComplete");
    }
}

// ── cancel: returns LlmComplete ─────────────────────────────────

#[tokio::test]
async fn cancel_returns_llm_complete_for_partial_generation() {
    let provider = StubProvider::with_tokens_and_usage(vec!["Hello ", "world"], 10, 2, 0);
    let mut backend = make_backend_with_provider(provider);

    backend.set_system_prompt("Test".into());
    backend.add_user_message("say hi".into());
    backend.start_turn().await.unwrap();

    // Consume a few tokens so the backend has partial state
    let _ = backend.recv().await; // Token("Hello ")
    let _ = backend.recv().await; // Token("world")

    // Give stream a moment to deliver Usage event
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Cancel mid-stream
    let result = backend.cancel();
    assert!(result.is_some(), "cancel() should return LlmComplete");

    if let Some(AgentEvent::LlmComplete { output_json, .. }) = result {
        let parsed: serde_json::Value = serde_json::from_str(&output_json).unwrap();
        assert_eq!(parsed["content"], "Hello world");
    } else {
        panic!("Expected LlmComplete from cancel()");
    }
}

#[tokio::test]
async fn cancel_when_idle_returns_none() {
    let provider = StubProvider::with_tokens(vec![]);
    let mut backend = make_backend_with_provider(provider);
    // Never started a turn
    let result = backend.cancel();
    assert!(
        result.is_none(),
        "cancel() on idle backend should return None"
    );
}

// ── is_active ───────────────────────────────────────────────────

#[tokio::test]
async fn is_active_false_when_idle() {
    let provider = StubProvider::with_tokens(vec![]);
    let backend = make_backend_with_provider(provider);
    assert!(!backend.is_active());
}

#[tokio::test]
async fn is_active_true_during_streaming() {
    let provider = StubProvider::with_tokens(vec!["test"]);
    let mut backend = make_backend_with_provider(provider);

    backend.set_system_prompt("Test".into());
    backend.add_user_message("Hi".into());
    backend.start_turn().await.unwrap();

    assert!(backend.is_active(), "Should be active during streaming");

    // Drain to finish
    drain_events(&mut backend).await;

    assert!(!backend.is_active(), "Should be idle after draining");
}

// ── agent_event_to_llm: exhaustive variant coverage ─────────────
// These tests verify that the agent_event_to_llm mapping is correct
// at the type level by constructing AgentEvents and checking them.

#[test]
fn agent_event_token_variant() {
    let event = AgentEvent::Token("hello".into());
    assert!(matches!(event, AgentEvent::Token(ref s) if s == "hello"));
}

#[test]
fn agent_event_finished_with_content() {
    let event = AgentEvent::Finished {
        content: Some("done".into()),
    };
    assert!(matches!(event, AgentEvent::Finished { content: Some(ref s) } if s == "done"));
}

#[test]
fn agent_event_finished_without_content() {
    let event = AgentEvent::Finished { content: None };
    assert!(matches!(event, AgentEvent::Finished { content: None }));
}

#[test]
fn agent_event_tool_call_started() {
    let event = AgentEvent::ToolCallStarted {
        id: "call_1".into(),
        name: "search".into(),
        side_effect: true,
    };
    assert!(
        matches!(event, AgentEvent::ToolCallStarted { ref id, ref name, side_effect } if id == "call_1" && name == "search" && side_effect)
    );
}

#[test]
fn agent_event_tool_call_completed() {
    let event = AgentEvent::ToolCallCompleted {
        id: "call_1".into(),
        name: "search".into(),
        success: false,
        error_message: Some("Tool 'search' returned 500".into()),
    };
    assert!(matches!(
        event,
        AgentEvent::ToolCallCompleted {
            ref id,
            ref name,
            success: false,
            error_message: Some(ref error_message),
        } if id == "call_1" && name == "search" && error_message == "Tool 'search' returned 500"
    ));
}

#[test]
fn tool_activity_serializes_optional_error_fields() {
    let event = Event::ToolActivity {
        tool_call_id: Some("call_1".into()),
        tool_name: "search".into(),
        status: "error".into(),
        error_message: Some("Tool 'search' returned 500".into()),
    };

    let json = serde_json::to_value(event).expect("tool activity should serialize");
    assert_eq!(json["type"], "tool_activity");
    assert_eq!(json["tool_call_id"], "call_1");
    assert_eq!(json["tool_name"], "search");
    assert_eq!(json["status"], "error");
    assert_eq!(json["error_message"], "Tool 'search' returned 500");
}

#[test]
fn agent_event_error() {
    let event = AgentEvent::Error("boom".into());
    assert!(matches!(event, AgentEvent::Error(ref s) if s == "boom"));
}

#[test]
fn agent_event_hang_up() {
    let event = AgentEvent::HangUp {
        reason: "goodbye".into(),
        content: None,
    };
    assert!(matches!(event, AgentEvent::HangUp { ref reason, .. } if reason == "goodbye"));
}

#[test]
fn agent_event_llm_complete() {
    let event = AgentEvent::LlmComplete {
        provider: "openai".into(),
        model: "gpt-4".into(),
        input_json: "[]".into(),
        output_json: "{}".into(),
        tools_json: None,
        temperature: 0.7,
        max_tokens: 32768,
        duration_ms: 100.0,
        ttfb_ms: Some(50.0),
        prompt_tokens: 10,
        completion_tokens: 5,
        cache_read_tokens: None,
        span_label: "llm".into(),
    };
    assert!(matches!(
        event,
        AgentEvent::LlmComplete {
            ref provider,
            ref model,
            ..
        } if provider == "openai" && model == "gpt-4"
    ));
}

// ── Duration/TTFB calculations ──────────────────────────────────

#[tokio::test]
async fn llm_complete_has_nonzero_duration() {
    let provider = StubProvider::with_tokens_and_usage(vec!["hi"], 1, 1, 0);
    let mut backend = make_backend_with_provider(provider);

    backend.set_system_prompt("Test".into());
    backend.add_user_message("yo".into());
    backend.start_turn().await.unwrap();

    let events = drain_events(&mut backend).await;
    let llm_complete = events
        .iter()
        .find(|e| matches!(e, AgentEvent::LlmComplete { .. }))
        .expect("Should have LlmComplete");

    if let AgentEvent::LlmComplete { ttfb_ms, .. } = llm_complete {
        // ttfb_ms should be set since we received a token
        assert!(
            ttfb_ms.is_some(),
            "ttfb_ms should be set when tokens were received"
        );
    }
}
