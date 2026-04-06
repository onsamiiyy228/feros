use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::agent_backends::{AfterToolCallAction, BeforeToolCallAction, ToolInterceptor};
use crate::ScriptEngine;

// ── Types ───────────────────────────────────────────────────────

/// Internal tool execution result.
#[derive(Debug, Clone)]
pub(super) struct ToolOutcome {
    pub success: bool,
    pub result: String,
}

impl ToolOutcome {
    pub(super) fn success(result: String) -> Self {
        Self {
            success: true,
            result,
        }
    }

    pub(super) fn failure(result: String) -> Self {
        Self {
            success: false,
            result,
        }
    }

    /// Parse the output of a QuickJS tool script and determine if it represents a success or error.
    ///
    /// This function interprets that JSON, unwraps the inner payload, and constructs a
    /// `ToolOutcome` with the appropriate `success` status.
    pub(super) fn classify_script_result(result_text: String) -> Self {
        let trimmed = result_text.trim();

        // Fast path for structured JSON results
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(trimmed) {
            // Check for explicit error objects: { error: "..." }
            if let Some(err_val) = map.get("error") {
                if !matches!(
                    err_val,
                    serde_json::Value::Null | serde_json::Value::Bool(false)
                ) {
                    let err_msg = match err_val {
                        serde_json::Value::String(s) => s.clone(),
                        val => val.to_string(),
                    };
                    return Self::failure(err_msg);
                }
            }

            // Unpack successful structured results: { result: "..." } / { data: "..." }
            let success_val = map
                .get("result")
                .or_else(|| map.get("data"))
                .or_else(|| map.get("message"));

            if let Some(val) = success_val {
                let val_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    v => v.to_string(),
                };
                return Self::success(val_str);
            }
        }

        // If it's not JSON, or it is JSON without specific designated keys,
        // treat it as a success and just return the raw string. This gracefully
        // handles returning plain strings or arbitrary JSON without crashing.
        Self::success(result_text)
    }
}

/// Internal tool execution task result sent over the channel.
pub(super) struct ToolTaskResult {
    pub call_id: String,
    pub name: String,
    pub success: bool,
    pub result: String,
}

// ── Pipeline ────────────────────────────────────────────────────

