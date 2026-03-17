//! evlog — wide-event logging for OpenFang API requests.
//!
//! Accumulates context throughout a request lifecycle and emits a single
//! comprehensive event at completion, to both Sentry and structured stdout.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Per-request wide-event accumulator.
///
/// Handlers call `set()` to attach context incrementally.  The middleware
/// emits the final wide event when the response completes.
#[derive(Clone, Debug)]
pub struct EvLog {
    inner: Arc<Mutex<EvLogInner>>,
}

#[derive(Debug)]
struct EvLogInner {
    fields: Map<String, Value>,
    errors: Vec<EvError>,
    spans: Vec<EvSpanRecord>,
    start: Instant,
}

/// A structured error with evlog's `why` / `fix` fields.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EvError {
    pub message: String,
    pub why: String,
    pub fix: String,
}

/// Completed span record for child operations (LLM calls, tool invocations).
#[derive(Debug, Clone, serde::Serialize)]
pub struct EvSpanRecord {
    pub name: String,
    pub duration_ms: u64,
}

/// RAII guard — records span duration on drop.
pub struct EvSpanGuard {
    inner: Arc<Mutex<EvLogInner>>,
    name: String,
    start: Instant,
}

impl Drop for EvSpanGuard {
    fn drop(&mut self) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        if let Ok(mut inner) = self.inner.lock() {
            inner.spans.push(EvSpanRecord {
                name: std::mem::take(&mut self.name),
                duration_ms,
            });
        }
    }
}

impl EvLog {
    /// Create a new wide-event accumulator.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EvLogInner {
                fields: Map::new(),
                errors: Vec::new(),
                spans: Vec::new(),
                start: Instant::now(),
            })),
        }
    }

    /// Accumulate a key-value pair into the wide event.
    ///
    /// If `value` is an object, its fields are merged into the existing object
    /// at `key` (if any).  Otherwise the value is set directly.
    pub fn set(&self, key: impl Into<String>, value: Value) {
        if let Ok(mut inner) = self.inner.lock() {
            let key = key.into();
            if let Value::Object(new_map) = &value {
                if let Some(Value::Object(existing)) = inner.fields.get_mut(&key) {
                    for (k, v) in new_map {
                        existing.insert(k.clone(), v.clone());
                    }
                    return;
                }
            }
            inner.fields.insert(key, value);
        }
    }

    /// Record a structured error with `why` and `fix` fields.
    pub fn error(
        &self,
        message: impl Into<String>,
        why: impl Into<String>,
        fix: impl Into<String>,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.errors.push(EvError {
                message: message.into(),
                why: why.into(),
                fix: fix.into(),
            });
        }
    }

    /// Start a child span.  Duration is recorded when the guard is dropped.
    pub fn span(&self, name: impl Into<String>) -> EvSpanGuard {
        EvSpanGuard {
            inner: Arc::clone(&self.inner),
            name: name.into(),
            start: Instant::now(),
        }
    }

    /// Consume the accumulator and build the final wide-event JSON object.
    ///
    /// The caller (middleware) supplies the HTTP-level fields; this method
    /// merges in everything the handlers accumulated.
    pub fn finalize(&self) -> WideEvent {
        let inner = self.inner.lock().expect("evlog lock poisoned");
        WideEvent {
            fields: inner.fields.clone(),
            errors: inner.errors.clone(),
            spans: inner.spans.clone(),
            elapsed_ms: inner.start.elapsed().as_millis() as u64,
        }
    }
}

impl Default for EvLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum extractor — pulls the EvLog from request extensions.
///
/// The `request_logging` middleware injects an EvLog before calling the handler.
/// If no EvLog is present (e.g. in tests), a fresh one is created.
impl<S: Send + Sync> FromRequestParts<S> for EvLog {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<EvLog>().cloned().unwrap_or_default())
    }
}

/// The finalized wide event ready for emission.
pub struct WideEvent {
    pub fields: Map<String, Value>,
    pub errors: Vec<EvError>,
    pub spans: Vec<EvSpanRecord>,
    pub elapsed_ms: u64,
}

impl WideEvent {
    /// Merge into a single JSON object for stdout emission.
    pub fn to_json(&self, base: Map<String, Value>) -> Value {
        let mut out = base;

        for (k, v) in &self.fields {
            out.insert(k.clone(), v.clone());
        }

        if !self.errors.is_empty() {
            out.insert(
                "errors".into(),
                serde_json::to_value(&self.errors).unwrap_or_default(),
            );
        }

        if !self.spans.is_empty() {
            out.insert(
                "spans".into(),
                serde_json::to_value(&self.spans).unwrap_or_default(),
            );
        }

        Value::Object(out)
    }
}

