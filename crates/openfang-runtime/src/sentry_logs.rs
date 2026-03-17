use chrono::Utc;
use openfang_memory::facts::{ArtifactIndexStore, FactEventStore};
use openfang_types::facts::{
    ArtifactRecord, CanonicalAgentRef, CanonicalArtifactRefs, CanonicalChannelRef, CanonicalCost,
    CanonicalEvent, CanonicalEventId, CanonicalModelRef, CanonicalRef,
    CANONICAL_EVENT_SCHEMA_VERSION,
};
use rusqlite::Connection;
use sentry::protocol::Value as SentryValue;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

pub const DEFAULT_MAX_ATTRIBUTE_BYTES: usize = 16 * 1024;
pub const DEFAULT_MAX_PAYLOAD_BYTES: usize = 512 * 1024;
const PAYLOAD_MARKER_RESERVE_BYTES: usize = 1024;
const MAX_TRUNCATED_FIELDS_REPORTED: usize = 64;
const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone)]
pub struct StructuredLogSettings {
    pub enable_logs: bool,
    pub realtime_log_flush: bool,
    pub realtime_log_flush_timeout_ms: u64,
    pub max_attribute_bytes: usize,
    pub max_payload_bytes: usize,
    pub include_prompts: bool,
    pub claude_capture_payloads: bool,
    pub mcp_capture_payloads: bool,
}

impl Default for StructuredLogSettings {
    fn default() -> Self {
        Self {
            enable_logs: true,
            realtime_log_flush: false,
            realtime_log_flush_timeout_ms: 1500,
            max_attribute_bytes: DEFAULT_MAX_ATTRIBUTE_BYTES,
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
            include_prompts: false,
            claude_capture_payloads: true,
            mcp_capture_payloads: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EventContext {
    pub trace_id: Option<String>,
    pub request_id: Option<String>,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub channel_kind: Option<String>,
    pub channel_user_id: Option<String>,
}

static SETTINGS: OnceLock<RwLock<StructuredLogSettings>> = OnceLock::new();
static FACT_STORE_CONN: OnceLock<RwLock<Option<Arc<Mutex<Connection>>>>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct GuardedLogAttributes {
    pub json_attributes: BTreeMap<String, JsonValue>,
    pub attributes: BTreeMap<String, SentryValue>,
    pub truncated_fields: Vec<String>,
    pub dropped_fields: usize,
    pub serialized_bytes: usize,
}

tokio::task_local! {
    static EVENT_CONTEXT: EventContext;
}

fn settings_lock() -> &'static RwLock<StructuredLogSettings> {
    SETTINGS.get_or_init(|| RwLock::new(StructuredLogSettings::default()))
}

fn fact_store_lock() -> &'static RwLock<Option<Arc<Mutex<Connection>>>> {
    FACT_STORE_CONN.get_or_init(|| RwLock::new(None))
}

pub fn configure(settings: &openfang_types::config::SentryConfig) {
    if let Ok(mut guard) = settings_lock().write() {
        *guard = StructuredLogSettings {
            enable_logs: settings.enable_logs,
            realtime_log_flush: settings.realtime_log_flush,
            realtime_log_flush_timeout_ms: settings.realtime_log_flush_timeout_ms,
            max_attribute_bytes: settings.wide_event_attribute_max_bytes,
            max_payload_bytes: settings.wide_event_payload_max_bytes,
            include_prompts: settings.include_prompts,
            claude_capture_payloads: settings.claude_capture_payloads,
            mcp_capture_payloads: settings.mcp_capture_payloads,
        };
    }
}

pub fn configure_fact_store(conn: Arc<Mutex<Connection>>) {
    if let Ok(mut guard) = fact_store_lock().write() {
        *guard = Some(conn);
    }
}

pub fn current_event_context() -> Option<EventContext> {
    EVENT_CONTEXT.try_with(|ctx| ctx.clone()).ok()
}

pub async fn scope_event_context<F, T>(next: EventContext, future: F) -> T
where
    F: Future<Output = T>,
{
    let mut merged = current_event_context().unwrap_or_default();
    if next.trace_id.is_some() {
        merged.trace_id = next.trace_id;
    }
    if next.request_id.is_some() {
        merged.request_id = next.request_id;
    }
    if next.run_id.is_some() {
        merged.run_id = next.run_id;
    }
    if next.session_id.is_some() {
        merged.session_id = next.session_id;
    }
    if next.agent_id.is_some() {
        merged.agent_id = next.agent_id;
    }
    if next.agent_name.is_some() {
        merged.agent_name = next.agent_name;
    }
    if next.channel_kind.is_some() {
        merged.channel_kind = next.channel_kind;
    }
    if next.channel_user_id.is_some() {
        merged.channel_user_id = next.channel_user_id;
    }
    EVENT_CONTEXT.scope(merged, future).await
}

fn current_settings() -> StructuredLogSettings {
    settings_lock()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

pub fn provider_family(provider: &str) -> String {
    let normalized = provider.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return "unknown".to_string();
    }
    if normalized == "codex-cli" || normalized.starts_with("codex") {
        return "codex".to_string();
    }
    if normalized.contains("claude") || normalized.contains("anthropic") {
        return "claude".to_string();
    }
    if normalized.contains("gemini") || normalized.contains("google") {
        return "gemini".to_string();
    }
    if normalized.contains("openai") || normalized.starts_with("gpt") {
        return "openai".to_string();
    }
    if normalized.contains("groq") {
        return "groq".to_string();
    }
    normalized
}

pub fn flatten_json(prefix: &str, value: &JsonValue, out: &mut BTreeMap<String, JsonValue>) {
    match value {
        JsonValue::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_json(&key, v, out);
            }
        }
        JsonValue::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                let key = if prefix.is_empty() {
                    idx.to_string()
                } else {
                    format!("{prefix}.{idx}")
                };
                flatten_json(&key, item, out);
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.insert(prefix.to_string(), value.clone());
            }
        }
    }
}

