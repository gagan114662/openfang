//! Claude Code CLI subprocess driver.
//!
//! Spawns `claude -p --verbose --output-format stream-json` as a subprocess to
//! leverage Claude Code's built-in OAuth authentication. Parses the JSONL stream
//! to extract tool calls and create Sentry child spans with real timing.

use crate::llm_driver::{CompletionRequest, CompletionResponse, LlmDriver, LlmError};
use async_trait::async_trait;
use openfang_types::message::{ContentBlock, MessageContent, Role, StopReason, TokenUsage};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

/// Driver that delegates to the `claude` CLI binary (Claude Code).
///
/// Auth is handled by the CLI itself (OAuth session). No API key needed.
/// Uses `--verbose --output-format stream-json` to capture tool calls for Sentry tracing.
pub struct ClaudeCodeDriver;

#[async_trait]
impl LlmDriver for ClaudeCodeDriver {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let prompt = serialize_messages(&request);
        let requested_model = if request.model.trim().is_empty() {
            None
        } else {
            Some(request.model.trim().to_string())
        };

        let result = run_claude_streaming(
            &prompt,
            requested_model.as_deref(),
            request.system.as_deref(),
        )
        .await;

        // Guardrail: if claude rejects request format and we passed a model,
        // retry once without --model because aliases/versions may drift.
        if let Err(ref e) = result {
            if requested_model.is_some() && is_request_format_error(&e.to_string()) {
                warn!(
                    model = requested_model.as_deref().unwrap_or_default(),
                    error = %e,
                    "claude request format error with explicit model; retrying once without --model"
                );
                return run_claude_streaming(&prompt, None, request.system.as_deref()).await;
            }
        }

        result
    }
}

/// Spawn `claude -p --verbose --output-format stream-json`, read stdout
/// line-by-line, and create Sentry child spans for each tool call in real time.
async fn run_claude_streaming(
    prompt: &str,
    model: Option<&str>,
    system: Option<&str>,
) -> Result<CompletionResponse, LlmError> {
    // --verbose is required for stream-json with -p
    let mut args = vec![
        "-p".to_string(),
        "--verbose".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
    ];

    if let Some(model) = model {
        args.push("--model".to_string());
        args.push(model.to_string());
    }

    if let Some(system) = system {
        args.push("--system-prompt".to_string());
        args.push(system.to_string());
    }

    debug!(
        args = ?args,
        prompt_len = prompt.len(),
        "Spawning claude CLI subprocess (stream-json)"
    );

    let mut child = tokio::process::Command::new("claude")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| LlmError::Http(format!("Failed to spawn claude CLI: {e}")))?;

    // Write prompt to stdin, then drop to signal EOF
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| LlmError::Http(format!("Failed to write to claude stdin: {e}")))?;
    }

    // Take stdout for line-by-line reading
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| LlmError::Http("No stdout from claude CLI".to_string()))?;

    // Collect stderr in background
    let stderr_handle = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        if let Some(mut stderr) = stderr_handle {
            let _ = stderr.read_to_string(&mut buf).await;
        }
        buf
    });

    // Parse stream-json with timeout (5 min for long-running tool calls)
    let parse_result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        parse_stream_json(stdout, None),
    )
    .await;

    // Wait for child exit
    let status = child
        .wait()
        .await
        .map_err(|e| LlmError::Http(format!("claude CLI process error: {e}")))?;

    let stderr_text = stderr_task.await.unwrap_or_default();

    match parse_result {
        Ok(Ok(response)) => {
            if !status.success() {
                let failure =
                    extract_claude_failure_message_from_lines(&response.text(), &stderr_text);
                warn!(
                    exit_code = ?status.code(),
                    error = %failure,
                    "claude CLI failed (stream-json)"
                );
                return Err(LlmError::Api {
                    status: status.code().unwrap_or(1) as u16,
                    message: format!("claude CLI exited with error: {failure}"),
                });
            }
            Ok(response)
        }
        Ok(Err(e)) => {
            if !status.success() {
                let failure = extract_claude_failure_message_from_lines("", &stderr_text);
                return Err(LlmError::Api {
                    status: status.code().unwrap_or(1) as u16,
                    message: format!("claude CLI exited with error: {failure}"),
                });
            }
            Err(e)
        }
        Err(_) => {
            warn!("claude CLI subprocess timed out after 300s");
            Err(LlmError::Http(
                "claude CLI subprocess timed out after 300s".to_string(),
            ))
        }
    }
}