/// In-memory ring buffer for API request wide events.
///
/// Holds the last N request entries so the dashboard can display them
/// in real time via WS topic `"requests"` or SSE `/api/requests/stream`.
pub struct WideEventLog {
    entries: std::sync::Mutex<Vec<WideEventEntry>>,
    next_seq: std::sync::Mutex<u64>,
}

/// A single API request log entry for the dashboard.
#[derive(Clone, serde::Serialize)]
pub struct WideEventEntry {
    pub seq: u64,
    pub timestamp: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub request_id: String,
    pub error_count: usize,
    pub span_count: usize,
}

impl Default for WideEventLog {
    fn default() -> Self {
        Self::new()
    }
}

impl WideEventLog {
    pub fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
            next_seq: std::sync::Mutex::new(1),
        }
    }

    /// Push an entry into the ring buffer, capping at 500 entries.
    pub fn append(&self, mut entry: WideEventEntry) {
        let seq = {
            let mut s = self.next_seq.lock().expect("WideEventLog seq lock");
            let v = *s;
            *s += 1;
            v
        };
        entry.seq = seq;
        let mut entries = self.entries.lock().expect("WideEventLog entries lock");
        entries.push(entry);
        let len = entries.len();
        if len > 500 {
            entries.drain(..len - 500);
        }
    }

    /// Return the last `n` entries (oldest first).
    pub fn recent(&self, n: usize) -> Vec<WideEventEntry> {
        let entries = self.entries.lock().expect("WideEventLog entries lock");
        let start = entries.len().saturating_sub(n);
        entries[start..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_set_and_finalize() {
        let log = EvLog::new();
        log.set("user_id", json!("u123"));
        log.set("agent", json!({"id": "a1", "name": "helper"}));
        let event = log.finalize();
        assert_eq!(event.fields["user_id"], json!("u123"));
        assert_eq!(event.fields["agent"]["id"], json!("a1"));
    }

    #[test]
    fn test_set_merges_objects() {
        let log = EvLog::new();
        log.set("llm", json!({"provider": "groq"}));
        log.set("llm", json!({"tokens_in": 100}));
        let event = log.finalize();
        assert_eq!(event.fields["llm"]["provider"], json!("groq"));
        assert_eq!(event.fields["llm"]["tokens_in"], json!(100));
    }

    #[test]
    fn test_error_recording() {
        let log = EvLog::new();
        log.error("Payment failed", "card_declined", "Try a different card");
        let event = log.finalize();
        assert_eq!(event.errors.len(), 1);
        assert_eq!(event.errors[0].why, "card_declined");
        assert_eq!(event.errors[0].fix, "Try a different card");
    }

    #[test]
    fn test_span_records_duration() {
        let log = EvLog::new();
        {
            let _guard = log.span("llm_call");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let event = log.finalize();
        assert_eq!(event.spans.len(), 1);
        assert_eq!(event.spans[0].name, "llm_call");
        assert!(event.spans[0].duration_ms >= 5);
    }

    #[test]
    fn test_wide_event_to_json() {
        let log = EvLog::new();
        log.set("agent_id", json!("a1"));
        log.error("fail", "reason", "fix it");
        let event = log.finalize();

        let mut base = Map::new();
        base.insert("method".into(), json!("POST"));
        base.insert("path".into(), json!("/api/test"));

        let output = event.to_json(base);
        assert_eq!(output["method"], json!("POST"));
        assert_eq!(output["agent_id"], json!("a1"));
        assert!(output["errors"].is_array());
    }

    #[test]
    fn test_clone_shares_state() {
        let log = EvLog::new();
        let log2 = log.clone();
        log.set("key", json!("from_original"));
        log2.set("key2", json!("from_clone"));
        let event = log.finalize();
        assert_eq!(event.fields["key"], json!("from_original"));
        assert_eq!(event.fields["key2"], json!("from_clone"));
    }

    #[test]
    fn test_wide_event_log_append_and_recent() {
        let log = WideEventLog::new();
        for i in 0..5 {
            log.append(WideEventEntry {
                seq: 0,
                timestamp: format!("2026-01-01T00:00:0{i}Z"),
                method: "GET".into(),
                path: format!("/api/test/{i}"),
                status: 200,
                duration_ms: i as u64,
                request_id: format!("req-{i}"),
                error_count: 0,
                span_count: 0,
            });
        }
        let recent = log.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].seq, 3);
        assert_eq!(recent[2].seq, 5);
    }

    #[test]
    fn test_wide_event_log_caps_at_500() {
        let log = WideEventLog::new();
        for i in 0..510 {
            log.append(WideEventEntry {
                seq: 0,
                timestamp: String::new(),
                method: "GET".into(),
                path: format!("/{i}"),
                status: 200,
                duration_ms: 0,
                request_id: String::new(),
                error_count: 0,
                span_count: 0,
            });
        }
        let all = log.recent(1000);
        assert_eq!(all.len(), 500);
        assert_eq!(all[0].seq, 11);
    }
}