pub fn flatten_with_prefix(prefix: &str, value: &JsonValue) -> BTreeMap<String, JsonValue> {
    let mut out = BTreeMap::new();
    flatten_json(prefix, value, &mut out);
    out
}

fn insert_tag_if_present(tags: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            tags.insert(key.to_string(), trimmed.to_string());
        }
    }
}

fn truncate_to_bytes(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    let mut end = 0usize;
    for (idx, ch) in input.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    input[..end].to_string()
}

fn sentry_value_from_json(value: JsonValue) -> SentryValue {
    match value {
        JsonValue::Null => SentryValue::Null,
        JsonValue::Bool(v) => SentryValue::Bool(v),
        JsonValue::Number(v) => SentryValue::Number(v),
        JsonValue::String(v) => SentryValue::String(v),
        JsonValue::Array(values) => {
            SentryValue::Array(values.into_iter().map(sentry_value_from_json).collect())
        }
        JsonValue::Object(values) => SentryValue::Object(
            values
                .into_iter()
                .map(|(k, v)| (k, sentry_value_from_json(v)))
                .collect(),
        ),
    }
}

fn approximate_attr_size(key: &str, value: &JsonValue) -> usize {
    let payload_size = serde_json::to_vec(value).map(|v| v.len()).unwrap_or(0);
    key.len() + payload_size + 6
}

fn should_redact_secret_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "authorization",
        "cookie",
        "api_key",
        "apikey",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