/// Parse JSONL stream from `claude -p --verbose --output-format stream-json`.
///
/// Creates Sentry child spans for each tool call with real timing.
async fn parse_stream_json(
    stdout: tokio::process::ChildStdout,
    parent_span: Option<&sentry::TransactionOrSpan>,
) -> Result<CompletionResponse, LlmError> {
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    let mut result_text = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut is_error = false;
    let mut tool_span_count: u32 = 0;

    // Track active tool spans by tool_use ID
    let mut active_tool_spans: HashMap<String, sentry::TransactionOrSpan> = HashMap::new();
    // Collect all stdout lines in case we need to fall back to single-JSON parsing
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
            // Assistant message — may contain tool_use content blocks
            "assistant" => {
                if let Some(content) = json.pointer("/message/content").and_then(|v| v.as_array()) {
                    for block in content {
                        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");

                        if block_type == "tool_use" {
                            let tool_id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let tool_name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");

                            debug!(
                                tool_name,
                                tool_id = tool_id.as_str(),
                                "claude subprocess: tool_use detected"
                            );

                            if let Some(parent) = parent_span {
                                let span = parent.start_child("tool.execute", tool_name);
                                span.set_data("tool.name", tool_name.into());
                                span.set_data("tool.id", tool_id.clone().into());
                                span.set_data("tool.source", "claude-code".into());

                                // Capture tool input (truncated for Sentry payload limits)
                                if let Some(input) = block.get("input") {
                                    let input_str = input.to_string();
                                    let truncated = if input_str.len() > 1024 {
                                        format!("{}...(truncated)", &input_str[..1024])
                                    } else {
                                        input_str
                                    };
                                    span.set_data("tool.input", truncated.into());
                                }

                                let span: sentry::TransactionOrSpan = span.into();
                                active_tool_spans.insert(tool_id, span);
                                tool_span_count += 1;
                            }
                        }

                        if block_type == "text" {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                if result_text.is_empty() {
                                    result_text = text.to_string();
                                }
                            }
                        }
                    }
                }
            }

            // Tool result — finish the corresponding span
            "tool_result" => {
                let tool_id = json
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                finish_tool_span(&mut active_tool_spans, tool_id, &json);
            }

            // Some Claude Code versions emit "user" messages with tool_result content
            "user" => {
                if let Some(content) = json.pointer("/message/content").and_then(|v| v.as_array()) {
                    for block in content {
                        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if block_type == "tool_result" {
                            let tool_id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            finish_tool_span(&mut active_tool_spans, tool_id, block);
                        }
                    }
                }
            }

            // Final result event — extract response and usage
            "result" => {
                result_text = json
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                input_tokens = json
                    .pointer("/usage/input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                output_tokens = json
                    .pointer("/usage/output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                is_error = json
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            }

            _ => {} // Ignore system, message_start, ping, etc.
        }
    }

    // Finish any orphaned tool spans
    for (id, span) in active_tool_spans.drain() {
        debug!(tool_id = id.as_str(), "Finishing orphaned tool span");
        span.set_status(sentry::protocol::SpanStatus::Aborted);
        span.finish();
    }

    debug!(
        result_len = result_text.len(),
        input_tokens, output_tokens, tool_span_count, "claude stream-json parsing complete"
    );

    // If no result event found, try falling back to single-JSON parsing
    if result_text.is_empty() && !all_lines.is_empty() {
        let combined = all_lines.join("\n");
        if let Ok(response) = parse_claude_json(&combined) {
            return Ok(response);
        }
    }

    if is_error {
        return Err(LlmError::Api {
            status: 0,
            message: format!("claude CLI error: {result_text}"),
        });
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

fn finish_tool_span(
    active_spans: &mut HashMap<String, sentry::TransactionOrSpan>,
    tool_id: &str,
    result_json: &serde_json::Value,
) {
    if let Some(span) = active_spans.remove(tool_id) {
        let tool_error = result_json
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if tool_error {
            span.set_status(sentry::protocol::SpanStatus::InternalError);
        } else {
            span.set_status(sentry::protocol::SpanStatus::Ok);
        }

        // Capture tool output (truncated for Sentry payload limits)
        if let Some(content) = result_json
            .get("content")
            .or_else(|| result_json.get("output"))
        {
            let output_str = content.to_string();
            let truncated = if output_str.len() > 1024 {
                format!("{}...(truncated)", &output_str[..1024])
            } else {
                output_str
            };
            span.set_data("tool.output", truncated.into());
        }
        span.set_data("tool.is_error", tool_error.into());

        debug!(tool_id, tool_error, "Finishing tool span");
        span.finish();
    }
}

fn parse_claude_json(raw: &str) -> Result<CompletionResponse, LlmError> {
    let json: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| LlmError::Parse(format!("Invalid JSON from claude CLI: {e}")))?;

    if json
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let msg = json
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown claude CLI error");
        return Err(LlmError::Api {
            status: 0,
            message: format!("claude CLI error: {msg}"),
        });
    }

    let result_text = json
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let input_tokens = json
        .pointer("/usage/input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let output_tokens = json
        .pointer("/usage/output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

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

fn is_request_format_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("invalid request format")
        || lower.contains("invalid request")
        || lower.contains("malformed")
        || lower.contains("missing field")
        || lower.contains("validation error")
        || lower.contains("schema")
}

fn filtered_stderr(stderr: &str) -> String {
    let all_lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    if all_lines.is_empty() {
        return "unknown claude CLI error (empty stderr)".to_string();
    }

    // Prefer non-warning lines (actual errors)
    let error_lines: Vec<&str> = all_lines
        .iter()
        .copied()
        .filter(|line| !line.starts_with("WARNING:"))
        .collect();

    if !error_lines.is_empty() {
        error_lines.join(" | ")
    } else {
        // All lines are warnings — include them rather than losing the info
        all_lines.join(" | ")
    }
}

fn extract_claude_failure_message_from_lines(stdout: &str, stderr: &str) -> String {
    // Try structured extraction from stdout first (most specific)
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(msg) = json
                .get("result")
                .or_else(|| json.get("message"))
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

    // Try single-JSON stdout
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        if let Some(msg) = json
            .get("result")
            .or_else(|| json.get("message"))
            .or_else(|| json.pointer("/error/message"))
            .and_then(|v| v.as_str())
        {
            let trimmed = msg.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    // Fall back to stderr (now includes warning lines instead of losing them)
    filtered_stderr(stderr)
}

fn serialize_messages(request: &CompletionRequest) -> String {
    let mut parts = Vec::new();

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

fn extract_text(content: &MessageContent) -> String {
    content.text_content()
}

pub fn is_available() -> bool {
    super::binary_on_path("claude")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_claude_json_full() {
        let json = r#"{
            "result": "Hello, world!",
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "total_cost_usd": 0.001
        }"#;
        let resp = parse_claude_json(json).unwrap();
        assert_eq!(resp.text(), "Hello, world!");
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_parse_claude_json_missing_usage() {
        let json = r#"{"result": "Just text"}"#;
        let resp = parse_claude_json(json).unwrap();
        assert_eq!(resp.text(), "Just text");
        assert_eq!(resp.usage.input_tokens, 0);
        assert_eq!(resp.usage.output_tokens, 0);
    }

    #[test]
    fn test_parse_claude_json_invalid() {
        let result = parse_claude_json("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_messages() {
        use openfang_types::message::Message;

        let request = CompletionRequest {
            model: "sonnet".to_string(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("Hello".to_string()),
                },
                Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("Hi there!".to_string()),
                },
                Message {
                    role: Role::User,
                    content: MessageContent::Text("How are you?".to_string()),
                },
            ],
            tools: vec![],
            max_tokens: 1024,
            temperature: 0.0,
            system: None,
            thinking: None,
            sentry_parent_span: None,
        };

        let prompt = serialize_messages(&request);
        assert!(prompt.contains("User: Hello"));
        assert!(prompt.contains("Assistant: Hi there!"));
        assert!(prompt.contains("User: How are you?"));
    }

    #[test]
    fn test_is_request_format_error_patterns() {
        assert!(is_request_format_error(
            "Invalid request format. This may be a bug."
        ));
        assert!(is_request_format_error("missing field `id_token`"));
        assert!(!is_request_format_error("rate limit exceeded"));
    }

    #[test]
    fn test_finish_tool_span_handles_missing_id() {
        let mut spans: HashMap<String, sentry::TransactionOrSpan> = HashMap::new();
        let json = serde_json::json!({"is_error": false});
        finish_tool_span(&mut spans, "nonexistent", &json);
        assert!(spans.is_empty());
    }
}
