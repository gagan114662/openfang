//! NitroOS agent-computer types.

use crate::agent::AgentId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Runtime status of an agent-owned computer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerStatus {
    #[default]
    Booting,
    Ready,
    Degraded,
    Recovering,
}

/// Supported WASM ABI contracts for Nitro extensions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NitroWasmAbi {
    /// Requires `memory`, `alloc(i32)->i32`, `execute(i32,i32)->i64`.
    #[default]
    AllocExecute,
}

/// A capability exposed by a Nitro extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NitroCapabilityDef {
    pub name: String,
    pub input_schema: serde_json::Value,
    pub description: String,
}

/// Extension manifest used by Nitro runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NitroExtensionManifest {
    pub id: String,
    pub version: String,
    pub entry: String,
    #[serde(default)]
    pub capabilities: Vec<NitroCapabilityDef>,
    #[serde(default)]
    pub required_host_apis: Vec<String>,
    #[serde(default)]
    pub abi: NitroWasmAbi,
}

/// Installed extension reference in an agent computer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledExtensionRef {
    pub extension_id: String,
    pub version: String,
    pub install_state: String,
    pub installed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Agent-owned computer projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentComputer {
    pub agent_id: AgentId,
    pub computer_revision: u64,
    pub status: ComputerStatus,
    pub root_path: PathBuf,
    #[serde(default)]
    pub installed_extensions: Vec<InstalledExtensionRef>,
}

/// Transaction scope for extension lifecycle mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxnScope {
    AgentLocal,
    GlobalPublish,
}

/// Transaction action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxnAction {
    Install,
    Update,
    Remove,
    Publish,
    Rollback,
}

/// Transaction status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxnStatus {
    Pending,
    Succeeded,
    Failed,
    Conflict,
}

/// Persisted extension transaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionTxn {
    pub txn_id: String,
    pub actor_agent_id: AgentId,
    pub scope: TxnScope,
    pub action: TxnAction,
    pub target_agent_id: Option<AgentId>,
    pub extension_id: String,
    pub base_revision: u64,
    pub new_revision: Option<u64>,
    pub status: TxnStatus,
    pub error: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Nitro event payloads used by transaction log and SSE stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum NitroEvent {
    ComputerBooted {
        agent_id: AgentId,
    },
    ComputerRecovered {
        agent_id: AgentId,
    },
    ExtensionInstalled {
        agent_id: AgentId,
        extension_id: String,
        version: String,
    },
    ExtensionUpdated {
        agent_id: AgentId,
        extension_id: String,
        from_version: String,
        to_version: String,
    },
    ExtensionRemoved {
        agent_id: AgentId,
        extension_id: String,
    },
    ExtensionPublished {
        agent_id: AgentId,
        extension_id: String,
        version: String,
    },
    CapabilityBound {
        agent_id: AgentId,
        capability: String,
        extension_id: String,
        version: String,
    },
    CapabilityUnbound {
        agent_id: AgentId,
        capability: String,
        extension_id: String,
    },
    ExtensionExecutionFailed {
        agent_id: AgentId,
        extension_id: String,
        capability: String,
        error: String,
    },
    RevisionConflict {
        agent_id: AgentId,
        extension_id: String,
        expected_base_revision: u64,
        actual_revision: u64,
    },
    ComputerV2Created {
        agent_id: AgentId,
        computer_id: String,
        backend: ComputerBackend,
    },
    ComputerV2Started {
        agent_id: AgentId,
        computer_id: String,
    },
    ComputerV2Suspended {
        agent_id: AgentId,
        computer_id: String,
    },
    ComputerV2Stopped {
        agent_id: AgentId,
        computer_id: String,
    },
    ComputerV2Rebuilt {
        agent_id: AgentId,
        computer_id: String,
    },
    ComputerV2GpuDegraded {
        agent_id: AgentId,
        computer_id: String,
        reason: String,
    },
    MemoryGcCompleted {
        agent_id: AgentId,
        reclaimed_items: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerBackend {
    RemoteKvm,
    LocalAppleVf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOs {
    Linux,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuMode {
    #[default]
    None,
    Shared,
    Passthrough,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuHealth {
    #[default]
    Unknown,
    Ready,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputerPlacementPolicy {
    #[serde(default = "default_true")]
    pub remote_primary: bool,
    #[serde(default = "default_true")]
    pub local_fallback: bool,
    pub affinity: Option<String>,
    #[serde(default)]
    pub gpu_required: bool,
}

impl Default for ComputerPlacementPolicy {
    fn default() -> Self {
        Self {
            remote_primary: true,
            local_fallback: true,
            affinity: None,
            gpu_required: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputerResourcePolicy {
    pub vcpu: u32,
    pub memory_mb: u32,
    pub disk_gb: u32,
    pub io_weight: u32,
    pub net_profile: String,
}

impl Default for ComputerResourcePolicy {
    fn default() -> Self {
        Self {
            vcpu: 2,
            memory_mb: 4096,
            disk_gb: 40,
            io_weight: 100,
            net_profile: "default".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryTierPolicy {
    pub hot_mb: u32,
    pub warm_mb: u32,
    pub cold_gb: u32,
    pub gc_interval_secs: u64,
    pub compaction_ratio: f32,
}

impl Default for MemoryTierPolicy {
    fn default() -> Self {
        Self {
            hot_mb: 256,
            warm_mb: 1024,
            cold_gb: 10,
            gc_interval_secs: 60,
            compaction_ratio: 0.25,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuCapability {
    #[serde(default)]
    pub requested: bool,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub mode: GpuMode,
    #[serde(default)]
    pub health: GpuHealth,
}

impl Default for GpuCapability {
    fn default() -> Self {
        Self {
            requested: false,
            available: false,
            mode: GpuMode::None,
            health: GpuHealth::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentComputerV2 {
    pub agent_id: AgentId,
    pub computer_id: String,
    pub status: ComputerStatus,
    pub backend: ComputerBackend,
    pub host: String,
    pub revision: u64,
    pub resources: ComputerResourcePolicy,
    pub guest_os: GuestOs,
    pub placement_policy: ComputerPlacementPolicy,
    pub memory_tier_policy: MemoryTierPolicy,
    pub gpu: GpuCapability,
    pub root_path: PathBuf,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputerActionRequest {
    pub action_type: String,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputerActionResult {
    pub action_type: String,
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputerCreateOrGetRequest {
    #[serde(default)]
    pub placement_policy: ComputerPlacementPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputerResourceUpdateRequest {
    pub base_revision: u64,
    pub resources: ComputerResourcePolicy,
}

/// Request for local extension install.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstallExtensionRequest {
    pub manifest: NitroExtensionManifest,
    pub wasm_path: String,
    #[serde(default)]
    pub base_revision: u64,
}

/// Request for local extension update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateExtensionRequest {
    pub manifest: NitroExtensionManifest,
    pub wasm_path: String,
    pub base_revision: u64,
}

/// Request for local extension removal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoveExtensionRequest {
    pub extension_id: String,
    pub base_revision: u64,
}

/// Request for publishing a locally-installed extension globally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PublishExtensionRequest {
    pub extension_id: String,
    pub version: String,
    pub base_revision: u64,
}

fn default_true() -> bool {
    true
}