fn should_redact_prompt_field(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    [
        "payload.input.user_message",
        "payload.output.response",
        "payload.prompt",
        "payload.prompts",
        "payload.completion",
        "payload.completions",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

fn should_drop_payload_for_event(
    key: &str,
    event_kind: &str,
    settings: &StructuredLogSettings,
) -> bool {
    if !key.starts_with("payload.") {
        return false;
    }
    if event_kind.starts_with("claude.") && !settings.claude_capture_payloads {
        return true;
    }
    if event_kind.starts_with("mcp.") && !settings.mcp_capture_payloads {
        return true;
    }
    false
}

fn sanitize_attributes(
    attributes: BTreeMap<String, JsonValue>,
    settings: &StructuredLogSettings,
) -> BTreeMap<String, JsonValue> {
    let event_kind = attributes
        .get("event.kind")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let mut sanitized = BTreeMap::new();

    for (key, value) in attributes {
        if should_drop_payload_for_event(&key, &event_kind, settings) {
            continue;
        }

        let sanitized_value = if should_redact_secret_key(&key)
            || (!settings.include_prompts && should_redact_prompt_field(&key))
        {
            JsonValue::String(REDACTED.to_string())
        } else {
            value
        };

        sanitized.insert(key, sanitized_value);
    }

    sanitized
}

pub fn build_guarded_log_attributes(
    mut attributes: BTreeMap<String, JsonValue>,
    max_attribute_bytes: usize,
    max_payload_bytes: usize,
) -> GuardedLogAttributes {
    let mut truncated = BTreeSet::new();

    for (k, v) in &mut attributes {
        if let JsonValue::String(text) = v {
            if text.len() > max_attribute_bytes {
                *text = truncate_to_bytes(text, max_attribute_bytes);
                truncated.insert(k.clone());
            }
        }
    }

    let budget = max_payload_bytes.saturating_sub(PAYLOAD_MARKER_RESERVE_BYTES);
    let mut kept: BTreeMap<String, JsonValue> = BTreeMap::new();
    let mut used = 0usize;
    let mut dropped_fields = 0usize;

    for (k, v) in attributes {
        let size = approximate_attr_size(&k, &v);
        if used.saturating_add(size) > budget {
            dropped_fields += 1;
            truncated.insert(k);
            continue;
        }
        used = used.saturating_add(size);
        kept.insert(k, v);
    }

    if !truncated.is_empty() {
        kept.insert("payload.truncated".to_string(), JsonValue::Bool(true));
        kept.insert(
            "payload.truncated_field_count".to_string(),
            JsonValue::from(truncated.len() as u64),
        );
        let truncated_list: Vec<JsonValue> = truncated
            .iter()
            .take(MAX_TRUNCATED_FIELDS_REPORTED)
            .map(|k| JsonValue::String(k.clone()))
            .collect();
        kept.insert(
            "payload.truncated_fields".to_string(),
            JsonValue::Array(truncated_list),
        );
        if truncated.len() > MAX_TRUNCATED_FIELDS_REPORTED {
            kept.insert(
                "payload.truncated_fields_omitted".to_string(),
                JsonValue::from((truncated.len() - MAX_TRUNCATED_FIELDS_REPORTED) as u64),
            );
        }
    }

    let serialized_bytes = kept
        .iter()
        .map(|(k, v)| approximate_attr_size(k, v))
        .sum::<usize>();

    let sentry_attrs = kept
        .iter()
        .map(|(k, v)| (k.clone(), sentry_value_from_json(v.clone())))
        .collect::<BTreeMap<_, _>>();

    GuardedLogAttributes {
        json_attributes: kept,
        attributes: sentry_attrs,
        truncated_fields: truncated.into_iter().collect(),
        dropped_fields,
        serialized_bytes,
    }
}

fn insert_if_missing(attrs: &mut BTreeMap<String, JsonValue>, key: &str, value: Option<String>) {
    if attrs.contains_key(key) {
        return;
    }
    if let Some(value) = value {
        attrs.insert(key.to_string(), JsonValue::String(value));
    }
}

fn enrich_with_context(mut attributes: BTreeMap<String, JsonValue>) -> BTreeMap<String, JsonValue> {
    let context = current_event_context().unwrap_or_default();
    insert_if_missing(&mut attributes, "trace.id", context.trace_id.clone());
    insert_if_missing(&mut attributes, "request.id", context.request_id.clone());
    insert_if_missing(
        &mut attributes,
        "run.id",
        context
            .run_id
            .clone()
            .or_else(|| context.request_id.clone()),
    );
    insert_if_missing(&mut attributes, "session.id", context.session_id);
    insert_if_missing(&mut attributes, "agent.id", context.agent_id);
    insert_if_missing(&mut attributes, "agent.name", context.agent_name);
    insert_if_missing(&mut attributes, "channel.kind", context.channel_kind);
    insert_if_missing(&mut attributes, "channel.user_id", context.channel_user_id);
    if !attributes.contains_key("occurred_at") {
        attributes.insert(
            "occurred_at".to_string(),
            JsonValue::String(Utc::now().to_rfc3339()),
        );
    }
    if !attributes.contains_key("event.id") {
        attributes.insert(
            "event.id".to_string(),
            JsonValue::String(uuid::Uuid::new_v4().to_string()),
        );
    }
    attributes
}

fn attr_string(attributes: &BTreeMap<String, JsonValue>, key: &str) -> Option<String> {
    attributes
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
}

fn attr_u64(attributes: &BTreeMap<String, JsonValue>, key: &str) -> Option<u64> {
    attributes.get(key).and_then(JsonValue::as_u64)
}

fn attr_f64(attributes: &BTreeMap<String, JsonValue>, key: &str) -> Option<f64> {
    attributes.get(key).and_then(JsonValue::as_f64)
}

fn indexed_tag_values(attributes: &BTreeMap<String, JsonValue>) -> BTreeMap<String, String> {
    let mut tags = BTreeMap::new();
    insert_tag_if_present(
        &mut tags,
        "event.kind",
        attr_string(attributes, "event.kind"),
    );
    insert_tag_if_present(
        &mut tags,
        "event.category",
        attr_string(attributes, "event.category"),
    );
    insert_tag_if_present(&mut tags, "event.id", attr_string(attributes, "event.id"));
    insert_tag_if_present(
        &mut tags,
        "request.id",
        attr_string(attributes, "request.id"),
    );
    insert_tag_if_present(&mut tags, "run.id", attr_string(attributes, "run.id"));
    insert_tag_if_present(&mut tags, "trace.id", attr_string(attributes, "trace.id"));
    insert_tag_if_present(
        &mut tags,
        "session.id",
        attr_string(attributes, "session.id"),
    );
    insert_tag_if_present(&mut tags, "agent.id", attr_string(attributes, "agent.id"));
    insert_tag_if_present(
        &mut tags,
        "agent.name",
        attr_string(attributes, "agent.name"),
    );
    insert_tag_if_present(&mut tags, "tool.name", attr_string(attributes, "tool.name"));
    insert_tag_if_present(
        &mut tags,
        "channel.kind",
        attr_string(attributes, "channel.kind"),
    );
    insert_tag_if_present(&mut tags, "outcome", attr_string(attributes, "outcome"));

    let provider =
        attr_string(attributes, "model.provider").or_else(|| attr_string(attributes, "provider"));
    insert_tag_if_present(&mut tags, "provider", provider.clone());
    insert_tag_if_present(
        &mut tags,
        "provider.family",
        provider.map(|provider| provider_family(&provider)),
    );

    // Git / worktree / contract context — searchable in Sentry Issues view
    insert_tag_if_present(
        &mut tags,
        "git.branch",
        attr_string(attributes, "git.branch"),
    );
    insert_tag_if_present(
        &mut tags,
        "worktree.agent",
        attr_string(attributes, "worktree.agent"),
    );
    insert_tag_if_present(
        &mut tags,
        "worktree.task",
        attr_string(attributes, "worktree.task"),
    );
    insert_tag_if_present(
        &mut tags,
        "contract.file",
        attr_string(attributes, "contract.file"),
    );
    insert_tag_if_present(
        &mut tags,
        "contract.id",
        attr_string(attributes, "contract.id"),
    );

    tags
}

fn artifact_ids(attributes: &BTreeMap<String, JsonValue>) -> Vec<String> {
    if let Some(JsonValue::Array(values)) = attributes.get("artifact.ids") {
        return values
            .iter()
            .filter_map(JsonValue::as_str)
            .map(ToString::to_string)
            .collect();
    }
    attr_string(attributes, "artifact.id")
        .map(|value| vec![value])
        .unwrap_or_default()
}

fn insert_payload_path(
    root: &mut serde_json::Map<String, JsonValue>,
    path: &str,
    value: JsonValue,
) {
    let parts = path.split('.').collect::<Vec<_>>();
    insert_payload_parts(root, &parts, value);
}

fn insert_payload_parts(
    root: &mut serde_json::Map<String, JsonValue>,
    parts: &[&str],
    value: JsonValue,
) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        root.insert(parts[0].to_string(), value);
        return;
    }
    let entry = root
        .entry(parts[0].to_string())
        .or_insert_with(|| JsonValue::Object(Default::default()));
    if !entry.is_object() {
        *entry = JsonValue::Object(Default::default());
    }
    if let Some(map) = entry.as_object_mut() {
        insert_payload_parts(map, &parts[1..], value);
    }
}

