//! OpenAI provider — wraps rig's OpenAI client.

pub use rig::providers::openai::*;

use crate::providers::rig_streaming::rig_provider;

rig_provider!(
    /// LLM provider backed by OpenAI's API (via rig-core).
    ///
    /// Supports all OpenAI models.
    /// Also works with OpenAI-compatible endpoints (Azure, local proxies)
    /// via `base_url`.
    ///
    /// Model constants are re-exported from `rig::providers::openai` —
    /// e.g. `openai::GPT_4O`, `openai::O1`, `openai::GPT_4O_MINI`.
    OpenAiProvider,
    rig::providers::openai::Client,
    "openai"
);
