//! LLM provider — re-exports from the `agent-kit` crate.
//!
//! The actual LLM client and types live in `crates/agent-kit`.
//! This module re-exports them for backward compatibility.

pub use agent_kit::{ChatMessage, LlmEvent, ToolCallEvent};
pub use agent_kit::{LlmCallConfig, LlmProvider, LlmProviderError};
