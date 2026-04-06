//! Shared streaming logic for rig-backed providers.
//!
//! All rig providers share the same message conversion, tool schema
//! conversion, and stream-processing code. This module extracts that
//! shared logic so each per-vendor provider is a thin wrapper.

use futures_util::StreamExt;
use rig::completion::message as rig_msg;
use rig::completion::message::ToolFunction;
use rig::completion::request::{CompletionModel, GetTokenUsage, ToolDefinition};
use rig::one_or_many::OneOrMany;
use tokio::sync::mpsc;
use tracing::warn;

use crate::providers::{LlmCallConfig, LlmProviderError};
use crate::agent_backends::ChatMessage;
use crate::providers::{LlmEvent, ToolCallEvent};

// ── Stream Execution ────────────────────────────────────────────

/// Execute a streaming completion against any rig `CompletionModel`.
///
/// This is the core loop shared by all vendor providers. It:
/// 1. Converts our `ChatMessage` format → rig messages
/// 2. Builds the rig `CompletionRequest`
/// 3. Spawns a background task that reads the rig stream and forwards
///    events as `LlmEvent` through the returned channel.
pub async fn stream_rig_completion<M>(
    model: &M,
    messages: &[ChatMessage],
    tools: Option<&[serde_json::Value]>,
    config: &LlmCallConfig,
) -> Result<mpsc::Receiver<LlmEvent>, LlmProviderError>
where
    M: CompletionModel,
    M::StreamingResponse: 'static,
{
    // Split messages: extract system prompt (preamble) and conversation
    let (preamble, rig_messages) = convert_messages(messages);

    // rig expects the last message as the "prompt" and prior ones as
    // chat_history. We use the last user message as the prompt.
    let (prompt, chat_history) = if let Some((last, rest)) = rig_messages.split_last() {
        (last.clone(), rest.to_vec())
    } else {
        (rig_msg::Message::user(""), vec![])
    };

    // Build the completion request
    let mut builder = model
        .completion_request(prompt)
        .messages(chat_history)
        .temperature(config.temperature);

    if let Some(preamble_text) = preamble {
        builder = builder.preamble(preamble_text);
    }

    if let Some(tool_schemas) = tools {
        let tool_defs: Vec<ToolDefinition> = tool_schemas
            .iter()
            .filter_map(convert_tool_schema)
            .collect();
        builder = builder.tools(tool_defs);
    }

    let request = builder.build();
    let mut stream = model
        .stream(request)
        .await
        .map_err(|e| LlmProviderError::Provider(format!("rig stream error: {}", e)))?;

    let (tx, rx) = mpsc::channel::<LlmEvent>(64);

    // Spawn adapter task: poll rig Stream → forward as LlmEvent
    tokio::spawn(async move {
        while let Some(chunk) = stream.next().await {
            let content = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!("[rig] Stream error: {}", e);
                    break;
                }
            };
            match content {
                rig::streaming::StreamedAssistantContent::Text(text) => {
                    if !text.text.is_empty()
                        && tx.send(LlmEvent::Token(text.text.clone())).await.is_err()
                    {
                        return; // Receiver dropped (barge-in)
                    }
                }
                rig::streaming::StreamedAssistantContent::ToolCall {
                    tool_call,
                    internal_call_id: _,
                } => {
                    if tx
                        .send(LlmEvent::ToolCall(ToolCallEvent {
                            name: tool_call.function.name.clone(),
                            arguments: serde_json::to_string(&tool_call.function.arguments)
                                .unwrap_or_default(),
                            tool_call_id: Some(tool_call.id.clone()),
                            signature: tool_call.signature.clone(),
                        }))
                        .await
                        .is_err()
                    {
                        return; // Receiver dropped
                    }
                }
                rig::streaming::StreamedAssistantContent::Final(response) => {
                    if let Some(usage) = response.token_usage() {
                        let _ = tx
                            .send(LlmEvent::Usage {
                                prompt_tokens: usage.input_tokens as u32,
                                completion_tokens: usage.output_tokens as u32,
                                cached_input_tokens: usage.cached_input_tokens as u32,
                            })
                            .await;
                    }
                }
                _ => {} // Reasoning, ToolCallDelta — not relevant
            }
        }
    });

    Ok(rx)
}

// ── Message Conversion ──────────────────────────────────────────

