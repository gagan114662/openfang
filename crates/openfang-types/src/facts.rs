//! Canonical structured event and artifact types for the OpenFang data spine.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CANONICAL_EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalEventId {
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalAgentRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalChannelRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalArtifactRefs {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalCost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalModelRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalEvent {
    pub schema_version: u32,
    pub event: CanonicalEventId,
    pub occurred_at: String,
    #[serde(default)]
    pub trace: CanonicalRef,
    #[serde(default)]
    pub request: CanonicalRef,
    #[serde(default)]
    pub run: CanonicalRef,
    #[serde(default)]
    pub session: CanonicalRef,
    #[serde(default)]
    pub agent: CanonicalAgentRef,
    #[serde(default)]
    pub channel: CanonicalChannelRef,
    #[serde(default)]
    pub artifact: CanonicalArtifactRefs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub cost: CanonicalCost,
    #[serde(default)]
    pub model: CanonicalModelRef,
    #[serde(default)]
    pub payload: Value,
}

impl Default for CanonicalEvent {
    fn default() -> Self {
        Self {
            schema_version: CANONICAL_EVENT_SCHEMA_VERSION,
            event: CanonicalEventId::default(),
            occurred_at: String::new(),
            trace: CanonicalRef::default(),
            request: CanonicalRef::default(),
            run: CanonicalRef::default(),
            session: CanonicalRef::default(),
            agent: CanonicalAgentRef::default(),
            channel: CanonicalChannelRef::default(),
            artifact: CanonicalArtifactRefs::default(),
            outcome: None,
            duration_ms: None,
            cost: CanonicalCost::default(),
            model: CanonicalModelRef::default(),
            payload: Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtifactRecord {
    pub artifact_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub artifact_kind: String,
    pub storage_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FactEventFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactEventRecord {
    pub event_id: String,
    pub event_kind: String,
    pub occurred_at: String,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub trace_id: Option<String>,
    pub outcome: Option<String>,
    pub event: CanonicalEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub first_occurred_at: Option<String>,
    pub last_occurred_at: Option<String>,
    pub event_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<FactEventRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRecord>,
}
