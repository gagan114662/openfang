use super::computer::NitroComputerManager;
use openfang_types::agent::AgentId;
use openfang_types::nitro::{
    ExtensionTxn, NitroCapabilityDef, NitroEvent, NitroExtensionManifest, TxnAction, TxnScope,
    TxnStatus,
};
use openfang_types::tool::ToolDefinition;
use rusqlite::{params, OptionalExtension};
use std::io::Write;

impl NitroComputerManager {
    pub(crate) fn current_revision(&self, agent_id: AgentId) -> Result<u64, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare("SELECT computer_revision FROM agent_computers WHERE agent_id = ?1")
            .map_err(|e| format!("Failed to prepare current_revision query: {e}"))?;
        let rev = stmt
            .query_row([agent_id.to_string()], |row: &rusqlite::Row<'_>| {
                row.get::<_, i64>(0)
            })
            .optional()
            .map_err(|e| format!("Failed to query current_revision: {e}"))?
            .unwrap_or(0);
        Ok(rev.max(0) as u64)
    }

    pub(crate) fn bump_revision(&self, agent_id: AgentId, status: &str) -> Result<u64, String> {
        let conn = self.lock_conn();
        let current = {
            let mut stmt = conn
                .prepare("SELECT computer_revision FROM agent_computers WHERE agent_id = ?1")
                .map_err(|e| format!("Failed to prepare bump_revision query: {e}"))?;
            stmt.query_row([agent_id.to_string()], |row: &rusqlite::Row<'_>| {
                row.get::<_, i64>(0)
            })
                .optional()
                .map_err(|e| format!("Failed to query current revision: {e}"))?
                .unwrap_or(0)
                .max(0) as u64
        };
        let next = current + 1;
        conn.execute(
            "INSERT INTO agent_computers (agent_id, computer_revision, status, root_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(agent_id) DO UPDATE SET
               computer_revision = excluded.computer_revision,
               status = excluded.status,
               updated_at = excluded.updated_at",
            params![
                agent_id.to_string(),
                next as i64,
                status,
                self.agent_computer_root(agent_id).display().to_string(),
                Self::now_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to bump computer revision: {e}"))?;
        Ok(next)
    }

    pub(crate) fn upsert_installed(
        &self,
        agent_id: AgentId,
        manifest: &NitroExtensionManifest,
        install_state: &str,
    ) -> Result<(), String> {
        let now = Self::now_rfc3339();
        let manifest_json = serde_json::to_string(manifest)
            .map_err(|e| format!("Failed to serialize extension manifest: {e}"))?;
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO nitro_extensions_installed (
                agent_id, extension_id, version, local_manifest_json, install_state, installed_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(agent_id, extension_id) DO UPDATE SET
                version = excluded.version,
                local_manifest_json = excluded.local_manifest_json,
                install_state = excluded.install_state,
                updated_at = excluded.updated_at",
            params![
                agent_id.to_string(),
                manifest.id,
                manifest.version,
                manifest_json,
                install_state,
                now,
                now,
            ],
        )
        .map_err(|e| format!("Failed to upsert installed extension: {e}"))?;
        Ok(())
    }

    pub(crate) fn remove_installed(
        &self,
        agent_id: AgentId,
        extension_id: &str,
    ) -> Result<(), String> {
        let conn = self.lock_conn();
        conn.execute(
            "DELETE FROM nitro_extensions_installed WHERE agent_id = ?1 AND extension_id = ?2",
            params![agent_id.to_string(), extension_id],
        )
        .map_err(|e| format!("Failed to remove installed extension: {e}"))?;
        Ok(())
    }

    pub(crate) fn bind_capabilities(
        &self,
        agent_id: AgentId,
        extension_id: &str,
        version: &str,
        capabilities: &[NitroCapabilityDef],
    ) -> Result<(), String> {
        let now = Self::now_rfc3339();
        let conn = self.lock_conn();
        for cap in capabilities {
            conn.execute(
                "INSERT INTO nitro_capability_bindings (
                    agent_id, capability_name, extension_id, version, bound_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(agent_id, capability_name) DO UPDATE SET
                    extension_id = excluded.extension_id,
                    version = excluded.version,
                    bound_at = excluded.bound_at",
                params![agent_id.to_string(), cap.name, extension_id, version, now],
            )
            .map_err(|e| format!("Failed to bind capability '{}': {e}", cap.name))?;
        }
        Ok(())
    }

    pub(crate) fn unbind_capabilities(
        &self,
        agent_id: AgentId,
        extension_id: &str,
    ) -> Result<Vec<String>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT capability_name FROM nitro_capability_bindings
                 WHERE agent_id = ?1 AND extension_id = ?2",
            )
            .map_err(|e| format!("Failed to prepare unbind_capabilities query: {e}"))?;
        let rows = stmt
            .query_map(params![agent_id.to_string(), extension_id], |row: &rusqlite::Row<'_>| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("Failed to read bound capability rows: {e}"))?;

        let mut names = Vec::new();
        for row in rows {
            names.push(row.map_err(|e| format!("Failed to decode bound capability row: {e}"))?);
        }

        conn.execute(
            "DELETE FROM nitro_capability_bindings WHERE agent_id = ?1 AND extension_id = ?2",
            params![agent_id.to_string(), extension_id],
        )
        .map_err(|e| format!("Failed to unbind capabilities: {e}"))?;

        Ok(names)
    }

    #[allow(dead_code)]
    pub(crate) fn extension_tool_definitions(
        &self,
        agent_id: AgentId,
    ) -> Result<Vec<ToolDefinition>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT local_manifest_json
                 FROM nitro_extensions_installed
                 WHERE agent_id = ?1 AND install_state = 'active'",
            )
            .map_err(|e| format!("Failed to prepare extension_tool_definitions query: {e}"))?;

        let rows = stmt
            .query_map([agent_id.to_string()], |row: &rusqlite::Row<'_>| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("Failed to iterate extension manifests: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            let manifest_json =
                row.map_err(|e| format!("Failed to decode extension manifest row: {e}"))?;
            let manifest: NitroExtensionManifest = Self::parse_manifest(&manifest_json)?;
            for cap in manifest.capabilities {
                out.push(ToolDefinition {
                    name: cap.name,
                    description: cap.description,
                    input_schema: cap.input_schema,
                });
            }
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub(crate) fn bound_capability_names(
        &self,
        agent_id: AgentId,
    ) -> Result<std::collections::HashSet<String>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare("SELECT capability_name FROM nitro_capability_bindings WHERE agent_id = ?1")
            .map_err(|e| format!("Failed to prepare bound_capability_names query: {e}"))?;
        let rows = stmt
            .query_map([agent_id.to_string()], |row: &rusqlite::Row<'_>| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("Failed to iterate bound capability names: {e}"))?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            out.insert(row.map_err(|e| format!("Failed to decode capability name row: {e}"))?);
        }
        Ok(out)
    }

    pub(crate) fn record_txn(
        &self,
        txn: &openfang_types::nitro::ExtensionTxn,
    ) -> Result<(), String> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT OR REPLACE INTO nitro_extension_txn (
                txn_id, actor_agent_id, scope, action, target_agent_id, extension_id,
                base_revision, new_revision, status, error, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                txn.txn_id,
                txn.actor_agent_id.to_string(),
                serde_json::to_string(&txn.scope)
                    .unwrap_or_else(|_| "\"agent_local\"".to_string())
                    .replace('"', ""),
                serde_json::to_string(&txn.action)
                    .unwrap_or_else(|_| "\"install\"".to_string())
                    .replace('"', ""),
                txn.target_agent_id.map(|a| a.to_string()),
                txn.extension_id,
                txn.base_revision as i64,
                txn.new_revision.map(|v| v as i64),
                serde_json::to_string(&txn.status)
                    .unwrap_or_else(|_| "\"pending\"".to_string())
                    .replace('"', ""),
                txn.error,
                txn.timestamp.to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to persist Nitro txn: {e}"))?;
        emit_nitro_txn_to_sentry(txn);
        Ok(())
    }

    pub(crate) fn record_event(&self, event: NitroEvent) -> Result<(), String> {
        let event_type = match &event {
            NitroEvent::ComputerBooted { .. } => "computer_booted",
            NitroEvent::ComputerRecovered { .. } => "computer_recovered",
            NitroEvent::ExtensionInstalled { .. } => "extension_installed",
            NitroEvent::ExtensionUpdated { .. } => "extension_updated",
            NitroEvent::ExtensionRemoved { .. } => "extension_removed",
            NitroEvent::ExtensionPublished { .. } => "extension_published",
            NitroEvent::CapabilityBound { .. } => "capability_bound",
            NitroEvent::CapabilityUnbound { .. } => "capability_unbound",
            NitroEvent::ExtensionExecutionFailed { .. } => "extension_execution_failed",
            NitroEvent::RevisionConflict { .. } => "revision_conflict",
            NitroEvent::ComputerV2Created { .. } => "computer_v2_created",
            NitroEvent::ComputerV2Started { .. } => "computer_v2_started",
            NitroEvent::ComputerV2Suspended { .. } => "computer_v2_suspended",
            NitroEvent::ComputerV2Stopped { .. } => "computer_v2_stopped",
            NitroEvent::ComputerV2Rebuilt { .. } => "computer_v2_rebuilt",
            NitroEvent::ComputerV2GpuDegraded { .. } => "computer_v2_gpu_degraded",
            NitroEvent::MemoryGcCompleted { .. } => "memory_gc_completed",
        };

        let payload_json = serde_json::to_string(&event)
            .map_err(|e| format!("Failed to serialize Nitro event payload: {e}"))?;
        let now = Self::now_rfc3339();

        {
            let conn = self.lock_conn();
            conn.execute(
                "INSERT INTO nitro_events (event_type, payload_json, created_at) VALUES (?1, ?2, ?3)",
                params![event_type, payload_json, now],
            )
            .map_err(|e| format!("Failed to persist Nitro event: {e}"))?;
        }

        emit_nitro_event_to_sentry(event_type, &event, &payload_json);

        if self.mirror_events_jsonl {
            let mirror_path = self.events_jsonl_path();
            if let Some(parent) = mirror_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&mirror_path)
            {
                let line = serde_json::json!({
                    "ts": now,
                    "event_type": event_type,
                    "payload": event,
                });
                let _ = writeln!(f, "{}", serde_json::to_string(&line).unwrap_or_default());
            }
        }

        Ok(())
    }
}

