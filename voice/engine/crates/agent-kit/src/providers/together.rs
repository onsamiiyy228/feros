//! Together AI provider — wraps rig's OpenAI-compat client with voice-optimized reasoning settings.
//!
//! Together AI has three reasoning model tiers, each with different suppression controls:
//!
//! | Tier           | Models                                               | Suppression                     |
//! |----------------|------------------------------------------------------|---------------------------------|
//! | Reasoning only | MiniMax-M2.5                            | Cannot be disabled — filter only  |
//! | Hybrid         | DeepSeek-V3.1, Qwen3.5 (9B/397B), Kimi-K2.5, GLM-5  | `reasoning: {"enabled": false}` |
//! | Adjustable     | openai/gpt-oss-120b, gpt-oss-20b                     | `reasoning_effort: "low"`       |
//!
//! We gate the reasoning suppression parameters based on specific model IDs,
//! as non-reasoning models (or different versions) may reject undocumented parameters.
//!
//! Note: `reasoning_effort` accepts only `"low"` | `"medium"` | `"high"` — there is no
//! `"none"` value. Do not use it for disabling reasoning.

//!
//! Together AI base URL: https://api.together.xyz/v1

use futures_util::StreamExt;
use rig::client::CompletionClient;
use rig::completion::message as rig_msg;
use rig::completion::request::{CompletionModel, GetTokenUsage, ToolDefinition};
use tokio::sync::mpsc;
use tracing::{error, warn};

use crate::agent_backends::ChatMessage;
use crate::providers::rig_streaming::{convert_messages, convert_tool_schema};
use crate::providers::{LlmCallConfig, LlmEvent, LlmProvider, LlmProviderError, ToolCallEvent};

/// The canonical Together AI base URL.
pub const TOGETHER_BASE_URL: &str = "https://api.together.xyz/v1";

/// LLM provider backed by Together AI's API (via rig-core's OpenAI-compat client).
///
/// Supports Together-hosted models including DeepSeek-R1, Qwen3, Llama, Mistral, etc.
/// For reasoning-capable models, automatically disables reasoning so thinking tokens
/// are never sent to TTS during voice interactions.
pub struct TogetherProvider {
    client: rig::providers::openai::CompletionsClient,
    model_name: String,
}

impl TogetherProvider {
    /// Create a new Together AI provider.
    ///
    /// * `api_key` — Together AI API key.
    /// * `model` — model name (e.g. `"deepseek-ai/DeepSeek-R1"`, `"meta-llama/Llama-3.3-70B-Instruct-Turbo"`).
    pub fn new(api_key: &str, model: &str) -> Result<Self, LlmProviderError> {
        let client = rig::providers::openai::CompletionsClient::builder()
            .api_key(api_key)
            .base_url(TOGETHER_BASE_URL)
            .build()
            .map_err(|e| LlmProviderError::Provider(format!("Together AI client error: {}", e)))?;
        Ok(Self {
            client,
            model_name: model.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for TogetherProvider {
    fn provider_name(&self) -> &str {
        "together"
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

        // Suppress reasoning/thinking tokens on Together AI.
        //
        // Together AI has three model tiers for reasoning:
        // 1. Hybrid (Qwen3.5 9B/397B, DeepSeek-V3.1, Kimi-K2.5, GLM-5) -> accepts `reasoning: {enabled: false}`
        // 2. Adjustable (gpt-oss) -> accepts `reasoning_effort: "low"` (cannot disable entirely)
        // 3. Reasoning-only (MiniMax) -> ignores params, sends thinking tokens anyway
        //
        // We gate the injection based on specific supported model IDs to avoid 400 errors
        // on standard models (Llama, Mistral) or older versions.
        
        let model_lower = model_name.to_lowercase();
        let is_hybrid = model_lower.contains("deepseek-v3.1")
            || model_lower.contains("qwen3.5-397b-a17b")
            || model_lower.contains("qwen3.5-9b")
            || model_lower.contains("kimi-k2.5")
            || model_lower.contains("glm-5");
            
        let is_gpt_oss = model_lower.contains("gpt-oss");

        if is_hybrid {
            builder = builder.additional_params(serde_json::json!({
                "reasoning": { "enabled": false }
            }));
        } else if is_gpt_oss {
            builder = builder.additional_params(serde_json::json!({
                "reasoning_effort": "low"
            }));
        }

        let request = builder.build();
        let mut stream = model
            .stream(request)
            .await
            .map_err(|e| LlmProviderError::Provider(format!("Together AI stream error: {}", e)))?;

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
                        let msg = format!("Together AI stream error: {}", e);
                        error!("[together] {}", msg);
                        // Send the error through the channel so the reactor's
                        // LlmEvent::Error arm can handle it (warn + arm idle timer).
                        // Without this, the channel closes silently and the reactor
                        // treats the empty stream as a normal Finished — going silent.
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

            // Diagnostic: log if we got no text tokens and the stream closed cleanly.
            // Distinguishes "model returned only reasoning chunks" (params being ignored)
            // from "model returned nothing" (API/parsing issue).
            // Skipped on error — the error path already logged the failure.
            if token_count == 0 && !had_error {
                warn!(
                    "[together] Stream closed with 0 text tokens (reasoning_chunks={}). \
                     Model may be returning thinking-only output or empty response.",
                    reasoning_count
                );
            }
        });

        Ok(rx)
    }
}
