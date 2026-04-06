//! Fireworks AI provider — wraps rig's OpenAI-compat client with voice-optimized reasoning settings.
//!
//! Fireworks AI supports reasoning control via two mutually exclusive parameters:
//!   - `reasoning_effort`: "none" | "low" | "medium" | "high"
//!   - `thinking`: Anthropic-compatible `{type: "enabled", budget_tokens: N}` or `{type: "disabled"}`
//!
//! For voice agents we set `thinking: {type: "disabled"}` which fully disables reasoning
//! (not just minimizes it). Thinking tokens like <think>…</think> must never reach TTS.
//! Fireworks hosts a large and growing model catalog so the param is applied unconditionally —
//! non-reasoning models silently ignore unknown parameters.
//!
//! Fireworks AI base URL: https://api.fireworks.ai/inference/v1

use futures_util::StreamExt;
use rig::client::CompletionClient;
use rig::completion::message as rig_msg;
use rig::completion::request::{CompletionModel, GetTokenUsage, ToolDefinition};
use tokio::sync::mpsc;
use tracing::{error, warn};

use crate::agent_backends::ChatMessage;
use crate::providers::rig_streaming::{convert_messages, convert_tool_schema};
use crate::providers::{LlmCallConfig, LlmEvent, LlmProvider, LlmProviderError, ToolCallEvent};

/// The canonical Fireworks AI base URL.
pub const FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";

/// LLM provider backed by Fireworks AI's API (via rig-core's OpenAI-compat client).
///
/// Supports Fireworks-hosted models including DeepSeek-R1, Qwen3, Llama, Phi, etc.
/// Unconditionally disables reasoning output so thinking tokens are never sent to TTS
/// during voice interactions.
pub struct FireworksProvider {
    client: rig::providers::openai::CompletionsClient,
    model_name: String,
}

impl FireworksProvider {
    /// Create a new Fireworks AI provider.
    ///
    /// * `api_key` — Fireworks AI API key.
    /// * `model` — model name (e.g. `"accounts/fireworks/models/deepseek-r1"`,
    ///   `"accounts/fireworks/models/llama-v3p3-70b-instruct"`).
    pub fn new(api_key: &str, model: &str) -> Result<Self, LlmProviderError> {
        let client = rig::providers::openai::CompletionsClient::builder()
            .api_key(api_key)
            .base_url(FIREWORKS_BASE_URL)
            .build()
            .map_err(|e| LlmProviderError::Provider(format!("Fireworks AI client error: {}", e)))?;
        Ok(Self {
            client,
            model_name: model.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for FireworksProvider {
    fn provider_name(&self) -> &str {
        "fireworks"
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

        // Always disable reasoning/thinking tokens on Fireworks AI.
        //
        // Fireworks hosts a large and growing model catalog. Non-reasoning models
        // silently ignore unknown parameters, so it is safe to send this for all
        // models without per-model detection.
        //
        // We use the Anthropic-compatible `thinking` parameter with `type: "disabled"`
        // which fully disables reasoning — stronger than `reasoning_effort: "none"` which
        // only minimizes it. The two params are mutually exclusive; never set both.
        //
        // Thinking tokens must never reach TTS — they would be read aloud verbatim.
        builder = builder.additional_params(serde_json::json!({
            "thinking": { "type": "disabled" }
        }));

        let request = builder.build();
        let mut stream = model
            .stream(request)
            .await
            .map_err(|e| LlmProviderError::Provider(format!("Fireworks AI stream error: {}", e)))?;

        let (tx, rx) = mpsc::channel::<LlmEvent>(64);

        tokio::spawn(async move {
            let mut token_count = 0u32;
            let mut reasoning_count = 0u32;
            let mut had_error = false;

            while let Some(chunk) = stream.next().await {
                let content = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        had_error = true;
                        let msg = format!("Fireworks AI stream error: {}", e);
                        error!("[fireworks] {}", msg);
                        let _ = tx.send(LlmEvent::Error(msg)).await;
                        break;
                    }
                };
                match content {
                    rig::streaming::StreamedAssistantContent::Text(text) => {
                        if !text.text.is_empty() {
                            token_count += 1;
                            if tx.send(LlmEvent::Token(text.text.clone())).await.is_err() {
                                return; // Receiver dropped (barge-in)
                            }
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
                    _ => {
                        reasoning_count += 1;
                    }
                }
            }

            if token_count == 0 && !had_error {
                warn!(
                    "[fireworks] Stream closed with 0 text tokens (reasoning_chunks={}). \
                     Model may be returning thinking-only output or empty response.",
                    reasoning_count
                );
            }
        });

        Ok(rx)
    }
}
