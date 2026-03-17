//! Codex CLI subprocess driver.
//!
//! Spawns `codex exec --json` as a subprocess to leverage Codex CLI's built-in
//! ChatGPT OAuth authentication. Reads stdout line-by-line to create Sentry
//! child spans for each tool call with real timing.

use crate::llm_driver::{CompletionRequest, CompletionResponse, LlmDriver, LlmError};
use async_trait::async_trait;
use openfang_types::message::{ContentBlock, MessageContent, Role, StopReason, TokenUsage};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

/// Driver that delegates to the `codex` CLI binary (Codex CLI).
///
/// Auth is handled by the CLI itself (ChatGPT OAuth). No API key needed.
/// Uses `--json` to capture tool calls for Sentry tracing.
pub struct CodexCliDriver;

#[async_trait]
impl LlmDriver for CodexCliDriver {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let prompt = serialize_prompt(&request);
        let requested_model = selected_model_arg(&request.model);
        let sentry_parent_span = request.sentry_parent_span.clone();

        let result = run_codex_streaming(
            &prompt,
            requested_model.as_deref(),
            sentry_parent_span.clone(),
        )
        .await;

        // Guardrail: if codex rejects the request format and we passed a model,
        // retry once without --model because OAuth accounts often only support
        // the CLI default model.
        if let Err(ref e) = result {
            if requested_model.is_some() && is_request_format_error(&e.to_string()) {
                warn!(
                    model = requested_model.as_deref().unwrap_or_default(),
                    error = %e,
                    "codex request format error with explicit model; retrying once without --model"
                );
                return run_codex_streaming(&prompt, None, sentry_parent_span).await;
            }
        }

        result
    }
}

/// Spawn `codex exec --json`, read stdout line-by-line, and create Sentry
/// child spans for each tool call in real time.
async fn run_codex_streaming(
    prompt: &str,
    model: Option<&str>,
    parent_span: Option<std::sync::Arc<sentry::TransactionOrSpan>>,
) -> Result<CompletionResponse, LlmError> {
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
    ];
    if let Some(model) = model {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    // Read prompt from stdin to avoid argv length/escaping edge cases.
    args.push("-".to_string());

    debug!(
        args = ?args,
        prompt_len = prompt.len(),
        "Spawning codex CLI subprocess (streaming)"
    );

    let mut child = tokio::process::Command::new("codex")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| LlmError::Http(format!("Failed to spawn codex CLI: {e}")))?;

    // Write prompt to stdin, then drop to signal EOF
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| LlmError::Http(format!("Failed to write to codex stdin: {e}")))?;
    }

    // Take stdout for line-by-line reading
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| LlmError::Http("No stdout from codex CLI".to_string()))?;

    // Collect stderr in background
    let stderr_handle = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        if let Some(mut stderr) = stderr_handle {
            let _ = stderr.read_to_string(&mut buf).await;
        }
        buf
    });

    // Parse JSONL stream with timeout (5 min for long-running tool calls)
    let parse_result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        parse_codex_stream(stdout, parent_span.as_deref()),
    )
    .await;

    // Wait for child exit
    let status = child
        .wait()
        .await
        .map_err(|e| LlmError::Http(format!("codex CLI process error: {e}")))?;

    let stderr_text = stderr_task.await.unwrap_or_default();

    match parse_result {
        Ok(Ok(response)) => {
            if !status.success() {
                let failure = extract_codex_failure_message(&response.text(), &stderr_text);
                warn!(
                    exit_code = ?status.code(),
                    error = %failure,
                    "codex CLI failed (streaming)"
                );
                return Err(LlmError::Api {
                    status: status.code().unwrap_or(1) as u16,
                    message: format!("codex CLI exited with error: {failure}"),
                });
            }
            Ok(response)
        }
        Ok(Err(e)) => {
            if !status.success() {
                let failure = extract_codex_failure_message("", &stderr_text);
                return Err(LlmError::Api {
                    status: status.code().unwrap_or(1) as u16,
                    message: format!("codex CLI exited with error: {failure}"),
                });
            }
            Err(e)
        }
        Err(_) => {
            warn!("codex CLI subprocess timed out after 300s");
            Err(LlmError::Http(
                "codex CLI subprocess timed out after 300s".to_string(),
            ))
        }
    }
}

