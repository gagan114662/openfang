use chrono::{DateTime, Utc};
use openfang_memory::MemorySubstrate;
use openfang_types::agent::AgentId;
use openfang_types::nitro::{
    AgentComputer, ComputerStatus, ExtensionTxn, InstalledExtensionRef, NitroEvent,
    NitroExtensionManifest,
};
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Nitro manager backing agent-owned computer state and extension lifecycle.
pub struct NitroComputerManager {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) home_dir: PathBuf,
    pub(crate) workspaces_dir: PathBuf,
    pub(crate) mirror_events_jsonl: bool,
}

impl NitroComputerManager {
    pub fn new(
        memory: &MemorySubstrate,
        home_dir: PathBuf,
        workspaces_dir: PathBuf,
        mirror: bool,
    ) -> Self {
        Self {
            conn: memory.usage_conn(),
            home_dir,
            workspaces_dir,
            mirror_events_jsonl: mirror,
        }
    }

    pub(crate) fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        }
    }

    pub fn nitro_root_dir(&self) -> PathBuf {
        self.home_dir.join("nitro")
    }

    pub fn global_extensions_dir(&self) -> PathBuf {
        self.nitro_root_dir().join("global_extensions")
    }

    pub fn events_jsonl_path(&self) -> PathBuf {
        self.nitro_root_dir().join("events.jsonl")
    }

    pub fn agent_computer_root(&self, agent_id: AgentId) -> PathBuf {
        // Best-effort deterministic fallback path.
        self.workspaces_dir
            .join(format!("agent-{}", &agent_id.to_string()[..8]))
            .join("computer")
    }

    pub fn bootstrap_computer(
        &self,
        agent_id: AgentId,
        workspace: Option<&Path>,
    ) -> Result<AgentComputer, String> {
        let root = workspace
            .map(|w| w.join("computer"))
            .unwrap_or_else(|| self.agent_computer_root(agent_id));

        std::fs::create_dir_all(root.join("extensions")).map_err(|e| {
            format!(
                "Failed to create agent computer directory '{}': {e}",
                root.display()
            )
        })?;

        let now = Utc::now();
        let conn = self.lock_conn();

        conn.execute(
            "INSERT INTO agent_computers (agent_id, computer_revision, status, root_path, updated_at)
             VALUES (?1, 0, 'ready', ?2, ?3)
             ON CONFLICT(agent_id) DO UPDATE SET
                status = excluded.status,
                root_path = excluded.root_path,
                updated_at = excluded.updated_at",
            rusqlite::params![agent_id.to_string(), root.display().to_string(), now.to_rfc3339()],
        )
        .map_err(|e| format!("Failed to upsert agent computer: {e}"))?;

        drop(conn);
        let _ = self.record_event(NitroEvent::ComputerBooted { agent_id });
        self.get_computer(agent_id).map(|opt| {
            opt.unwrap_or(AgentComputer {
                agent_id,
                computer_revision: 0,
                status: ComputerStatus::Ready,
                root_path: root,
                installed_extensions: Vec::new(),
            })
        })
    }

    pub fn get_computer(&self, agent_id: AgentId) -> Result<Option<AgentComputer>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT computer_revision, status, root_path FROM agent_computers WHERE agent_id = ?1",
            )
            .map_err(|e| format!("Failed to prepare get_computer query: {e}"))?;

        let row = stmt
            .query_row([agent_id.to_string()], |row| {
                let revision: i64 = row.get(0)?;
                let status_str: String = row.get(1)?;
                let root_path: String = row.get(2)?;
                Ok((revision, status_str, root_path))
            })
            .optional()
            .map_err(|e| format!("Failed to query agent computer: {e}"))?;

        drop(stmt);
        drop(conn);

        let Some((revision, status_str, root_path)) = row else {
            return Ok(None);
        };

        let installed_extensions = self.list_extensions(agent_id)?;
        Ok(Some(AgentComputer {
            agent_id,
            computer_revision: revision.max(0) as u64,
            status: parse_status(&status_str),
            root_path: PathBuf::from(root_path),
            installed_extensions,
        }))
    }

    pub fn list_extensions(&self, agent_id: AgentId) -> Result<Vec<InstalledExtensionRef>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT extension_id, version, install_state, installed_at, updated_at
                 FROM nitro_extensions_installed
                 WHERE agent_id = ?1
                 ORDER BY extension_id ASC",
            )
            .map_err(|e| format!("Failed to prepare list_extensions query: {e}"))?;

        let rows = stmt
            .query_map([agent_id.to_string()], |row| {
                let extension_id: String = row.get(0)?;
                let version: String = row.get(1)?;
                let install_state: String = row.get(2)?;
                let installed_at: String = row.get(3)?;
                let updated_at: String = row.get(4)?;
                Ok((
                    extension_id,
                    version,
                    install_state,
                    installed_at,
                    updated_at,
                ))
            })
            .map_err(|e| format!("Failed to iterate extensions: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            let (extension_id, version, install_state, installed_at, updated_at) =
                row.map_err(|e| format!("Failed to decode extension row: {e}"))?;
            out.push(InstalledExtensionRef {
                extension_id,
                version,
                install_state,
                installed_at: parse_ts(&installed_at),
                updated_at: parse_ts(&updated_at),
            });
        }
        Ok(out)
    }

    pub fn capability_graph(&self, agent_id: AgentId) -> Result<serde_json::Value, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT capability_name, extension_id, version, bound_at
                 FROM nitro_capability_bindings
                 WHERE agent_id = ?1
                 ORDER BY capability_name ASC",
            )
            .map_err(|e| format!("Failed to prepare capability_graph query: {e}"))?;

        let rows = stmt
            .query_map([agent_id.to_string()], |row| {
                Ok(serde_json::json!({
                    "capability": row.get::<_, String>(0)?,
                    "extension_id": row.get::<_, String>(1)?,
                    "version": row.get::<_, String>(2)?,
                    "bound_at": row.get::<_, String>(3)?,
                }))
            })
            .map_err(|e| format!("Failed to iterate capability graph: {e}"))?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| format!("Failed to decode capability graph row: {e}"))?);
        }

        Ok(serde_json::json!({
            "agent_id": agent_id.to_string(),
            "bindings": entries,
        }))
    }

    pub fn list_global_extensions(&self) -> Result<Vec<serde_json::Value>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, version, wasm_sha256, artifact_path, published_by, created_at
                 FROM nitro_extensions_global
                 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("Failed to prepare list_global_extensions query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "version": row.get::<_, String>(1)?,
                    "wasm_sha256": row.get::<_, String>(2)?,
                    "artifact_path": row.get::<_, String>(3)?,
                    "published_by": row.get::<_, String>(4)?,
                    "created_at": row.get::<_, String>(5)?,
                }))
            })
            .map_err(|e| format!("Failed to iterate global extensions: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Failed to decode global extension row: {e}"))?);
        }
        Ok(out)
    }

    pub fn list_txns(&self, limit: usize) -> Result<Vec<ExtensionTxn>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT txn_id, actor_agent_id, scope, action, target_agent_id, extension_id,
                        base_revision, new_revision, status, error, created_at
                 FROM nitro_extension_txn
                 ORDER BY created_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare list_txns query: {e}"))?;

        let rows = stmt
            .query_map([limit as i64], |row| {
                let actor_agent_id: String = row.get(1)?;
                let target_agent_id: Option<String> = row.get(4)?;
                Ok(ExtensionTxn {
                    txn_id: row.get(0)?,
                    actor_agent_id: parse_agent_id(&actor_agent_id),
                    scope: parse_scope(&row.get::<_, String>(2)?),
                    action: parse_action(&row.get::<_, String>(3)?),
                    target_agent_id: target_agent_id.as_deref().map(parse_agent_id),
                    extension_id: row.get(5)?,
                    base_revision: row.get::<_, i64>(6)?.max(0) as u64,
                    new_revision: row.get::<_, Option<i64>>(7)?.map(|v| v.max(0) as u64),
                    status: parse_txn_status(&row.get::<_, String>(8)?),
                    error: row.get(9)?,
                    timestamp: parse_ts(&row.get::<_, String>(10)?),
                })
            })
            .map_err(|e| format!("Failed to iterate txns: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Failed to decode txn row: {e}"))?);
        }
        Ok(out)
    }

    pub fn list_events_since(
        &self,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT seq, event_type, payload_json, created_at
                 FROM nitro_events
                 WHERE seq > ?1
                 ORDER BY seq ASC
                 LIMIT ?2",
            )
            .map_err(|e| format!("Failed to prepare list_events_since query: {e}"))?;

        let rows = stmt
            .query_map(
                rusqlite::params![after_seq, limit as i64],
                |row: &rusqlite::Row<'_>| {
                let payload: String = row.get(2)?;
                let parsed_payload: serde_json::Value = serde_json::from_str(&payload)
                    .unwrap_or_else(|_| serde_json::json!({"raw": payload}));
                Ok(serde_json::json!({
                    "seq": row.get::<_, i64>(0)?,
                    "event_type": row.get::<_, String>(1)?,
                    "payload": parsed_payload,
                    "created_at": row.get::<_, String>(3)?,
                }))
                },
            )
            .map_err(|e| format!("Failed to iterate events: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Failed to decode event row: {e}"))?);
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub(crate) fn parse_manifest(json: &str) -> Result<NitroExtensionManifest, String> {
        serde_json::from_str(json).map_err(|e| format!("Invalid extension manifest JSON: {e}"))
    }

    pub(crate) fn now_rfc3339() -> String {
        Utc::now().to_rfc3339()
    }
}