fn collect_payload(attributes: &BTreeMap<String, JsonValue>, body: &str) -> JsonValue {
    let mut payload = serde_json::Map::new();
    for (key, value) in attributes {
        if let Some(rest) = key.strip_prefix("payload.") {
            insert_payload_path(&mut payload, rest, value.clone());
        }
    }
    if !body.is_empty() {
        payload
            .entry("_body".to_string())
            .or_insert_with(|| JsonValue::String(body.to_string()));
    }
    JsonValue::Object(payload)
}

fn canonical_event_from_attributes(
    body: &str,
    attributes: &BTreeMap<String, JsonValue>,
) -> CanonicalEvent {
    CanonicalEvent {
        schema_version: CANONICAL_EVENT_SCHEMA_VERSION,
        event: CanonicalEventId {
            id: attr_string(attributes, "event.id")
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            kind: attr_string(attributes, "event.kind").unwrap_or_else(|| "unknown".to_string()),
        },
        occurred_at: attr_string(attributes, "occurred_at")
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        trace: CanonicalRef {
            id: attr_string(attributes, "trace.id"),
        },
        request: CanonicalRef {
            id: attr_string(attributes, "request.id"),
        },
        run: CanonicalRef {
            id: attr_string(attributes, "run.id"),
        },
        session: CanonicalRef {
            id: attr_string(attributes, "session.id"),
        },
        agent: CanonicalAgentRef {
            id: attr_string(attributes, "agent.id"),
            name: attr_string(attributes, "agent.name"),
        },
        channel: CanonicalChannelRef {
            kind: attr_string(attributes, "channel.kind"),
            user_id: attr_string(attributes, "channel.user_id"),
        },
        artifact: CanonicalArtifactRefs {
            ids: artifact_ids(attributes),
        },
        outcome: attr_string(attributes, "outcome"),
        duration_ms: attr_u64(attributes, "duration_ms"),
        cost: CanonicalCost {
            usd: attr_f64(attributes, "cost.usd").or_else(|| attr_f64(attributes, "cost_usd")),
        },
        model: CanonicalModelRef {
            provider: attr_string(attributes, "model.provider")
                .or_else(|| attr_string(attributes, "provider")),
            name: attr_string(attributes, "model.name")
                .or_else(|| attr_string(attributes, "model")),
        },
        payload: collect_payload(attributes, body),
    }
}