fn emit_nitro_event_to_sentry(event_type: &str, event: &NitroEvent, payload_json: &str) {
    let level = match event {
        NitroEvent::ExtensionExecutionFailed { .. } => sentry::Level::Error,
        NitroEvent::RevisionConflict { .. } | NitroEvent::ComputerV2GpuDegraded { .. } => {
            sentry::Level::Warning
        }
        _ => sentry::Level::Info,
    };

    let mut data = std::collections::BTreeMap::new();
    data.insert(
        "event_type".to_string(),
        serde_json::Value::String(event_type.to_string()),
    );
    data.insert(
        "payload_json".to_string(),
        serde_json::Value::String(payload_json.to_string()),
    );

    sentry::add_breadcrumb(sentry::Breadcrumb {
        category: Some("nitro.event".to_string()),
        message: Some(format!("nitro::{event_type}")),
        level,
        data,
        ..Default::default()
    });

    let event_id = sentry::with_scope(
        |scope| {
            scope.set_tag("subsystem", "nitro");
            scope.set_tag("nitro_event_type", event_type);
            scope.set_extra("nitro_payload_json", payload_json.to_string().into());
        },
        || sentry::capture_message(&format!("nitro event: {event_type}"), level),
    );
    if event_id.is_nil() {
        tracing::warn!(
            event_type = event_type,
            level = ?level,
            "Nitro Sentry event capture returned nil event_id"
        );
    }
}

