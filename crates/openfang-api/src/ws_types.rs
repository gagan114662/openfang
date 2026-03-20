//! WebSocket v1 envelope and topic/ops types used by the `/ws` multiplexed transport.

use serde::{Deserialize, Serialize};

/// Transport-level envelope for all WS v1 frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsFrame {
    pub v: u8,
    pub id: Option<String>,
    #[serde(default)]
    pub ts: Option<String>,
    pub topic: String,
    pub op: String,
    #[serde(default)]
    pub seq: Option<u64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlAuthenticate {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlSubscribe {
    pub topic: String,
    #[serde(default)]
    pub filter: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlUnsubscribe {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeStream {
    pub key: String,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResume {
    pub streams: Vec<ResumeStream>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlAck {
    pub key: String,
    pub last_seq: u64,
    /// Optional convenience: grant additional credits for the stream in the same message.
    #[serde(default)]
    pub grant: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunStart {
    pub agent_id: String,
    pub message: String,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecStart {
    pub tool: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// Optional agent_id for capability scoping and audit attribution.
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsGrant {
    pub topic: String,
    pub key: String,
    pub grant: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlErrorPayload {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
}