fn persist_canonical_event(event: &CanonicalEvent) {
    let Some(conn) = fact_store_lock()
        .read()
        .ok()
        .and_then(|guard| guard.clone())
    else {
        return;
    };
    if let Err(error) = FactEventStore::new(conn).record(event) {
        tracing::warn!(
            %error,
            event_id = %event.event.id,
            event_kind = %event.event.kind,
            "Failed to persist canonical fact event"
        );
    }
}

fn flush_sentry_logs(timeout_ms: u64) {
    let timeout = Duration::from_millis(timeout_ms.max(1));
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(timeout));
    }
}

pub fn capture_structured_log(
    level: sentry::Level,
    body: impl Into<String>,
    attributes: BTreeMap<String, JsonValue>,
) -> GuardedLogAttributes {
    let settings = current_settings();
    let body = body.into();
    let enriched = enrich_with_context(attributes);
    let sanitized = sanitize_attributes(enriched, &settings);
    let guarded = build_guarded_log_attributes(
        sanitized,
        settings.max_attribute_bytes,
        settings.max_payload_bytes,
    );

    let event = canonical_event_from_attributes(&body, &guarded.json_attributes);
    persist_canonical_event(&event);

    if settings.enable_logs {
        sentry::with_scope(
            |scope| {
                for (key, value) in indexed_tag_values(&guarded.json_attributes) {
                    scope.set_tag(&key, value);
                }
                for (key, value) in guarded.attributes.clone() {
                    scope.set_extra(&key, value);
                }
            },
            || {
                sentry::capture_message(&body, level);
            },
        );
        if settings.realtime_log_flush {
            flush_sentry_logs(settings.realtime_log_flush_timeout_ms);
        }
    }

    guarded
}

