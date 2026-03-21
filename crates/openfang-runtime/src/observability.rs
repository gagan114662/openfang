//! Agent-accessible structured log querying.
//!
//! Provides types and functions for agents to query JSON log files
//! from worktrees, enabling self-diagnosis and observability-driven
//! development without leaving the agent loop.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A single parsed log entry from a JSON log file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub fields: HashMap<String, serde_json::Value>,
}

/// Filters for querying log entries.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    pub level: Option<String>,
    pub target_pattern: Option<String>,
    pub since: Option<SystemTime>,
    pub until: Option<SystemTime>,
    pub keyword: Option<String>,
    pub limit: Option<usize>,
}

/// Configuration for the observability system.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    pub log_dir: PathBuf,
    pub max_entries: usize,
    pub retention_days: u32,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("log"),
            max_entries: 10_000,
            retention_days: 7,
        }
    }
}

/// Observer that reads JSON log files from a worktree directory.
pub struct WorktreeObserver {
    pub config: ObservabilityConfig,
}

impl WorktreeObserver {
    pub fn new(config: ObservabilityConfig) -> Self {
        Self { config }
    }

    pub fn log_dir(&self) -> &Path {
        &self.config.log_dir
    }
}

/// Parse a single JSON log line into a LogEntry.
pub fn parse_log_line(line: &str) -> Option<LogEntry> {
    serde_json::from_str(line).ok()
}

/// Filter log entries based on a query.
pub fn query_logs(entries: &[LogEntry], query: &LogQuery) -> Vec<LogEntry> {
    let mut results: Vec<LogEntry> = entries
        .iter()
        .filter(|e| {
            if let Some(ref level) = query.level {
                if !e.level.eq_ignore_ascii_case(level) {
                    return false;
                }
            }
            if let Some(ref pattern) = query.target_pattern {
                if !e.target.contains(pattern) {
                    return false;
                }
            }
            if let Some(ref keyword) = query.keyword {
                let kw_lower = keyword.to_lowercase();
                if !e.message.to_lowercase().contains(&kw_lower)
                    && !e.target.to_lowercase().contains(&kw_lower)
                {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect();

    if let Some(limit) = query.limit {
        results.truncate(limit);
    }

    results
}

/// Return the last N error-level entries.
pub fn recent_errors(entries: &[LogEntry], count: usize) -> Vec<LogEntry> {
    query_logs(
        entries,
        &LogQuery {
            level: Some("ERROR".to_string()),
            limit: Some(count),
            ..Default::default()
        },
    )
}

/// Build an observability section for prompt injection.
pub fn build_observability_section(errors: &[LogEntry]) -> String {
    if errors.is_empty() {
        return "## Observability\n\nNo recent errors.\n".to_string();
    }

    let mut section = String::new();
    section.push_str(&format!(
        "## Observability ({} recent errors)\n\n",
        errors.len()
    ));

    for entry in errors {
        section.push_str(&format!(
            "- **[{}]** `{}`: {}\n",
            entry.timestamp, entry.target, entry.message
        ));
    }

    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<LogEntry> {
        vec![
            LogEntry {
                timestamp: "2025-01-15T10:00:00Z".to_string(),
                level: "INFO".to_string(),
                target: "openfang_runtime::agent_loop".to_string(),
                message: "Agent loop started".to_string(),
                span_id: Some("span-1".to_string()),
                fields: HashMap::new(),
            },
            LogEntry {
                timestamp: "2025-01-15T10:00:01Z".to_string(),
                level: "ERROR".to_string(),
                target: "openfang_runtime::llm_driver".to_string(),
                message: "LLM request failed: timeout".to_string(),
                span_id: None,
                fields: HashMap::from([(
                    "model".to_string(),
                    serde_json::Value::String("gpt-4".to_string()),
                )]),
            },
            LogEntry {
                timestamp: "2025-01-15T10:00:02Z".to_string(),
                level: "WARN".to_string(),
                target: "openfang_runtime::tool_runner".to_string(),
                message: "Tool execution slow".to_string(),
                span_id: None,
                fields: HashMap::new(),
            },
            LogEntry {
                timestamp: "2025-01-15T10:00:03Z".to_string(),
                level: "ERROR".to_string(),
                target: "openfang_kernel::startup".to_string(),
                message: "Config parse error".to_string(),
                span_id: None,
                fields: HashMap::new(),
            },
        ]
    }

    #[test]
    fn test_parse_log_line_valid() {
        let line = r#"{"timestamp":"2025-01-15T10:00:00Z","level":"INFO","target":"test","message":"hello"}"#;
        let entry = parse_log_line(line).unwrap();
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.message, "hello");
    }

    #[test]
    fn test_parse_log_line_invalid() {
        assert!(parse_log_line("not json").is_none());
        assert!(parse_log_line("").is_none());
    }

    #[test]
    fn test_query_by_level() {
        let entries = sample_entries();
        let results = query_logs(
            &entries,
            &LogQuery {
                level: Some("ERROR".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.level == "ERROR"));
    }

    #[test]
    fn test_query_by_target_pattern() {
        let entries = sample_entries();
        let results = query_logs(
            &entries,
            &LogQuery {
                target_pattern: Some("agent_loop".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 1);
        assert!(results[0].target.contains("agent_loop"));
    }

    #[test]
    fn test_query_by_keyword() {
        let entries = sample_entries();
        let results = query_logs(
            &entries,
            &LogQuery {
                keyword: Some("timeout".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 1);
        assert!(results[0].message.contains("timeout"));
    }

    #[test]
    fn test_query_with_limit() {
        let entries = sample_entries();
        let results = query_logs(
            &entries,
            &LogQuery {
                limit: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_query_combined_filters() {
        let entries = sample_entries();
        let results = query_logs(
            &entries,
            &LogQuery {
                level: Some("ERROR".to_string()),
                keyword: Some("timeout".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "LLM request failed: timeout");
    }

    #[test]
    fn test_recent_errors() {
        let entries = sample_entries();
        let errors = recent_errors(&entries, 10);
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn test_recent_errors_limited() {
        let entries = sample_entries();
        let errors = recent_errors(&entries, 1);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_build_observability_section_empty() {
        let section = build_observability_section(&[]);
        assert!(section.contains("No recent errors"));
    }

    #[test]
    fn test_build_observability_section_with_errors() {
        let entries = sample_entries();
        let errors = recent_errors(&entries, 10);
        let section = build_observability_section(&errors);
        assert!(section.contains("2 recent errors"));
        assert!(section.contains("timeout"));
        assert!(section.contains("Config parse error"));
    }

    #[test]
    fn test_config_defaults() {
        let config = ObservabilityConfig::default();
        assert_eq!(config.max_entries, 10_000);
        assert_eq!(config.retention_days, 7);
    }

    #[test]
    fn test_worktree_observer_log_dir() {
        let config = ObservabilityConfig {
            log_dir: PathBuf::from("/tmp/logs"),
            ..Default::default()
        };
        let observer = WorktreeObserver::new(config);
        assert_eq!(observer.log_dir(), Path::new("/tmp/logs"));
    }

    #[test]
    fn test_keyword_case_insensitive() {
        let entries = sample_entries();
        let results = query_logs(
            &entries,
            &LogQuery {
                keyword: Some("TIMEOUT".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 1);
    }
}