pub(crate) fn parse_status(s: &str) -> ComputerStatus {
    match s {
        "ready" => ComputerStatus::Ready,
        "degraded" => ComputerStatus::Degraded,
        "recovering" => ComputerStatus::Recovering,
        _ => ComputerStatus::Booting,
    }
}

pub(crate) fn parse_agent_id(s: &str) -> AgentId {
    s.parse().unwrap_or_else(|_| AgentId::new())
}

pub(crate) fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

pub(crate) fn parse_scope(s: &str) -> openfang_types::nitro::TxnScope {
    use openfang_types::nitro::TxnScope;
    match s {
        "global_publish" => TxnScope::GlobalPublish,
        _ => TxnScope::AgentLocal,
    }
}

pub(crate) fn parse_action(s: &str) -> openfang_types::nitro::TxnAction {
    use openfang_types::nitro::TxnAction;
    match s {
        "update" => TxnAction::Update,
        "remove" => TxnAction::Remove,
        "publish" => TxnAction::Publish,
        "rollback" => TxnAction::Rollback,
        _ => TxnAction::Install,
    }
}

pub(crate) fn parse_txn_status(s: &str) -> openfang_types::nitro::TxnStatus {
    use openfang_types::nitro::TxnStatus;
    match s {
        "succeeded" => TxnStatus::Succeeded,
        "failed" => TxnStatus::Failed,
        "conflict" => TxnStatus::Conflict,
        _ => TxnStatus::Pending,
    }
}