/// Parse JSONL stream from `codex exec --json`, creating Sentry child spans
/// for each tool call with real timing.
///
/// Key events:
/// - `item.started` / `item.created` for tool-like items → start a tool span
/// - `item.completed` with matching item ID → finish the tool span
/// - `item.completed` with `item.type == "agent_message"` → extract response text
/// - `turn.completed` → extract usage tokens
async fn parse_codex_stream(
    stdout: tokio::process::ChildStdout,
    parent_span: Option<&sentry::TransactionOrSpan>,
) -> Result<CompletionResponse, LlmError> {
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    let mut text_parts: Vec<String> = Vec::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut tool_span_count: u32 = 0;

    // Track active tool spans by item ID
    let mut active_tool_spans: HashMap<String, sentry::TransactionOrSpan> = HashMap::new();
    // Collect all lines for fallback parsing
    let mut all_lines: Vec<String> = Vec::new();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        all_lines.push(trimmed.clone());

        let json: serde_json::Value = match serde_json::from_str(&trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            // Codex error events — surface these as LLM errors
            "error" | "turn.failed" => {
                // Finish all active tool spans first
                for (id, span) in active_tool_spans.drain() {
                    debug!(item_id = id.as_str(), "Finishing tool span due to error");
                    span.set_status(sentry::protocol::SpanStatus::InternalError);
                    span.finish();
                }

                let msg = json
                    .get("message")
                    .or_else(|| json.pointer("/error/message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown codex error");
                return Err(LlmError::Api {
                    status: 0,
                    message: format!("codex CLI error: {msg}"),
                });
            }

            // Tool item started — start a span for command/tool execution items
            "item.created" | "item.started" => {
                if let Some(tool_start) = extract_codex_tool_start(&json) {
                    debug!(
                        tool_name = tool_start.tool_name.as_str(),
                        item_id = tool_start.item_id.as_str(),
                        command = tool_start.command.as_deref().unwrap_or(""),
                        "codex subprocess: tool call started"
                    );

                    if let Some(parent) = parent_span {
                        let span = parent.start_child("tool.execute", &tool_start.tool_name);
                        span.set_data("tool.name", tool_start.tool_name.clone().into());
                        span.set_data("tool.id", tool_start.item_id.clone().into());
                        span.set_data("tool.source", "codex-cli".into());
                        if let Some(command) = tool_start.command {
                            span.set_data("tool.command", command.into());
                        }
                        let span: sentry::TransactionOrSpan = span.into();
                        active_tool_spans.insert(tool_start.item_id, span);
                        tool_span_count += 1;
                    }
                }
            }

            // Item completed — finish tool spans or extract text
            "item.completed" => {
                let item_type = json
                    .pointer("/item/type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let item_id = json
                    .pointer("/item/id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Finish tool span if this completes a tool-like item.
                if is_codex_tool_item_type(item_type) {
                    if let Some(span) = active_tool_spans.remove(item_id) {
                        let tool_error = json.pointer("/item/error").map(|_| true).unwrap_or(false);
                        if tool_error {
                            span.set_status(sentry::protocol::SpanStatus::InternalError);
                        } else {
                            span.set_status(sentry::protocol::SpanStatus::Ok);
                        }
                        debug!(item_id, tool_error, "Finishing codex tool span");
                        span.finish();
                    }
                }

                // Extract text from agent_message items
                if item_type == "agent_message" {
                    if let Some(text) = json.pointer("/item/text").and_then(|v| v.as_str()) {
                        text_parts.push(text.to_string());
                    }
                    if let Some(content) = json.pointer("/item/content").and_then(|v| v.as_array())
                    {
                        for block in content {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(text.to_string());
                            }
                        }
                    }
                }
            }

            // Turn completed — extract usage
            "turn.completed" => {
                input_tokens = json
                    .pointer("/usage/input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                output_tokens = json
                    .pointer("/usage/output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
            }

            _ => {} // Ignore other event types
        }
    }

    // Finish any orphaned tool spans
    for (id, span) in active_tool_spans.drain() {
        debug!(item_id = id.as_str(), "Finishing orphaned codex tool span");
        span.set_status(sentry::protocol::SpanStatus::Aborted);
        span.finish();
    }

    debug!(
        text_parts_count = text_parts.len(),
        input_tokens, output_tokens, tool_span_count, "codex stream parsing complete"
    );

    // If no structured events found, try fallback
    if text_parts.is_empty() && !all_lines.is_empty() {
        let combined = all_lines.join("\n");
        if let Ok(response) = parse_codex_jsonl_fallback(&combined) {
            return Ok(response);
        }
    }

    let result_text = text_parts.join("");

    if result_text.is_empty() {
        return Err(LlmError::Parse(
            "No agent_message content found in codex CLI output".to_string(),
        ));
    }

    Ok(CompletionResponse {
        content: vec![ContentBlock::Text { text: result_text }],
        stop_reason: StopReason::EndTurn,
        tool_calls: vec![],
        usage: TokenUsage {
            input_tokens,
            output_tokens,
        },
    })
}

fn is_codex_tool_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "function_call" | "tool_call" | "command_execution"
    )
}

struct CodexToolStart {
    item_id: String,
    tool_name: String,
    command: Option<String>,
}

fn extract_codex_tool_start(json: &serde_json::Value) -> Option<CodexToolStart> {
    let item_type = json.pointer("/item/type").and_then(|v| v.as_str())?;
    if !is_codex_tool_item_type(item_type) {
        return None;
    }

    let item_id = json
        .pointer("/item/id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if item_id.is_empty() {
        return None;
    }

    let command = json
        .pointer("/item/command")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let tool_name = match item_type {
        "command_execution" => "command_execution".to_string(),
        _ => json
            .pointer("/item/name")
            .or_else(|| json.pointer("/item/function/name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
    };

    Some(CodexToolStart {
        item_id,
        tool_name,
        command,
    })
}

/// Fallback parser for non-streaming codex output.
fn parse_codex_jsonl_fallback(raw: &str) -> Result<CompletionResponse, LlmError> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let json: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "error" | "turn.failed" => {
                let msg = json
                    .get("message")
                    .or_else(|| json.pointer("/error/message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown codex error");
                return Err(LlmError::Api {
                    status: 0,
                    message: format!("codex CLI error: {msg}"),
                });
            }
            "item.completed" => {
                let item_type = json
                    .pointer("/item/type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if item_type == "agent_message" {
                    if let Some(text) = json.pointer("/item/text").and_then(|v| v.as_str()) {
                        text_parts.push(text.to_string());
                    }
                    if let Some(content) = json.pointer("/item/content").and_then(|v| v.as_array())
                    {
                        for block in content {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(text.to_string());
                            }
                        }
                    }
                }
            }
            "turn.completed" => {
                input_tokens = json
                    .pointer("/usage/input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                output_tokens = json
                    .pointer("/usage/output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
            }
            _ => {}
        }
    }

    // Single-JSON fallback
    if text_parts.is_empty() {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(text) = json.get("result").and_then(|v| v.as_str()) {
                    text_parts.push(text.to_string());
                } else if let Some(text) = json.get("text").and_then(|v| v.as_str()) {
                    text_parts.push(text.to_string());
                }
            }
        }
    }

    let result_text = text_parts.join("");

    if result_text.is_empty() {
        return Err(LlmError::Parse(
            "No agent_message content found in codex CLI output".to_string(),
        ));
    }

    Ok(CompletionResponse {
        content: vec![ContentBlock::Text { text: result_text }],
        stop_reason: StopReason::EndTurn,
        tool_calls: vec![],
        usage: TokenUsage {
            input_tokens,
            output_tokens,
        },
    })
}

