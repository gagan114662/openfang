//! Raindrop incident types for integration.

use serde::{Deserialize, Serialize};

/// Raindrop incident record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaindropIncident {
    pub id: String,
    pub workspace_id: String,
    pub agent_id: String,
    pub signal_label: String,
    pub severity: RaindropSeverity,
    pub status: RaindropIncidentStatus,
    pub latest_message: String,
    pub source_system: Option<String>,
    pub created_at: String,
}

/// Raindrop severity levels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum RaindropSeverity {
    Low,
    Medium,
    High,
    Critical,
}

/// Raindrop incident status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum RaindropIncidentStatus {
    Open,
    Resolved,
}
