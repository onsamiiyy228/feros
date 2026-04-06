//! Provider configuration — runtime selection of LLM backend.
//!
//! The `ProviderConfig` enum determines which `LlmProvider` implementation
//! to construct. It's `serde`-deserializable for JSON/TOML config files.

use std::time::Duration;

use serde::Deserialize;

use crate::providers::{LlmProvider, LlmProviderError};

/// Shared fields for rig-backed provider configs.
#[derive(Debug, Clone, Deserialize)]
pub struct RigProviderFields {
    pub api_key: String,
    /// Optional base URL override (e.g. for proxies, Azure).
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
}

/// Configuration for selecting and constructing an LLM provider at runtime.
///
/// # Examples
///
/// ```json
/// { "type": "openai_compat", "url": "https://api.openai.com", "api_key": "sk-...", "model": "gpt-4o" }
/// ```
///
/// ```json
/// { "type": "openai", "api_key": "sk-...", "model": "gpt-4o" }
/// ```
///
/// ```json
/// { "type": "anthropic", "api_key": "sk-ant-...", "model": "claude-sonnet-4-20250514" }
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProviderConfig {
    /// OpenAI-compatible endpoint (via rig-core's OpenAI client with a custom base URL).
    /// Works with any endpoint that speaks the OpenAI protocol (Ollama, vLLM, LiteLLM, etc.).
    #[serde(rename = "openai_compat")]
    OpenAiCompat {
        url: String,
        api_key: String,
        model: String,
    },

    /// OpenAI provider (via rig-core).
    OpenAi {
        #[serde(flatten)]
        fields: RigProviderFields,
    },

    /// Anthropic provider (via rig-core).
    Anthropic {
        #[serde(flatten)]
        fields: RigProviderFields,
    },

    /// Google Gemini provider (via rig-core).
    Gemini {
        #[serde(flatten)]
        fields: RigProviderFields,
    },

    /// DeepSeek provider (via rig-core).
    DeepSeek {
        #[serde(flatten)]
        fields: RigProviderFields,
    },

    /// OpenRouter provider.
    #[serde(rename = "openrouter")]
    OpenRouter {
        #[serde(flatten)]
        fields: RigProviderFields,
    },

    /// Fallback provider — tries providers in order until one works.
    Fallback {
        /// Timeout in ms per provider attempt before trying the next.
        #[serde(default = "default_attempt_timeout_ms")]
        attempt_timeout_ms: u64,
        /// Ordered list of provider configs to try.
        providers: Vec<NamedProviderConfig>,
    },
}

/// A named provider used inside `Fallback` config.
#[derive(Debug, Clone, Deserialize)]
pub struct NamedProviderConfig {
    pub name: String,
    #[serde(flatten)]
    pub config: ProviderConfig,
}

fn default_attempt_timeout_ms() -> u64 {
    5000
}

impl ProviderConfig {
    /// Construct a boxed `LlmProvider` from this configuration.
    pub fn build(self) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
        match self {
            ProviderConfig::OpenAiCompat {
                url,
                api_key,
                model,
            } => {
                use crate::providers::openai_compat::OpenAiCompatProvider;
                Ok(Box::new(OpenAiCompatProvider::new(
                    &api_key,
                    Some(&url),
                    &model,
                )?))
            }

            ProviderConfig::OpenAi { fields } => {
                use crate::providers::openai::OpenAiProvider;
                Ok(Box::new(OpenAiProvider::new(
                    &fields.api_key,
                    fields.base_url.as_deref(),
                    &fields.model,
                )?))
            }

            ProviderConfig::Anthropic { fields } => {
                use crate::providers::anthropic::AnthropicProvider;
                Ok(Box::new(AnthropicProvider::new(
                    &fields.api_key,
                    fields.base_url.as_deref(),
                    &fields.model,
                )?))
            }

            ProviderConfig::Gemini { fields } => {
                use crate::providers::gemini::GeminiProvider;
                Ok(Box::new(GeminiProvider::new(
                    &fields.api_key,
                    fields.base_url.as_deref(),
                    &fields.model,
                )?))
            }

            ProviderConfig::DeepSeek { fields } => {
                use crate::providers::deepseek::DeepSeekProvider;
                Ok(Box::new(DeepSeekProvider::new(
                    &fields.api_key,
                    fields.base_url.as_deref(),
                    &fields.model,
                )?))
            }

            ProviderConfig::OpenRouter { fields } => {
                use crate::providers::openai_compat::{OpenAiCompatProvider, normalize_openrouter_url};
                let url = normalize_openrouter_url(fields.base_url.as_deref());
                Ok(Box::new(OpenAiCompatProvider::new(
                    &fields.api_key,
                    Some(&url),
                    &fields.model,
                )?))
            }

            ProviderConfig::Fallback {
                attempt_timeout_ms,
                providers,
            } => {
                use crate::providers::fallback::FallbackProvider;
                let mut built: Vec<(String, Box<dyn LlmProvider>)> = Vec::new();
                for named in providers {
                    let p = named.config.build()?;
                    built.push((named.name, p));
                }
                if built.is_empty() {
                    return Err(LlmProviderError::Provider(
                        "fallback config has no providers".to_string(),
                    ));
                }
                Ok(Box::new(FallbackProvider::new(
                    built,
                    Duration::from_millis(attempt_timeout_ms),
                )))
            }
        }
    }
}