pub fn record_artifact(record: ArtifactRecord) {
    let Some(conn) = fact_store_lock()
        .read()
        .ok()
        .and_then(|guard| guard.clone())
    else {
        return;
    };
    if let Err(error) = ArtifactIndexStore::new(conn).upsert(&record) {
        tracing::warn!(%error, artifact_id = %record.artifact_id, "Failed to persist artifact index row");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_flatten_json_nested_and_arrays() {
        let input = serde_json::json!({
            "agent": {"id": "a1", "meta": {"provider": "groq"}},
            "tools": [{"name": "web"}, {"name": "fs"}],
            "status": 200
        });

        let out = flatten_with_prefix("payload", &input);
        assert_eq!(out.get("payload.agent.id"), Some(&serde_json::json!("a1")));
        assert_eq!(
            out.get("payload.agent.meta.provider"),
            Some(&serde_json::json!("groq"))
        );
        assert_eq!(
            out.get("payload.tools.0.name"),
            Some(&serde_json::json!("web"))
        );
        assert_eq!(
            out.get("payload.tools.1.name"),
            Some(&serde_json::json!("fs"))
        );
        assert_eq!(out.get("payload.status"), Some(&serde_json::json!(200)));
    }

    #[test]
    fn test_guard_truncates_oversized_field_and_marks_payload() {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "payload.big".to_string(),
            serde_json::json!("x".repeat(20_000)),
        );

        let guarded = build_guarded_log_attributes(attrs, 256, 4096);
        assert!(!guarded.truncated_fields.is_empty());
        assert!(guarded.attributes.contains_key("payload.truncated"));
        assert!(guarded.attributes.contains_key("payload.truncated_fields"));
    }

    #[test]
    fn test_guard_enforces_total_payload_budget() {
        let mut attrs = BTreeMap::new();
        for i in 0..200 {
            attrs.insert(format!("payload.k{i}"), serde_json::json!("x".repeat(128)));
        }

        let guarded = build_guarded_log_attributes(attrs, 1024, 8 * 1024);
        assert!(guarded.dropped_fields > 0);
        assert!(guarded.serialized_bytes <= 8 * 1024 + 2048);
    }

    #[test]
    fn test_sanitize_redacts_sensitive_fields() {
        let settings = StructuredLogSettings::default();
        let attrs = BTreeMap::from([
            ("auth.token".to_string(), serde_json::json!("secret-token")),
            (
                "payload.prompt".to_string(),
                serde_json::json!("prompt text"),
            ),
            (
                "event.kind".to_string(),
                serde_json::json!("runtime.llm_call.completed"),
            ),
        ]);

        let sanitized = sanitize_attributes(attrs, &settings);
        assert_eq!(
            sanitized.get("auth.token"),
            Some(&serde_json::json!(REDACTED))
        );
        assert_eq!(
            sanitized.get("payload.prompt"),
            Some(&serde_json::json!(REDACTED))
        );
    }

    #[test]
    fn test_sanitize_drops_claude_payload_when_disabled() {
        let settings = StructuredLogSettings {
            claude_capture_payloads: false,
            ..StructuredLogSettings::default()
        };
        let attrs = BTreeMap::from([
            (
                "event.kind".to_string(),
                serde_json::json!("claude.task.completed"),
            ),
            (
                "payload.input.user_message".to_string(),
                serde_json::json!("hello"),
            ),
            ("agent.id".to_string(), serde_json::json!("a1")),
        ]);

        let sanitized = sanitize_attributes(attrs, &settings);
        assert!(!sanitized.contains_key("payload.input.user_message"));
        assert_eq!(sanitized.get("agent.id"), Some(&serde_json::json!("a1")));
    }

    #[test]
    fn test_configure_applies_realtime_flush_settings() {
        configure(&openfang_types::config::SentryConfig {
            enable_logs: true,
            realtime_log_flush: true,
            realtime_log_flush_timeout_ms: 2750,
            ..Default::default()
        });

        let settings = current_settings();
        assert!(settings.enable_logs);
        assert!(settings.realtime_log_flush);
        assert_eq!(settings.realtime_log_flush_timeout_ms, 2750);
    }

    #[test]
    fn test_provider_family_normalizes_known_providers() {
        assert_eq!(provider_family("codex-cli"), "codex");
        assert_eq!(provider_family("claude-code"), "claude");
        assert_eq!(provider_family("gemini"), "gemini");
        assert_eq!(provider_family("GOOGLE"), "gemini");
        assert_eq!(provider_family("openai"), "openai");
        assert_eq!(provider_family("groq"), "groq");
    }

    #[test]
    fn test_indexed_tag_values_uses_allowlist_only() {
        let attrs = BTreeMap::from([
            (
                "event.kind".to_string(),
                serde_json::json!("ops.guard.heartbeat"),
            ),
            ("event.id".to_string(), serde_json::json!("evt-1")),
            ("request.id".to_string(), serde_json::json!("req-1")),
            ("run.id".to_string(), serde_json::json!("run-1")),
            ("trace.id".to_string(), serde_json::json!("trace-1")),
            ("session.id".to_string(), serde_json::json!("session-1")),
            ("agent.id".to_string(), serde_json::json!("agent-1")),
            (
                "agent.name".to_string(),
                serde_json::json!("vacation-guard"),
            ),
            ("channel.kind".to_string(), serde_json::json!("ops")),
            ("outcome".to_string(), serde_json::json!("success")),
            ("model.provider".to_string(), serde_json::json!("codex-cli")),
            (
                "payload.report.source".to_string(),
                serde_json::json!("should-stay-extra"),
            ),
            ("http.status_code".to_string(), serde_json::json!(200)),
        ]);

        let tags = indexed_tag_values(&attrs);
        assert_eq!(
            tags.get("event.kind").map(String::as_str),
            Some("ops.guard.heartbeat")
        );
        assert_eq!(tags.get("event.id").map(String::as_str), Some("evt-1"));
        assert_eq!(tags.get("request.id").map(String::as_str), Some("req-1"));
        assert_eq!(tags.get("run.id").map(String::as_str), Some("run-1"));
        assert_eq!(tags.get("trace.id").map(String::as_str), Some("trace-1"));
        assert_eq!(
            tags.get("session.id").map(String::as_str),
            Some("session-1")
        );
        assert_eq!(tags.get("agent.id").map(String::as_str), Some("agent-1"));
        assert_eq!(
            tags.get("agent.name").map(String::as_str),
            Some("vacation-guard")
        );
        assert_eq!(tags.get("channel.kind").map(String::as_str), Some("ops"));
        assert_eq!(tags.get("outcome").map(String::as_str), Some("success"));
        assert_eq!(tags.get("provider").map(String::as_str), Some("codex-cli"));
        assert_eq!(
            tags.get("provider.family").map(String::as_str),
            Some("codex")
        );
        assert!(!tags.contains_key("payload.report.source"));
        assert!(!tags.contains_key("http.status_code"));
    }

    #[test]
    fn test_indexed_tag_values_omits_missing_fields() {
        let attrs = BTreeMap::from([
            (
                "event.kind".to_string(),
                serde_json::json!("artifact.recorded"),
            ),
            (
                "payload.upload.filename".to_string(),
                serde_json::json!("hello.txt"),
            ),
        ]);

        let tags = indexed_tag_values(&attrs);
        assert_eq!(
            tags.get("event.kind").map(String::as_str),
            Some("artifact.recorded")
        );
        assert!(!tags.contains_key("run.id"));
        assert!(!tags.contains_key("request.id"));
        assert!(!tags.contains_key("provider"));
        assert!(!tags.contains_key("provider.family"));
        assert!(!tags.contains_key("payload.upload.filename"));
    }

    #[test]
    fn test_indexed_tag_values_derives_provider_family_from_provider_field() {
        let attrs = BTreeMap::from([
            (
                "event.kind".to_string(),
                serde_json::json!("runtime.llm_call.completed"),
            ),
            ("provider".to_string(), serde_json::json!("claude-code")),
        ]);

        let tags = indexed_tag_values(&attrs);
        assert_eq!(
            tags.get("provider").map(String::as_str),
            Some("claude-code")
        );
        assert_eq!(
            tags.get("provider.family").map(String::as_str),
            Some("claude")
        );
    }

    #[tokio::test]
    async fn test_capture_structured_log_persists_fact_event() {
        let tmp = TempDir::new().unwrap();
        let conn = Arc::new(Mutex::new(
            Connection::open(tmp.path().join("events.db")).unwrap(),
        ));
        openfang_memory::migration::run_migrations(&conn.lock().unwrap()).unwrap();
        configure_fact_store(conn.clone());

        let _ = scope_event_context(
            EventContext {
                trace_id: Some("trace-1".to_string()),
                request_id: Some("request-1".to_string()),
                run_id: Some("run-1".to_string()),
                session_id: Some("session-1".to_string()),
                agent_id: Some("agent-1".to_string()),
                agent_name: Some("tester".to_string()),
                channel_kind: Some("http".to_string()),
                channel_user_id: Some("user-1".to_string()),
            },
            async {
                capture_structured_log(
                    sentry::Level::Info,
                    "runtime.agent_loop.completed",
                    BTreeMap::from([
                        (
                            "event.kind".to_string(),
                            serde_json::json!("runtime.agent_loop.completed"),
                        ),
                        ("outcome".to_string(), serde_json::json!("success")),
                        ("duration_ms".to_string(), serde_json::json!(12)),
                        (
                            "payload.output.response".to_string(),
                            serde_json::json!("done"),
                        ),
                    ]),
                )
            },
        )
        .await;

        let rows = FactEventStore::new(conn)
            .query(
                &openfang_types::facts::FactEventFilter {
                    run_id: Some("run-1".to_string()),
                    limit: Some(10),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event.run.id.as_deref(), Some("run-1"));
        assert_eq!(rows[0].event.agent.name.as_deref(), Some("tester"));
    }
}
