use super::computer::NitroComputerManager;
use chrono::Utc;
use openfang_runtime::kernel_handle::KernelHandle;
use openfang_types::agent::AgentId;
use openfang_types::nitro::{
    ExtensionTxn, InstallExtensionRequest, NitroEvent, PublishExtensionRequest,
    RemoveExtensionRequest, TxnAction, TxnScope, TxnStatus, UpdateExtensionRequest,
};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

impl NitroComputerManager {
    pub fn install_extension(
        &self,
        actor: AgentId,
        target: AgentId,
        req: InstallExtensionRequest,
    ) -> Result<ExtensionTxn, String> {
        self.install_or_update(
            actor,
            target,
            req.base_revision,
            req.manifest,
            req.wasm_path,
            TxnAction::Install,
        )
    }

    pub fn update_extension(
        &self,
        actor: AgentId,
        target: AgentId,
        req: UpdateExtensionRequest,
    ) -> Result<ExtensionTxn, String> {
        self.install_or_update(
            actor,
            target,
            req.base_revision,
            req.manifest,
            req.wasm_path,
            TxnAction::Update,
        )
    }

    fn install_or_update(
        &self,
        actor: AgentId,
        target: AgentId,
        base_revision: u64,
        mut manifest: openfang_types::nitro::NitroExtensionManifest,
        wasm_path: String,
        action: TxnAction,
    ) -> Result<ExtensionTxn, String> {
        let current_revision = self.current_revision(target)?;
        if current_revision != base_revision {
            let txn = ExtensionTxn {
                txn_id: uuid::Uuid::new_v4().to_string(),
                actor_agent_id: actor,
                scope: TxnScope::AgentLocal,
                action,
                target_agent_id: Some(target),
                extension_id: manifest.id.clone(),
                base_revision,
                new_revision: None,
                status: TxnStatus::Conflict,
                error: Some(format!(
                    "Revision conflict: expected base revision {}, actual {}",
                    base_revision, current_revision
                )),
                timestamp: Utc::now(),
            };
            self.record_txn(&txn)?;
            self.record_event(NitroEvent::RevisionConflict {
                agent_id: target,
                extension_id: manifest.id,
                expected_base_revision: base_revision,
                actual_revision: current_revision,
            })?;
            return Ok(txn);
        }

        let source_path = PathBuf::from(&wasm_path);
        if !source_path.exists() {
            return Err(format!(
                "WASM path does not exist: {}",
                source_path.display()
            ));
        }

        let agent_root = self
            .get_computer(target)?
            .map(|c| c.root_path)
            .unwrap_or_else(|| self.agent_computer_root(target));
        let extension_dir = agent_root
            .join("extensions")
            .join(&manifest.id)
            .join(&manifest.version);
        std::fs::create_dir_all(&extension_dir).map_err(|e| {
            format!(
                "Failed to create extension directory '{}': {e}",
                extension_dir.display()
            )
        })?;

        let dest_wasm_path = extension_dir.join("module.wasm");
        std::fs::copy(&source_path, &dest_wasm_path).map_err(|e| {
            format!(
                "Failed to copy extension module from '{}' to '{}': {e}",
                source_path.display(),
                dest_wasm_path.display()
            )
        })?;

        manifest.entry = dest_wasm_path.display().to_string();

        self.upsert_installed(target, &manifest, "active")?;
        self.bind_capabilities(
            target,
            &manifest.id,
            &manifest.version,
            &manifest.capabilities,
        )?;

        let new_revision = self.bump_revision(target, "ready")?;
        let txn = ExtensionTxn {
            txn_id: uuid::Uuid::new_v4().to_string(),
            actor_agent_id: actor,
            scope: TxnScope::AgentLocal,
            action,
            target_agent_id: Some(target),
            extension_id: manifest.id.clone(),
            base_revision,
            new_revision: Some(new_revision),
            status: TxnStatus::Succeeded,
            error: None,
            timestamp: Utc::now(),
        };
        self.record_txn(&txn)?;

        match action {
            TxnAction::Install => self.record_event(NitroEvent::ExtensionInstalled {
                agent_id: target,
                extension_id: manifest.id.clone(),
                version: manifest.version.clone(),
            })?,
            TxnAction::Update => self.record_event(NitroEvent::ExtensionUpdated {
                agent_id: target,
                extension_id: manifest.id.clone(),
                from_version: "previous".to_string(),
                to_version: manifest.version.clone(),
            })?,
            _ => {}
        }

        for cap in &manifest.capabilities {
            self.record_event(NitroEvent::CapabilityBound {
                agent_id: target,
                capability: cap.name.clone(),
                extension_id: manifest.id.clone(),
                version: manifest.version.clone(),
            })?;
        }

        Ok(txn)
    }