/// Convert our `ChatMessage[]` → rig `Message[]`.
///
/// Returns `(optional_preamble, rig_messages)`. System messages are collapsed
/// into a single preamble string (rig uses `preamble` instead of a System
/// message variant). Tool result messages become `UserContent::ToolResult`.
pub(crate) fn convert_messages(
    messages: &[ChatMessage],
) -> (Option<String>, Vec<rig_msg::Message>) {
    let mut preamble_parts: Vec<String> = Vec::new();
    let mut rig_msgs: Vec<rig_msg::Message> = Vec::new();

    for msg in messages {
        let content_str = match &msg.content {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(v) => v.to_string(), // Arrays gracefully degrade to JSON strings here until natively parsed
            None => String::new(),
        };
        match msg.role.as_str() {
            "system" => {
                if !content_str.is_empty() {
                    preamble_parts.push(content_str.to_string());
                }
            }
            "user" => {
                rig_msgs.push(rig_msg::Message::user(&content_str));
            }
            "assistant" => {
                let mut parts: Vec<rig_msg::AssistantContent> = Vec::new();
                if !content_str.is_empty() {
                    parts.push(rig_msg::AssistantContent::text(&content_str));
                }
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        if let (Some(id), Some(func)) =
                            (tc.get("id").and_then(|v| v.as_str()), tc.get("function"))
                        {
                            let name = func
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args_str = func
                                .get("arguments")
                                .and_then(|a| a.as_str())
                                .unwrap_or("{}");
                            let arguments: serde_json::Value =
                                serde_json::from_str(args_str).unwrap_or_default();
                            let signature = tc
                                .get("signature")
                                .and_then(|v| v.as_str())
                                .map(str::to_string);
                            parts.push(rig_msg::AssistantContent::ToolCall(
                                rig_msg::ToolCall::new(
                                    id.to_string(),
                                    ToolFunction { name, arguments },
                                )
                                .with_signature(signature),
                            ));
                        }
                    }
                }
                if !parts.is_empty() {
                    rig_msgs.push(rig_msg::Message::Assistant {
                        id: None,
                        content: OneOrMany::many(parts).expect("non-empty assistant content"),
                    });
                }
            }
            "tool" => {
                let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("unknown");
                rig_msgs.push(rig_msg::Message::tool_result(tool_call_id, &content_str));
            }
            _ => {
                warn!("[rig] Unknown message role: {}", msg.role);
            }
        }
    }

    let preamble = if preamble_parts.is_empty() {
        None
    } else {
        Some(preamble_parts.join("\n"))
    };

    (preamble, rig_msgs)
}

// ── Tool Schema Conversion ──────────────────────────────────────

/// Convert an OpenAI-format tool JSON schema → rig `ToolDefinition`.
pub(crate) fn convert_tool_schema(schema: &serde_json::Value) -> Option<ToolDefinition> {
    let func = schema.get("function")?;
    let name = func.get("name")?.as_str()?.to_string();
    let description = func
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    let parameters = func
        .get("parameters")
        .cloned()
        .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));

    Some(ToolDefinition {
        name,
        description,
        parameters,
    })
}

// ── Provider Macro ──────────────────────────────────────────────

/// Generate a rig-backed LLM provider struct for a specific vendor.
///
/// Usage:
/// ```rust,ignore
/// rig_provider!(OpenAiProvider, rig::providers::openai::Client, "openai");
/// ```
///
/// This generates:
/// - A struct with `new(api_key, base_url, model)` constructor
/// - An `LlmProvider` implementation that delegates to the shared streaming helper
macro_rules! rig_provider {
    (
        $(#[$meta:meta])*
        $name:ident, $client:ty, $provider_name:expr
    ) => {
        $(#[$meta])*
        pub struct $name {
            client: $client,
            model_name: String,
        }

        impl $name {
            /// Create a new provider.
            ///
            /// * `api_key` — API key for the vendor.
            /// * `base_url` — optional base URL override (e.g. for proxies).
            /// * `model` — model name (e.g. "gpt-4o", "claude-sonnet-4-20250514").
            pub fn new(
                api_key: &str,
                base_url: Option<&str>,
                model: &str,
            ) -> Result<Self, $crate::providers::LlmProviderError> {
                let client = if let Some(url) = base_url {
                    <$client>::builder()
                        .api_key(api_key)
                        .base_url(url)
                        .build()
                        .map_err(|e| {
                            $crate::providers::LlmProviderError::Provider(format!(
                                "{} client error: {}",
                                stringify!($name),
                                e
                            ))
                        })?
                } else {
                    <$client>::new(api_key).map_err(|e| {
                        $crate::providers::LlmProviderError::Provider(format!(
                            "{} client error: {}",
                            stringify!($name),
                            e
                        ))
                    })?
                };
                Ok(Self {
                    client,
                    model_name: model.to_string(),
                })
            }
        }

        #[async_trait::async_trait]
        impl $crate::providers::LlmProvider for $name {
            fn provider_name(&self) -> &str {
                $provider_name
            }

            async fn stream_completion(
                &self,
                messages: &[$crate::agent_backends::ChatMessage],
                tools: Option<&[serde_json::Value]>,
                config: &$crate::providers::LlmCallConfig,
            ) -> Result<
                tokio::sync::mpsc::Receiver<$crate::providers::LlmEvent>,
                $crate::providers::LlmProviderError,
            > {
                use rig::client::CompletionClient;
                let model_name = config.model.as_deref().unwrap_or(&self.model_name);
                let model = self.client.completion_model(model_name);
                super::rig_streaming::stream_rig_completion(&model, messages, tools, config).await
            }
        }
    };
}

pub(crate) use rig_provider;

#[cfg(test)]
mod tests {
    use super::convert_messages;
    use crate::agent_backends::ChatMessage;

    #[test]
    fn convert_messages_preserves_tool_call_signature() {
        let (_, messages) = convert_messages(&[ChatMessage {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(vec![serde_json::json!({
                "id": "call_1",
                "type": "function",
                "function": {
                    "name": "check_availability",
                    "arguments": "{\"date\":\"2026-04-11\"}",
                },
                "signature": "sig_123",
            })]),
            tool_call_id: None,
        }]);

        let rig::completion::message::Message::Assistant { content, .. } = &messages[0] else {
            panic!("expected assistant message");
        };
        let first = content.first();
        let rig::completion::message::AssistantContent::ToolCall(tool_call) = first else {
            panic!("expected tool call");
        };
        assert_eq!(tool_call.signature.as_deref(), Some("sig_123"));
    }
}
