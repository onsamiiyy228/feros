//! In-session artifact store — persistent named memory for the voice LLM.
//!
//! Provides `save_artifact`, `read_artifact`, and `list_artifacts` as
//! synthetic tools intercepted by `ArtifactInterceptor` via the `ToolInterceptor`
//! `before_tool_call → Stub` mechanism.
//!
//! The store is an in-memory `HashMap` scoped to a single call session.
//! Voice sessions are ephemeral (minutes), so there is no need for
//! cross-session persistence — we only need facts to survive context
//! summarization *within* a call.
//!
//! # Why this works
//!
//! The `ToolInterceptor::before_tool_call` returning `Stub(result)` short-
//! circuits execution before any HTTP or script engine is invoked.  The
//! normal agentic loop machinery (`ToolCallStarted` / `ToolCallCompleted`
//! events, conversation history, tool-result transformer) runs unchanged,
//! so artifact calls are pipe-compatible with all existing interceptors including
//! the tool filler — which is intentional (file I/O is faster than any
//! LLM call, so a filler here is harmless and consistent).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::agent_backends::{AfterToolCallAction, BeforeToolCallAction, ToolInterceptor};

// ── Tool name constants ──────────────────────────────────────────

pub const SAVE_ARTIFACT_TOOL: &str = "save_artifact";
pub const READ_ARTIFACT_TOOL: &str = "read_artifact";
pub const LIST_ARTIFACTS_TOOL: &str = "list_artifacts";

/// Maximum artifact content length (bytes).  Prevents a runaway LLM from
/// bloating the in-memory store.
pub const MAX_ARTIFACT_BYTES: usize = 100_000;

// ── ArtifactStore ────────────────────────────────────────────────

/// Shared, in-memory artifact store for a single voice session.
///
/// Cheaply cloneable — all clones share the same underlying `HashMap`.
#[derive(Clone, Default)]
pub struct ArtifactStore {
    inner: Arc<Mutex<HashMap<String, String>>>,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self::default()
    }
}

// ── ArtifactInterceptor ─────────────────────────────────────────────────

/// [`ToolInterceptor`] that intercepts the three artifact tools and handles
/// them entirely in-process via `Stub`, bypassing any HTTP/script
/// execution.
pub struct ArtifactInterceptor {
    store: ArtifactStore,
}

impl ArtifactInterceptor {
    pub fn new(store: ArtifactStore) -> Self {
        Self { store }
    }
}

impl ToolInterceptor for ArtifactInterceptor {
    fn before_tool_call(&self, tool_name: &str, arguments: &str) -> BeforeToolCallAction {
        match tool_name {
            SAVE_ARTIFACT_TOOL => BeforeToolCallAction::Stub(handle_save(&self.store, arguments)),
            READ_ARTIFACT_TOOL => BeforeToolCallAction::Stub(handle_read(&self.store, arguments)),
            LIST_ARTIFACTS_TOOL => BeforeToolCallAction::Stub(handle_list(&self.store)),
            _ => BeforeToolCallAction::Proceed,
        }
    }

    fn after_tool_call(
        &self,
        _tool_name: &str,
        _arguments: &str,
        _result: &str,
    ) -> AfterToolCallAction {
        AfterToolCallAction::PassThrough
    }
}

// ── Handlers ────────────────────────────────────────────────────

fn handle_save(store: &ArtifactStore, arguments: &str) -> String {
    let v: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("Error: invalid arguments — {e}"),
    };

    let name = match v.get("name").and_then(|n| n.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return "Error: missing or empty 'name' argument".to_string(),
    };
    let content = match v.get("content").and_then(|c| c.as_str()) {
        Some(c) => c.to_string(),
        None => return "Error: missing 'content' argument".to_string(),
    };

    if content.len() > MAX_ARTIFACT_BYTES {
        return format!(
            "Artifact too large ({} bytes, max {MAX_ARTIFACT_BYTES}). Summarize or split it.",
            content.len()
        );
    }

    let len = content.len();
    store.inner.lock().unwrap().insert(name.clone(), content);
    tracing::info!("[artifacts] Saved '{}' ({} bytes)", name, len);
    format!("Artifact '{name}' saved ({len} bytes).")
}

