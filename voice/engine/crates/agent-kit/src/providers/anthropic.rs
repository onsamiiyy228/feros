//! Anthropic provider — wraps rig's Anthropic client.

pub use rig::providers::anthropic::*;

use crate::providers::rig_streaming::rig_provider;

rig_provider!(
    /// LLM provider backed by Anthropic's API (via rig-core).
    ///
    /// Supports Claude Sonnet, Claude Opus, Claude Haiku,
    /// and all Anthropic models.
    ///
    /// Model constants are re-exported from `rig::providers::anthropic` —
    /// e.g. `anthropic::CLAUDE_3_5_SONNET`.
    AnthropicProvider,
    rig::providers::anthropic::Client,
    "anthropic"
);
