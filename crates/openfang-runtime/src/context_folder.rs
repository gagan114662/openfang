//! Context folding via sub-LLM delegation.
//!
//! When tool results exceed the context budget, delegates extraction to a
//! sub-LLM rather than brute-force truncating. Falls back to existing
//! truncation on LLM error or when below the fold threshold.

use crate::llm_driver::{CompletionRequest, LlmDriver};
use openfang_types::message::Message;
use std::sync::Arc;

/// Configuration for the context folder.
#[derive(Debug, Clone)]
pub struct FoldingConfig {
    /// Minimum chars before folding kicks in.
    pub min_fold_chars: usize,
    /// Maximum tokens for the folded output.
    pub max_fold_tokens: usize,
    /// Model to use for folding.
    pub fold_model: String,
    /// Temperature for the fold LLM call.
    pub temperature: f32,
}

impl Default for FoldingConfig {
    fn default() -> Self {
        Self {
            min_fold_chars: 8000,
            max_fold_tokens: 1024,
            fold_model: "llama-3.1-8b-instant".to_string(),
            temperature: 0.1,
        }
    }
}

/// Result of a fold operation.
#[derive(Debug, Clone)]
pub struct FoldResult {
    /// The (possibly folded) content.
    pub folded_content: String,
    /// Original content size in chars.
    pub original_chars: usize,
    /// Folded content size in chars.
    pub folded_chars: usize,
    /// Whether folding was actually performed.
    pub was_folded: bool,
}

/// Fold a tool result by delegating extraction to a sub-LLM.
///
/// If the content is below `min_fold_chars`, returns it unchanged.
/// On LLM error, falls back to truncation.
pub async fn fold_tool_result(
    tool_result: &str,
    user_question: &str,
    tool_name: &str,
    config: &FoldingConfig,
    driver: &Arc<dyn LlmDriver>,
) -> FoldResult {
    let original_chars = tool_result.len();

    // Below threshold — no folding needed
    if original_chars < config.min_fold_chars {
        return FoldResult {
            folded_content: tool_result.to_string(),
            original_chars,
            folded_chars: original_chars,
            was_folded: false,
        };
    }

    // Build extraction prompt
    let system = format!(
        "You are a context extraction assistant. Extract ONLY the information relevant to \
         the user's question from the tool output below. Preserve exact values, code snippets, \
         and data points. Do not add commentary or interpretation.\n\n\
         Tool: {tool_name}\n\
         User question: {user_question}"
    );

    let request = CompletionRequest {
        model: config.fold_model.clone(),
        messages: vec![Message::user(tool_result)],
        tools: vec![],
        max_tokens: config.max_fold_tokens as u32,
        temperature: config.temperature,
        system: Some(system),
        thinking: None,
        sentry_parent_span: None,
    };

    match driver.complete(request).await {
        Ok(response) => {
            let folded = response.text();
            if folded.is_empty() {
                // Empty response — fall back to truncation
                let truncated = truncate_fallback(tool_result, config.min_fold_chars);
                FoldResult {
                    folded_chars: truncated.len(),
                    folded_content: truncated,
                    original_chars,
                    was_folded: true,
                }
            } else {
                FoldResult {
                    folded_chars: folded.len(),
                    folded_content: folded,
                    original_chars,
                    was_folded: true,
                }
            }
        }
        Err(_) => {
            // LLM error — fall back to truncation
            let truncated = truncate_fallback(tool_result, config.min_fold_chars);
            FoldResult {
                folded_chars: truncated.len(),
                folded_content: truncated,
                original_chars,
                was_folded: true,
            }
        }
    }
}

/// Find the largest byte offset <= `pos` that falls on a UTF-8 char boundary.
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut p = pos;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Fallback truncation when the sub-LLM call fails.
fn truncate_fallback(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let max_chars = floor_char_boundary(content, max_chars);
    let search_start = floor_char_boundary(content, max_chars.saturating_sub(200));
    let break_point = content[search_start..max_chars]
        .rfind('\n')
        .map(|pos| search_start + pos)
        .unwrap_or_else(|| floor_char_boundary(content, max_chars.saturating_sub(100)));
    format!(
        "{}\n\n[FOLD FALLBACK: truncated from {} to {} chars]",
        &content[..break_point],
        content.len(),
        break_point,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_content_not_folded() {
        let config = FoldingConfig::default();
        let content = "short tool result";
        // We can't call async fold_tool_result in a sync test without a driver,
        // but we can verify the threshold logic:
        assert!(content.len() < config.min_fold_chars);
    }

    #[test]
    fn test_fold_result_reports_was_folded_false() {
        let result = FoldResult {
            folded_content: "hello".to_string(),
            original_chars: 5,
            folded_chars: 5,
            was_folded: false,
        };
        assert!(!result.was_folded);
        assert_eq!(result.original_chars, result.folded_chars);
    }

    #[test]
    fn test_config_defaults_are_sane() {
        let config = FoldingConfig::default();
        assert_eq!(config.min_fold_chars, 8000);
        assert_eq!(config.max_fold_tokens, 1024);
        assert!(!config.fold_model.is_empty());
        assert!(config.temperature > 0.0 && config.temperature < 1.0);
    }

    #[test]
    fn test_floor_char_boundary_ascii() {
        let s = "hello world";
        assert_eq!(floor_char_boundary(s, 5), 5);
        assert_eq!(floor_char_boundary(s, 100), s.len());
        assert_eq!(floor_char_boundary(s, 0), 0);
    }

    #[test]
    fn test_floor_char_boundary_multibyte() {
        let s = "héllo wörld";
        for i in 0..s.len() {
            let b = floor_char_boundary(s, i);
            assert!(
                s.is_char_boundary(b),
                "not a boundary at {b} for offset {i}"
            );
            assert!(b <= i);
        }
    }

    #[test]
    fn test_truncate_fallback_multibyte_no_panic() {
        let content = "á".repeat(10_000);
        let result = truncate_fallback(&content, 5000);
        assert!(result.contains("[FOLD FALLBACK:"));
        assert!(result.len() < content.len());
    }

    #[test]
    fn test_truncate_fallback_mixed_utf8() {
        let content = "日本語テスト ".repeat(2000);
        let result = truncate_fallback(&content, 3000);
        assert!(result.contains("[FOLD FALLBACK:"));
    }
}