    pub fn remove_extension(
        &self,
        actor: AgentId,
        target: AgentId,
        req: RemoveExtensionRequest,
    ) -> Result<ExtensionTxn, String> {
        let current_revision = self.current_revision(target)?;
        if current_revision != req.base_revision {
            let txn = ExtensionTxn {
                txn_id: uuid::Uuid::new_v4().to_string(),
                actor_agent_id: actor,
                scope: TxnScope::AgentLocal,
                action: TxnAction::Remove,
                target_agent_id: Some(target),
                extension_id: req.extension_id.clone(),
                base_revision: req.base_revision,
                new_revision: None,
                status: TxnStatus::Conflict,
                error: Some(format!(
                    "Revision conflict: expected base revision {}, actual {}",
                    req.base_revision, current_revision
                )),
                timestamp: Utc::now(),
            };
            self.record_txn(&txn)?;
            self.record_event(NitroEvent::RevisionConflict {
                agent_id: target,
                extension_id: req.extension_id,
                expected_base_revision: req.base_revision,
                actual_revision: current_revision,
            })?;
            return Ok(txn);
        }

        let unbound = self.unbind_capabilities(target, &req.extension_id)?;
        self.remove_installed(target, &req.extension_id)?;

        let new_revision = self.bump_revision(target, "ready")?;
        let txn = ExtensionTxn {
            txn_id: uuid::Uuid::new_v4().to_string(),
            actor_agent_id: actor,
            scope: TxnScope::AgentLocal,
            action: TxnAction::Remove,
            target_agent_id: Some(target),
            extension_id: req.extension_id.clone(),
            base_revision: req.base_revision,
            new_revision: Some(new_revision),
            status: TxnStatus::Succeeded,
            error: None,
            timestamp: Utc::now(),
        };
        self.record_txn(&txn)?;
        self.record_event(NitroEvent::ExtensionRemoved {
            agent_id: target,
            extension_id: req.extension_id.clone(),
        })?;
        for capability in unbound {
            self.record_event(NitroEvent::CapabilityUnbound {
                agent_id: target,
                capability,
                extension_id: req.extension_id.clone(),
            })?;
        }

        Ok(txn)
    }