/// Spawns a background task to execute a tool call, routing through hooks and engines.
///
/// Sends the outcome back down `tx` as a `ToolTaskResult`.
pub(super) fn spawn_tool_task(
    call_id: String,
    name: String,
    args: String,
    side_effect: bool,
    script_engine_opt: Option<Arc<ScriptEngine>>,
    interceptor_opt: Option<Arc<dyn ToolInterceptor>>,
    tx: mpsc::UnboundedSender<ToolTaskResult>,
) {
    tokio::spawn(async move {
        let task_name = name.clone();

        // Timeout wrapper over the execution hooks
        let result_fut = async {
            // ── Before hook ──────────────────────────────────────
            let before_action = if let Some(ref interceptor) = interceptor_opt {
                let h = Arc::clone(interceptor);
                let tn = name.clone();
                let ag = args.clone();
                tokio::task::spawn_blocking(move || h.before_tool_call(&tn, &ag))
                    .await
                    .unwrap_or(BeforeToolCallAction::Proceed)
            } else {
                BeforeToolCallAction::Proceed
            };

            match before_action {
                BeforeToolCallAction::Stub(canned) => {
                    info!(
                        "[interceptor] Stubbed tool '{}' → {} chars",
                        name,
                        canned.len()
                    );
                    ToolOutcome::classify_script_result(canned)
                }
                BeforeToolCallAction::Proceed => {
                    // ── Normal execution ──────────────────────────
                    let actual = if let Some(engine) = script_engine_opt
                        .as_ref()
                        .filter(|r| r.get(&name).is_some())
                    {
                        // Graph tool — run on blocking thread
                        let engine_clone = Arc::clone(engine);
                        let tn = name.clone();
                        let ag = args.clone();
                        match tokio::task::spawn_blocking(move || engine_clone.execute(&tn, &ag))
                            .await
                        {
                            Ok(Ok(r)) => ToolOutcome::classify_script_result(r),
                            Ok(Err(e)) => {
                                let error_message = e.to_string();
                                ToolOutcome::failure(format!("Tool error: {error_message}"))
                            }
                            Err(e) => {
                                let error_message = format!("Tool task panicked: {e}");
                                ToolOutcome::failure(error_message)
                            }
                        }
                    } else {
                        // Tool not found in graph — fail immediately
                        ToolOutcome::failure(format!(
                            "Tool error: '{}' is not defined in the agent graph.",
                            name
                        ))
                    };

                    // ── After hook ────────────────────────────────
                    if let Some(ref interceptor) = interceptor_opt {
                        let h = Arc::clone(interceptor);
                        let tn = name.clone();
                        let ag = args.clone();
                        let res = actual.result.clone();
                        let after_action =
                            tokio::task::spawn_blocking(move || h.after_tool_call(&tn, &ag, &res))
                                .await
                                .unwrap_or(AfterToolCallAction::PassThrough);
                        match after_action {
                            AfterToolCallAction::Override(replaced) => {
                                info!(
                                    "[interceptor] Overrode tool '{}' result → {} chars",
                                    name,
                                    replaced.len()
                                );
                                ToolOutcome::classify_script_result(replaced)
                            }
                            AfterToolCallAction::PassThrough => actual,
                        }
                    } else {
                        actual
                    }
                }
            }
        };

        // Execution Timeout limit (25 seconds)
        let result =
            match tokio::time::timeout(std::time::Duration::from_secs(25), result_fut).await {
                Ok(r) => r,
                Err(_) => {
                    let error_message =
                        format!("Tool '{}' execution timed out after 25 seconds.", task_name);
                    ToolOutcome::failure(error_message)
                }
            };

        if tx
            .send(ToolTaskResult {
                call_id: call_id.clone(),
                name: task_name.clone(),
                success: result.success,
                result: result.result.clone(),
            })
            .is_err()
        {
            // Session ended (hang-up or disconnect) before the tool finished
            warn!(
                tool.name = %task_name,
                tool.call_id = %call_id,
                tool.side_effect = side_effect,
                tool.result_chars = result.result.len(),
                "[agent_backend] Tool completed after session ended (result orphaned)"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_plain_string_is_success() {
        let out = ToolOutcome::classify_script_result("hello world".to_string());
        assert!(out.success);
        assert_eq!(out.result, "hello world");
    }

    #[test]
    fn classify_result_key_unwrapped() {
        let out = ToolOutcome::classify_script_result(r#"{"result": "ok"}"#.to_string());
        assert!(out.success);
        assert_eq!(out.result, "ok");
    }

    #[test]
    fn classify_data_key_unwrapped() {
        let out = ToolOutcome::classify_script_result(r#"{"data": "payload"}"#.to_string());
        assert!(out.success);
        assert_eq!(out.result, "payload");
    }

    #[test]
    fn classify_message_key_unwrapped() {
        let out = ToolOutcome::classify_script_result(r#"{"message": "done"}"#.to_string());
        assert!(out.success);
        assert_eq!(out.result, "done");
    }

    #[test]
    fn classify_error_string_is_failure() {
        let out =
            ToolOutcome::classify_script_result(r#"{"error": "something broke"}"#.to_string());
        assert!(!out.success);
        assert_eq!(out.result, "something broke");
    }

    #[test]
    fn classify_error_null_treated_as_success() {
        // { error: null } means "no error" — not a failure
        let out = ToolOutcome::classify_script_result(r#"{"error": null}"#.to_string());
        assert!(out.success);
    }

    #[test]
    fn classify_error_false_treated_as_success() {
        // { error: false } is not an error
        let out = ToolOutcome::classify_script_result(r#"{"error": false}"#.to_string());
        assert!(out.success);
    }

    #[test]
    fn classify_arbitrary_json_object_is_success() {
        // JSON object with no designated keys passes through raw
        let input = r#"{"foo": "bar", "count": 3}"#;
        let out = ToolOutcome::classify_script_result(input.to_string());
        assert!(out.success);
        assert_eq!(out.result, input);
    }

    #[test]
    fn classify_non_json_is_success() {
        let out = ToolOutcome::classify_script_result("Not JSON at all".to_string());
        assert!(out.success);
        assert_eq!(out.result, "Not JSON at all");
    }

    #[test]
    fn classify_result_key_numeric_value() {
        // Non-string values under "result" are coerced to their JSON string repr
        let out = ToolOutcome::classify_script_result(r#"{"result": 42}"#.to_string());
        assert!(out.success);
        assert_eq!(out.result, "42");
    }

    #[test]
    fn classify_whitespace_trimmed_before_parsing() {
        let out =
            ToolOutcome::classify_script_result("  {\"result\": \"trimmed\"}  ".to_string());
        assert!(out.success);
        assert_eq!(out.result, "trimmed");
    }
}