fn emit_nitro_txn_to_sentry(txn: &ExtensionTxn) {
    let level = match txn.status {
        TxnStatus::Failed => sentry::Level::Error,
        TxnStatus::Conflict => sentry::Level::Warning,
        TxnStatus::Pending | TxnStatus::Succeeded => sentry::Level::Info,
    };
    let scope = match txn.scope {
        TxnScope::AgentLocal => "agent_local",
        TxnScope::GlobalPublish => "global_publish",
    };
    let action = match txn.action {
        TxnAction::Install => "install",
        TxnAction::Update => "update",
        TxnAction::Remove => "remove",
        TxnAction::Publish => "publish",
        TxnAction::Rollback => "rollback",
    };
    let status = match txn.status {
        TxnStatus::Pending => "pending",
        TxnStatus::Succeeded => "succeeded",
        TxnStatus::Failed => "failed",
        TxnStatus::Conflict => "conflict",
    };

    let mut data = std::collections::BTreeMap::new();
    data.insert(
        "txn_id".to_string(),
        serde_json::Value::String(txn.txn_id.clone()),
    );
    data.insert(
        "scope".to_string(),
        serde_json::Value::String(scope.to_string()),
    );
    data.insert(
        "action".to_string(),
        serde_json::Value::String(action.to_string()),
    );
    data.insert(
        "status".to_string(),
        serde_json::Value::String(status.to_string()),
    );
    data.insert(
        "extension_id".to_string(),
        serde_json::Value::String(txn.extension_id.clone()),
    );
    if let Some(ref err) = txn.error {
        data.insert("error".to_string(), serde_json::Value::String(err.clone()));
    }

    sentry::add_breadcrumb(sentry::Breadcrumb {
        category: Some("nitro.txn".to_string()),
        message: Some(format!(
            "nitro txn {action}::{status} extension={}",
            txn.extension_id
        )),
        level,
        data,
        ..Default::default()
    });

    let event_id = sentry::with_scope(
        |scope_obj| {
            scope_obj.set_tag("subsystem", "nitro");
            scope_obj.set_tag("nitro_txn_action", action);
            scope_obj.set_tag("nitro_txn_status", status);
            scope_obj.set_tag("nitro_txn_scope", scope);
            scope_obj.set_tag("nitro_extension_id", txn.extension_id.as_str());
            scope_obj.set_extra("nitro_txn_id", txn.txn_id.clone().into());
            scope_obj.set_extra("nitro_base_revision", (txn.base_revision as i64).into());
            if let Some(new_rev) = txn.new_revision {
                scope_obj.set_extra("nitro_new_revision", (new_rev as i64).into());
            }
            if let Some(ref err) = txn.error {
                scope_obj.set_extra("nitro_txn_error", err.clone().into());
            }
        },
        || {
            sentry::capture_message(
                &format!(
                    "nitro txn: action={action} status={status} extension={}",
                    txn.extension_id
                ),
                level,
            )
        },
    );
    if event_id.is_nil() {
        tracing::warn!(
            action = action,
            status = status,
            extension_id = %txn.extension_id,
            level = ?level,
            "Nitro Sentry txn capture returned nil event_id"
        );
    }
}
