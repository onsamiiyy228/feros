//! OpenAI-compatible provider — wraps rig's OpenAI **Completions** client.
//!
//! Uses the Chat Completions API (`POST /v1/chat/completions`) instead of
//! the newer Responses API (`POST /responses`).  This is the right choice
//! for any OpenAI-compatible endpoint that doesn't implement the Responses
//! API — e.g. Ollama, vLLM, LiteLLM, Azure OpenAI, local proxies.

use crate::providers::rig_streaming::rig_provider;

rig_provider!(
    /// LLM provider for OpenAI-compatible endpoints.
    ///
    /// Unlike [`super::openai::OpenAiProvider`] (which uses the Responses
    /// API), this provider uses the **Chat Completions API** — the
    /// widely-supported `/v1/chat/completions` endpoint.
    ///
    /// Use this for Ollama, vLLM, LiteLLM, Azure OpenAI, and any other
    /// server that speaks the OpenAI chat completions protocol.
    OpenAiCompatProvider,
    rig::providers::openai::CompletionsClient,
    "openai_compat"
);

/// Aggressively normalizes an OpenRouter URL.
/// Converts fallback inputs like `"https://openrouter.ai"` to the
/// correct API endpoint `"https://openrouter.ai/api/v1"`.
pub fn normalize_openrouter_url(url: Option<&str>) -> String {
    let mut u = url.unwrap_or("https://openrouter.ai/api/v1").trim();
    if u.is_empty()
        || u == "https://openrouter.ai"
        || u == "https://openrouter.ai/"
        || u == "https://openrouter.ai/api"
        || u == "https://openrouter.ai/api/"
        || u == "openrouter.ai"
    {
        u = "https://openrouter.ai/api/v1";
    }
    u.to_string()
}