/// Serialize a `CompletionRequest` into a single prompt string for codex exec.
///
/// Codex CLI doesn't have a `--system-prompt` flag, so system prompts are
/// prepended to the user prompt.
fn serialize_prompt(request: &CompletionRequest) -> String {
    let mut parts = Vec::new();

    // Codex doesn't support --system-prompt, so prepend it
    if let Some(ref system) = request.system {
        parts.push(format!("[System]\n{system}"));
    }

    for msg in &request.messages {
        let role_label = match msg.role {
            Role::System => "System",
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        let text = extract_text(&msg.content);
        if !text.is_empty() {
            parts.push(format!("{role_label}: {text}"));
        }
    }

    parts.join("\n\n")
}

fn selected_model_arg(model: &str) -> Option<String> {
    let normalized = model.trim();
    if normalized.is_empty()
        || normalized == "default"
        || normalized == "gpt-4o"
        || normalized == "gpt-4o-mini"
    {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn is_request_format_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("invalid request format")
        || lower.contains("invalid request")
        || lower.contains("malformed")
        || lower.contains("missing field")
        || lower.contains("validation error")
        || lower.contains("schema")
}

fn extract_codex_failure_message(stdout: &str, stderr: &str) -> String {
    let stderr_msg = filtered_stderr(stderr);
    if stderr_msg != "unknown codex CLI error" {
        return stderr_msg;
    }

    // codex --json frequently reports failures as JSONL events on stdout.
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(event_type, "error" | "turn.failed") {
            if let Some(msg) = json
                .get("message")
                .or_else(|| json.pointer("/error/message"))
                .and_then(|v| v.as_str())
            {
                let trimmed = msg.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }

    // Single-JSON fallback shape.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        if let Some(msg) = json
            .get("message")
            .or_else(|| json.pointer("/error/message"))
            .or_else(|| json.get("result"))
            .and_then(|v| v.as_str())
        {
            let trimmed = msg.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    stderr_msg
}

fn filtered_stderr(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("WARNING: proceeding"))
        .collect();

    if lines.is_empty() {
        "unknown codex CLI error".to_string()
    } else {
        lines.join(" | ")
    }
}

/// Extract plain text from a `MessageContent` (delegates to shared method).
fn extract_text(content: &MessageContent) -> String {
    content.text_content()
}

/// Check if the `codex` binary is available on PATH.
pub fn is_available() -> bool {
    super::binary_on_path("codex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_codex_jsonl_agent_message_flat_text() {
        // Real codex output format: flat "text" field on agent_message
        let jsonl = r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Hello from codex!"}}
{"type":"turn.completed","usage":{"input_tokens":15,"output_tokens":8}}"#;

        let resp = parse_codex_jsonl_fallback(jsonl).unwrap();
        assert_eq!(resp.text(), "Hello from codex!");
        assert_eq!(resp.usage.input_tokens, 15);
        assert_eq!(resp.usage.output_tokens, 8);
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_codex_jsonl_content_array() {
        // Forward compat: content array format
        let jsonl = r#"{"type":"item.completed","item":{"type":"agent_message","content":[{"text":"Hello from codex!"}]}}
{"type":"turn.completed","usage":{"input_tokens":15,"output_tokens":8}}"#;

        let resp = parse_codex_jsonl_fallback(jsonl).unwrap();
        assert_eq!(resp.text(), "Hello from codex!");
    }

    #[test]
    fn test_parse_codex_jsonl_multiple_messages() {
        let jsonl = r#"{"type":"item.completed","item":{"type":"agent_message","text":"Part 1"}}
{"type":"item.completed","item":{"type":"agent_message","text":" Part 2"}}
{"type":"turn.completed","usage":{"input_tokens":20,"output_tokens":12}}"#;

        let resp = parse_codex_jsonl_fallback(jsonl).unwrap();
        assert_eq!(resp.text(), "Part 1 Part 2");
        assert_eq!(resp.usage.input_tokens, 20);
    }

    #[test]
    fn test_parse_codex_jsonl_no_content() {
        let jsonl = r#"{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":0}}"#;
        let result = parse_codex_jsonl_fallback(jsonl);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_codex_jsonl_error_event() {
        let jsonl = r#"{"type":"error","message":"model not supported"}
{"type":"turn.failed","error":{"message":"model not supported"}}"#;
        let result = parse_codex_jsonl_fallback(jsonl);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("model not supported"));
    }

    #[test]
    fn test_parse_codex_jsonl_ignores_non_agent_items() {
        let jsonl = r#"{"type":"item.completed","item":{"type":"reasoning","text":"thinking..."}}
{"type":"item.completed","item":{"type":"agent_message","text":"Actual response"}}
{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}"#;

        let resp = parse_codex_jsonl_fallback(jsonl).unwrap();
        assert_eq!(resp.text(), "Actual response");
    }

    #[test]
    fn test_parse_codex_jsonl_with_blank_lines() {
        let jsonl = r#"
{"type":"item.completed","item":{"type":"agent_message","text":"works"}}

{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}
"#;

        let resp = parse_codex_jsonl_fallback(jsonl).unwrap();
        assert_eq!(resp.text(), "works");
    }

    #[test]
    fn test_extract_codex_tool_start_for_command_execution() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'pwd'"}}"#,
        )
        .unwrap();

        let tool = extract_codex_tool_start(&json).unwrap();
        assert_eq!(tool.item_id, "item_1");
        assert_eq!(tool.tool_name, "command_execution");
        assert_eq!(tool.command.as_deref(), Some("/bin/zsh -lc 'pwd'"));
    }

    #[test]
    fn test_is_codex_tool_item_type_includes_command_execution() {
        assert!(is_codex_tool_item_type("command_execution"));
        assert!(is_codex_tool_item_type("function_call"));
        assert!(is_codex_tool_item_type("tool_call"));
        assert!(!is_codex_tool_item_type("agent_message"));
    }

    #[test]
    fn test_serialize_prompt_with_system() {
        use openfang_types::message::Message;

        let request = CompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
            }],
            tools: vec![],
            max_tokens: 1024,
            temperature: 0.0,
            system: Some("You are helpful.".to_string()),
            thinking: None,
            sentry_parent_span: None,
        };

        let prompt = serialize_prompt(&request);
        assert!(prompt.contains("[System]\nYou are helpful."));
        assert!(prompt.contains("User: Hello"));
    }

    #[test]
    fn test_serialize_prompt_no_system() {
        use openfang_types::message::Message;

        let request = CompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("Hi".to_string()),
                },
                Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("Hello!".to_string()),
                },
            ],
            tools: vec![],
            max_tokens: 1024,
            temperature: 0.0,
            system: None,
            thinking: None,
            sentry_parent_span: None,
        };

        let prompt = serialize_prompt(&request);
        assert!(!prompt.contains("[System]"));
        assert!(prompt.contains("User: Hi"));
        assert!(prompt.contains("Assistant: Hello!"));
    }

    #[test]
    fn test_selected_model_arg_skips_defaultish_models() {
        assert_eq!(selected_model_arg(""), None);
        assert_eq!(selected_model_arg("default"), None);
        assert_eq!(selected_model_arg("gpt-4o"), None);
        assert_eq!(selected_model_arg("gpt-4o-mini"), None);
        assert_eq!(selected_model_arg(" o3 "), Some("o3".to_string()));
    }

    #[test]
    fn test_is_request_format_error_patterns() {
        assert!(is_request_format_error(
            "Invalid request format. This may be a bug."
        ));
        assert!(is_request_format_error("missing field `id_token`"));
        assert!(is_request_format_error("Validation error: expected object"));
        assert!(!is_request_format_error("rate limit exceeded"));
    }

    #[test]
    fn test_filtered_stderr_removes_codex_warning_noise() {
        let stderr =
            "WARNING: proceeding, even though we could not update PATH\nmissing field id_token";
        assert_eq!(filtered_stderr(stderr), "missing field id_token");
    }

    #[test]
    fn test_extract_codex_failure_message_from_stdout_jsonl() {
        let stdout = r#"{"type":"error","message":"Invalid request format. This may be a bug."}"#;
        let msg = extract_codex_failure_message(stdout, "");
        assert_eq!(msg, "Invalid request format. This may be a bug.");
    }

    #[test]
    fn test_extract_codex_failure_message_prefers_stderr_when_present() {
        let stdout = r#"{"type":"error","message":"stdout error"}"#;
        let stderr = "validation error: bad schema";
        let msg = extract_codex_failure_message(stdout, stderr);
        assert_eq!(msg, "validation error: bad schema");
    }
}
