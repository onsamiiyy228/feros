//! Google Gemini provider — wraps rig's Gemini client.

pub use rig::providers::gemini::*;

use crate::providers::rig_streaming::rig_provider;

rig_provider!(
    /// LLM provider backed by Google's Gemini API (via rig-core).
    ///
    /// Supports Gemini Pro, Gemini Flash, and all Gemini models.
    ///
    /// Model constants are re-exported from `rig::providers::gemini` —
    /// e.g. `gemini::EMBEDDING_001`.
    GeminiProvider,
    rig::providers::gemini::Client,
    "gemini"
);
