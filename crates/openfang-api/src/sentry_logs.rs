use sentry::protocol::Value as SentryValue;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet};

pub const DEFAULT_MAX_ATTRIBUTE_BYTES: usize = 16 * 1024;
pub const DEFAULT_MAX_PAYLOAD_BYTES: usize = 512 * 1024;
const PAYLOAD_MARKER_RESERVE_BYTES: usize = 1024;
const MAX_TRUNCATED_FIELDS_REPORTED: usize = 64;

#[derive(Debug, Clone)]
pub struct GuardedLogAttributes {
    pub json_attributes: BTreeMap<String, JsonValue>,
    pub attributes: BTreeMap<String, SentryValue>,
    pub truncated_fields: Vec<String>,
    pub dropped_fields: usize,
    pub serialized_bytes: usize,
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

fn attr_string(attributes: &BTreeMap<String, JsonValue>, key: &str) -> Option<String> {
    attributes
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
}

fn insert_tag_if_present(tags: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            tags.insert(key.to_string(), trimmed.to_string());
        }
    }
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
    insert_tag_if_present(&mut tags, "run.id", attr_string(attributes, "run.id"));
    insert_tag_if_present(
        &mut tags,
        "request.id",
        attr_string(attributes, "request.id"),
    );
    insert_tag_if_present(&mut tags, "trace.id", attr_string(attributes, "trace.id"));
    insert_tag_if_present(
        &mut tags,
        "folder.path_hash",
        attr_string(attributes, "folder.path_hash"),
    );
    insert_tag_if_present(&mut tags, "scan.mode", attr_string(attributes, "scan.mode"));
    insert_tag_if_present(&mut tags, "graph_id", attr_string(attributes, "graph_id"));
    insert_tag_if_present(
        &mut tags,
        "simulation_id",
        attr_string(attributes, "simulation_id"),
    );
    insert_tag_if_present(&mut tags, "outcome", attr_string(attributes, "outcome"));
    tags
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

    let json_attrs = kept.clone();
    let sentry_attrs = kept
        .into_iter()
        .map(|(k, v)| (k, sentry_value_from_json(v)))
        .collect::<BTreeMap<_, _>>();

    GuardedLogAttributes {
        json_attributes: json_attrs,
        attributes: sentry_attrs,
        truncated_fields: truncated.into_iter().collect(),
        dropped_fields,
        serialized_bytes,
    }
}

pub fn capture_structured_log(
    level: sentry::Level,
    body: impl Into<String>,
    attributes: BTreeMap<String, JsonValue>,
) -> GuardedLogAttributes {
    let guarded = build_guarded_log_attributes(
        attributes,
        DEFAULT_MAX_ATTRIBUTE_BYTES,
        DEFAULT_MAX_PAYLOAD_BYTES,
    );

    let message = body.into();
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
            sentry::capture_message(&message, level);
        },
    );

    guarded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_flatten_json_nested_and_arrays() {
        let input = json!({
            "agent": {"id": "a1", "meta": {"provider": "groq"}},
            "tools": [{"name": "web"}, {"name": "fs"}],
            "status": 200
        });

        let out = flatten_with_prefix("payload", &input);
        assert_eq!(out.get("payload.agent.id"), Some(&json!("a1")));
        assert_eq!(out.get("payload.agent.meta.provider"), Some(&json!("groq")));
        assert_eq!(out.get("payload.tools.0.name"), Some(&json!("web")));
        assert_eq!(out.get("payload.tools.1.name"), Some(&json!("fs")));
        assert_eq!(out.get("payload.status"), Some(&json!(200)));
    }

    #[test]
    fn test_guard_truncates_oversized_field_and_marks_payload() {
        let mut attrs = BTreeMap::new();
        attrs.insert("payload.big".to_string(), json!("x".repeat(20_000)));

        let guarded = build_guarded_log_attributes(attrs, 256, 4096);
        assert!(!guarded.truncated_fields.is_empty());
        assert!(guarded.attributes.contains_key("payload.truncated"));
        assert!(guarded.attributes.contains_key("payload.truncated_fields"));
    }

    #[test]
    fn test_guard_enforces_total_payload_budget() {
        let mut attrs = BTreeMap::new();
        for i in 0..200 {
            attrs.insert(format!("payload.k{i}"), json!("x".repeat(128)));
        }

        let guarded = build_guarded_log_attributes(attrs, 1024, 8 * 1024);
        assert!(guarded.dropped_fields > 0);
        assert!(guarded.serialized_bytes <= 8 * 1024 + 2048);
    }

    #[test]
    fn test_indexed_tag_values_extracts_mirofish_fields() {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "event.kind".to_string(),
            json!("mirofish.autotrigger.decision"),
        );
        attrs.insert("event.category".to_string(), json!("mirofish"));
        attrs.insert("run.id".to_string(), json!("run-123"));
        attrs.insert("request.id".to_string(), json!("req-123"));
        attrs.insert("trace.id".to_string(), json!("trace-123"));
        attrs.insert("folder.path_hash".to_string(), json!("abc123"));
        attrs.insert("scan.mode".to_string(), json!("fast"));

        let tags = indexed_tag_values(&attrs);
        assert_eq!(
            tags.get("event.kind"),
            Some(&"mirofish.autotrigger.decision".to_string())
        );
        assert_eq!(tags.get("run.id"), Some(&"run-123".to_string()));
        assert_eq!(tags.get("request.id"), Some(&"req-123".to_string()));
        assert_eq!(tags.get("trace.id"), Some(&"trace-123".to_string()));
        assert_eq!(tags.get("scan.mode"), Some(&"fast".to_string()));
        assert_eq!(tags.get("folder.path_hash"), Some(&"abc123".to_string()));
    }

    #[test]
    fn test_indexed_tag_values_ignores_empty_values() {
        let mut attrs = BTreeMap::new();
        attrs.insert("event.kind".to_string(), json!(" "));
        attrs.insert("run.id".to_string(), json!(""));

        let tags = indexed_tag_values(&attrs);
        assert!(tags.is_empty());
    }
}
