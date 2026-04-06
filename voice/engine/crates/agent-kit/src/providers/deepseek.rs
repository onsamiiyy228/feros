//! DeepSeek provider — wraps rig's DeepSeek client.

pub use rig::providers::deepseek::*;

use crate::providers::rig_streaming::rig_provider;

rig_provider!(
    /// LLM provider backed by DeepSeek's API (via rig-core).
    ///
    /// Supports DeepSeek-V3, DeepSeek-R1, and all DeepSeek models.
    DeepSeekProvider,
    rig::providers::deepseek::Client,
    "deepseek"
);
