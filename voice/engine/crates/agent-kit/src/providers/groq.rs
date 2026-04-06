//! Groq provider — wraps rig's Groq client with voice-optimized reasoning settings.
//!
//! Unlike the other providers that use the `rig_provider!` macro,
//! Groq needs a custom implementation because:
//! 1. Must set `reasoning_format: Hidden` for reasoning models (Qwen3,
//!    DeepSeek-R1, GPT-OSS) to prevent thinking tokens from being sent as
//!    speech output.  Non-reasoning models (Llama, Gemma, etc.) reject this
//!    parameter with HTTP 400, so it is only set conditionally.
//! 2. Groq's streaming API uses `send_compatible_streaming_request` rather
//!    than the standard OpenAI-compatible client flow.

use futures_util::StreamExt;
use rig::client::CompletionClient;
use rig::completion::message as rig_msg;
use rig::completion::request::{CompletionModel, GetTokenUsage, ToolDefinition};
use rig::providers::groq;
use tokio::sync::mpsc;
use tracing::error;

use crate::providers::{LlmCallConfig, LlmProvider, LlmProviderError};
use crate::providers::rig_streaming::{convert_messages, convert_tool_schema};
use crate::agent_backends::ChatMessage;
use crate::providers::{LlmEvent, ToolCallEvent};

/// LLM provider backed by Groq's API (via rig-core).
///
/// Supports all Groq-hosted models including Qwen3, GPT-OSS,
/// Llama, DeepSeek-R1, and others. For reasoning-capable models,
/// automatically sets `reasoning_format: Hidden` so that thinking
/// tokens are never sent to TTS during voice interactions.
pub struct GroqProvider {
    client: groq::Client,
    model_name: String,
}

impl GroqProvider {
    /// Create a new Groq provider.
    ///
    /// * `api_key` — Groq API key.
    /// * `model` — model name (e.g. `"qwen/qwen3-32b"`, `"llama-3.3-70b-versatile"`).
    pub fn new(api_key: &str, model: &str) -> Result<Self, LlmProviderError> {
        let client = groq::Client::new(api_key)
            .map_err(|e| LlmProviderError::Provider(format!("Groq client error: {}", e)))?;
        Ok(Self {
            client,
            model_name: model.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for GroqProvider {
    fn provider_name(&self) -> &str {
        "groq"
    }
    async fn stream_completion(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[serde_json::Value]>,
        config: &LlmCallConfig,
    ) -> Result<mpsc::Receiver<LlmEvent>, LlmProviderError> {
        let model_name = config.model.as_deref().unwrap_or(&self.model_name);
        let model = self.client.completion_model(model_name);

        let (preamble, rig_messages) = convert_messages(messages);

        let (prompt, chat_history) = if let Some((last, rest)) = rig_messages.split_last() {
            (last.clone(), rest.to_vec())
        } else {
            (rig_msg::Message::user(""), vec![])
        };

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

        // Suppress reasoning tokens for models that support the `reasoning_format`
        // parameter.  Non-reasoning models (Llama, Gemma, Mistral, …) do NOT
        // accept this parameter and will return HTTP 400 if it is set.
        //
        // - reasoning_format: "hidden" — prevents thinking tokens (<think>…</think>)
        //   from appearing in the streamed response.  Without this, Qwen3 emits raw
        //   <think> XML that gets passed straight to TTS and spoken aloud.
        //
        // - reasoning_effort — controls how much the model thinks:
        //   · Qwen 3: "none" disables reasoning entirely (fastest for voice)
        //   · GPT-OSS: "low" minimizes reasoning overhead
        //   · DeepSeek-R1: only reasoning_format is set (no effort knob)
        let model_lower = model_name.to_lowercase();
        // NOTE: Substring matching is inherently fragile (a future model name
        // like "qwen3-fast-no-reasoning" would falsely match).  Acceptable for
        // now because the Groq model list is small and stable; if it grows,
        // switch to an explicit allowlist or a user-configurable flag.
        let is_reasoning_model = model_lower.contains("qwen3")
            || model_lower.contains("qwen-3")
            || model_lower.contains("gpt-oss");

        if is_reasoning_model {
            let mut reasoning_params = serde_json::json!({
                "reasoning_format": "hidden"
            });
            if model_lower.contains("qwen3") || model_lower.contains("qwen-3") {
                reasoning_params["reasoning_effort"] = serde_json::json!("none");
            } else if model_lower.contains("gpt-oss") {
                reasoning_params["reasoning_effort"] = serde_json::json!("low");
            }
            builder = builder.additional_params(reasoning_params);
        }

        let request = builder.build();
        let mut stream = model
            .stream(request)
            .await
            .map_err(|e| LlmProviderError::Provider(format!("Groq stream error: {}", e)))?;

        let (tx, rx) = mpsc::channel::<LlmEvent>(64);

        // Spawn adapter task: poll rig Stream → forward as LlmEvent
        tokio::spawn(async move {
            while let Some(chunk) = stream.next().await {
                let content = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        let msg = format!("Groq stream error: {}", e);
                        error!("[groq] {}", msg);
                        let _ = tx.send(LlmEvent::Error(msg)).await;
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
                    // Reasoning content is intentionally ignored for voice —
                    // thinking tokens should never be sent to TTS.
                    _ => {}
                }
            }
        });

        Ok(rx)
    }
}