    pub fn publish_extension(
        &self,
        actor: AgentId,
        target: AgentId,
        req: PublishExtensionRequest,
    ) -> Result<ExtensionTxn, String> {
        let current_revision = self.current_revision(target)?;
        if current_revision != req.base_revision {
            let txn = ExtensionTxn {
                txn_id: uuid::Uuid::new_v4().to_string(),
                actor_agent_id: actor,
                scope: TxnScope::GlobalPublish,
                action: TxnAction::Publish,
                target_agent_id: Some(target),
                extension_id: req.extension_id.clone(),
                base_revision: req.base_revision,
                new_revision: None,
                status: TxnStatus::Conflict,
                error: Some(format!(
                    "Revision conflict: expected base revision {}, actual {}",
                    req.base_revision, current_revision
                )),
                timestamp: Utc::now(),
            };
            self.record_txn(&txn)?;
            self.record_event(NitroEvent::RevisionConflict {
                agent_id: target,
                extension_id: req.extension_id,
                expected_base_revision: req.base_revision,
                actual_revision: current_revision,
            })?;
            return Ok(txn);
        }

        let (manifest_json, entry_path): (String, String) = {
            let conn = self.lock_conn();
            let mut stmt = conn
                .prepare(
                    "SELECT local_manifest_json FROM nitro_extensions_installed
                     WHERE agent_id = ?1 AND extension_id = ?2",
                )
                .map_err(|e| format!("Failed to prepare publish query: {e}"))?;
            let manifest_json: String = stmt
                .query_row(
                    params![target.to_string(), req.extension_id],
                    |row: &rusqlite::Row<'_>| row.get::<_, String>(0),
                )
                .map_err(|e| format!("Extension not installed for publish: {e}"))?;
            let manifest: openfang_types::nitro::NitroExtensionManifest =
                serde_json::from_str(&manifest_json)
                    .map_err(|e| format!("Stored extension manifest is invalid JSON: {e}"))?;
            (manifest_json, manifest.entry)
        };

        let source = PathBuf::from(&entry_path);
        if !source.exists() {
            return Err(format!(
                "Cannot publish extension '{}': source WASM module is missing at '{}'.",
                req.extension_id,
                source.display()
            ));
        }

        let bytes = std::fs::read(&source).map_err(|e| {
            format!(
                "Failed to read local extension artifact '{}': {e}",
                source.display()
            )
        })?;
        let wasm_sha256 = hex::encode(Sha256::digest(&bytes));

        let global_dir = self
            .global_extensions_dir()
            .join(&req.extension_id)
            .join(&req.version);
        std::fs::create_dir_all(&global_dir).map_err(|e| {
            format!(
                "Failed to create global extension directory '{}': {e}",
                global_dir.display()
            )
        })?;
        let global_module = global_dir.join("module.wasm");
        std::fs::write(&global_module, &bytes).map_err(|e| {
            format!(
                "Failed to write global extension module '{}': {e}",
                global_module.display()
            )
        })?;

        {
            let conn = self.lock_conn();
            conn.execute(
                "INSERT OR REPLACE INTO nitro_extensions_global (
                    id, version, manifest_json, wasm_sha256, artifact_path, published_by, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    req.extension_id,
                    req.version,
                    manifest_json,
                    wasm_sha256,
                    global_module.display().to_string(),
                    actor.to_string(),
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(|e| format!("Failed to persist global extension publish: {e}"))?;
        }

        let new_revision = self.bump_revision(target, "ready")?;
        let txn = ExtensionTxn {
            txn_id: uuid::Uuid::new_v4().to_string(),
            actor_agent_id: actor,
            scope: TxnScope::GlobalPublish,
            action: TxnAction::Publish,
            target_agent_id: Some(target),
            extension_id: req.extension_id.clone(),
            base_revision: req.base_revision,
            new_revision: Some(new_revision),
            status: TxnStatus::Succeeded,
            error: None,
            timestamp: Utc::now(),
        };
        self.record_txn(&txn)?;
        self.record_event(NitroEvent::ExtensionPublished {
            agent_id: target,
            extension_id: req.extension_id,
            version: req.version,
        })?;

        Ok(txn)
    }

    pub async fn execute_capability(
        &self,
        agent_id: AgentId,
        capability: &str,
        args: &serde_json::Value,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
    ) -> Result<String, String> {
        let (extension_id, _version, manifest_json): (String, String, String) = {
            let conn = self.lock_conn();
            let mut stmt = conn
                .prepare(
                    "SELECT b.extension_id, b.version, i.local_manifest_json
                     FROM nitro_capability_bindings b
                     JOIN nitro_extensions_installed i
                       ON i.agent_id = b.agent_id AND i.extension_id = b.extension_id
                     WHERE b.agent_id = ?1 AND b.capability_name = ?2",
                )
                .map_err(|e| format!("Failed to prepare execute_capability query: {e}"))?;
            stmt.query_row(
                params![agent_id.to_string(), capability],
                |row: &rusqlite::Row<'_>| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|e| format!("Capability '{}' is not bound: {e}", capability))?
        };

        let manifest: openfang_types::nitro::NitroExtensionManifest =
            serde_json::from_str(&manifest_json).map_err(|e| {
                format!(
                    "Invalid stored extension manifest for '{}': {e}",
                    extension_id
                )
            })?;

        let payload = serde_json::json!({
            "capability": capability,
            "args": args,
            "agent_id": agent_id.to_string(),
            "workspace": self.agent_computer_root(agent_id).display().to_string(),
        });

        let host = openfang_runtime::nitro_host::NitroExtensionHost::new();
        match host
            .execute_manifest(&manifest, payload, kernel_handle, &agent_id.to_string())
            .await
        {
            Ok(value) => Ok(value.to_string()),
            Err(error) => {
                let _ = self.record_event(NitroEvent::ExtensionExecutionFailed {
                    agent_id,
                    extension_id,
                    capability: capability.to_string(),
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }
}