fn handle_read(store: &ArtifactStore, arguments: &str) -> String {
    let v: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("Error: invalid arguments — {e}"),
    };

    let name = match v.get("name").and_then(|n| n.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return "Error: missing or empty 'name' argument".to_string(),
    };

    let map = store.inner.lock().unwrap();
    match map.get(&name) {
        Some(content) => {
            tracing::info!("[artifacts] Read '{}' ({} bytes)", name, content.len());
            content.clone()
        }
        None => {
            let mut names: Vec<&str> = map.keys().map(String::as_str).collect();
            names.sort();
            let available = if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            };
            format!("Artifact '{name}' not found. Available: {available}")
        }
    }
}

fn handle_list(store: &ArtifactStore) -> String {
    let map = store.inner.lock().unwrap();
    if map.is_empty() {
        return "No artifacts saved yet.".to_string();
    }
    let mut lines: Vec<String> = map
        .iter()
        .map(|(name, content)| format!("- {name} ({} bytes)", content.len()))
        .collect();
    lines.sort();
    lines.join("\n")
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> ArtifactStore {
        ArtifactStore::new()
    }

    #[test]
    fn save_and_read_roundtrip() {
        let store = make_store();
        let args = r#"{"name": "caller_info.md", "content": "Name: Alice, Phone: 555-1234"}"#;
        let save_result = handle_save(&store, args);
        assert!(save_result.contains("saved"), "got: {save_result}");

        let read_result = handle_read(&store, r#"{"name": "caller_info.md"}"#);
        assert_eq!(read_result, "Name: Alice, Phone: 555-1234");
    }

    #[test]
    fn upsert_overwrites() {
        let store = make_store();
        handle_save(&store, r#"{"name": "x.md", "content": "v1"}"#);
        handle_save(&store, r#"{"name": "x.md", "content": "v2"}"#);
        assert_eq!(handle_read(&store, r#"{"name": "x.md"}"#), "v2");
    }

    #[test]
    fn read_missing_returns_available_list() {
        let store = make_store();
        handle_save(&store, r#"{"name": "a.md", "content": "hello"}"#);
        let result = handle_read(&store, r#"{"name": "missing.md"}"#);
        assert!(result.contains("not found"), "got: {result}");
        assert!(result.contains("a.md"), "got: {result}");
    }

    #[test]
    fn read_when_store_empty_says_none() {
        let store = make_store();
        let result = handle_read(&store, r#"{"name": "missing.md"}"#);
        assert!(result.contains("Available: none"), "got: {result}");
    }

    #[test]
    fn list_empty_store() {
        let store = make_store();
        assert_eq!(handle_list(&store), "No artifacts saved yet.");
    }

    #[test]
    fn list_shows_all_sorted() {
        let store = make_store();
        handle_save(&store, r#"{"name": "b.md", "content": "bb"}"#);
        handle_save(&store, r#"{"name": "a.md", "content": "aaa"}"#);
        let result = handle_list(&store);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("- a.md"), "got: {result}");
        assert!(lines[1].starts_with("- b.md"), "got: {result}");
    }

    #[test]
    fn max_size_enforced() {
        let store = make_store();
        let big = "x".repeat(MAX_ARTIFACT_BYTES + 1);
        let args = format!(r#"{{"name": "big.md", "content": "{big}"}}"#);
        let result = handle_save(&store, &args);
        assert!(result.contains("too large"), "got: {result}");
        // Confirm nothing was stored
        assert_eq!(handle_list(&store), "No artifacts saved yet.");
    }

    #[test]
    fn invalid_json_returns_error() {
        let store = make_store();
        let result = handle_save(&store, "not json");
        assert!(result.starts_with("Error:"), "got: {result}");
    }

    #[test]
    fn missing_name_returns_error() {
        let store = make_store();
        let result = handle_save(&store, r#"{"content": "hello"}"#);
        assert!(result.contains("name"), "got: {result}");
    }

    #[test]
    fn interceptor_stubs_artifact_tools_proceeds_for_others() {
        let store = make_store();
        let interceptor = ArtifactInterceptor::new(store);

        match interceptor
            .before_tool_call(SAVE_ARTIFACT_TOOL, r#"{"name": "x.md", "content": "data"}"#)
        {
            BeforeToolCallAction::Stub(s) => assert!(s.contains("saved"), "got: {s}"),
            _ => panic!("expected Stub"),
        }

        match interceptor.before_tool_call(LIST_ARTIFACTS_TOOL, "{}") {
            BeforeToolCallAction::Stub(s) => assert!(s.contains("x.md"), "got: {s}"),
            _ => panic!("expected Stub"),
        }

        match interceptor.before_tool_call("some_user_tool", "{}") {
            BeforeToolCallAction::Proceed => {}
            _ => panic!("expected Proceed for non-artifact tool"),
        }
    }
}
