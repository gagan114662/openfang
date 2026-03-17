use super::computer::{parse_status, parse_ts, NitroComputerManager};
use chrono::Utc;
use openfang_types::agent::AgentId;
use openfang_types::config::{ComputerV2Config, RemoteVmHostConfig};
use openfang_types::nitro::{
    AgentComputerV2, ComputerActionRequest, ComputerActionResult, ComputerBackend,
    ComputerPlacementPolicy, ComputerResourcePolicy, ComputerResourceUpdateRequest, GpuCapability,
    GpuHealth, GpuMode, GuestOs, MemoryTierPolicy, NitroEvent,
};
use rusqlite::{params, OptionalExtension};
use std::process::Command;
use tracing;

impl NitroComputerManager {
    pub fn create_or_get_computer_v2(
        &self,
        agent_id: AgentId,
        placement_policy: ComputerPlacementPolicy,
        cfg: &ComputerV2Config,
    ) -> Result<AgentComputerV2, String> {
        if let Some(existing) = self.get_computer_v2(agent_id)? {
            return Ok(existing);
        }

        let legacy = self.bootstrap_computer(agent_id, None)?;
        let (backend, host) = derive_backend_host(cfg, &placement_policy);
        let resources = default_resources(cfg);
        let memory_tier_policy = MemoryTierPolicy::default();
        let gpu = GpuCapability {
            requested: placement_policy.gpu_required,
            available: false,
            mode: GpuMode::None,
            health: GpuHealth::Unknown,
        };
        let computer_id = format!("comp-{}", &agent_id.to_string()[..8]);
        let now = Utc::now();

        let conn = self.lock_conn();
        conn.execute(
            "INSERT OR REPLACE INTO agent_computers_v2 (
                agent_id, computer_id, status, backend, host, revision, guest_os,
                placement_policy_json, memory_tier_policy_json, gpu_json, root_path, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                agent_id.to_string(),
                computer_id,
                "booting",
                to_backend_str(backend),
                host.clone(),
                "linux",
                serde_json::to_string(&placement_policy)
                    .map_err(|e| format!("Failed to serialize placement policy: {e}"))?,
                serde_json::to_string(&memory_tier_policy)
                    .map_err(|e| format!("Failed to serialize memory tier policy: {e}"))?,
                serde_json::to_string(&gpu).map_err(|e| format!("Failed to serialize GPU: {e}"))?,
                legacy.root_path.display().to_string(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to insert computer_v2: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO agent_computer_resources (
                agent_id, vcpu, memory_mb, disk_gb, io_weight, net_profile, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                agent_id.to_string(),
                resources.vcpu as i64,
                resources.memory_mb as i64,
                resources.disk_gb as i64,
                resources.io_weight as i64,
                resources.net_profile,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to insert computer resources: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO agent_computer_host_affinity (agent_id, host, backend, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                agent_id.to_string(),
                host.clone(),
                to_backend_str(backend),
                now.to_rfc3339()
            ],
        )
        .map_err(|e| format!("Failed to insert host affinity: {e}"))?;
        drop(conn);

        self.record_event(NitroEvent::ComputerV2Created {
            agent_id,
            computer_id: format!("comp-{}", &agent_id.to_string()[..8]),
            backend,
        })?;
        let _ = self.record_v2_event(
            "computer_v2_created",
            serde_json::json!({"agent_id": agent_id.to_string(), "backend": to_backend_str(backend)}),
        );

        self.get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer v2 create succeeded but fetch failed".to_string())
    }

    pub fn get_computer_v2(&self, agent_id: AgentId) -> Result<Option<AgentComputerV2>, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT computer_id, status, backend, host, revision, guest_os,
                        placement_policy_json, memory_tier_policy_json, gpu_json, root_path, updated_at
                 FROM agent_computers_v2
                 WHERE agent_id = ?1",
            )
            .map_err(|e| format!("Failed to prepare get_computer_v2: {e}"))?;

        let row = stmt
            .query_row([agent_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            })
            .optional()
            .map_err(|e| format!("Failed to query computer_v2: {e}"))?;
        drop(stmt);
        drop(conn);

        let Some((
            computer_id,
            status_str,
            backend_str,
            host,
            revision,
            _guest_os,
            placement_json,
            memory_json,
            gpu_json,
            root_path,
            updated_at,
        )) = row
        else {
            return Ok(None);
        };

        let resources = self.get_resources_v2(agent_id)?;
        let placement_policy: ComputerPlacementPolicy = serde_json::from_str(&placement_json)
            .map_err(|e| format!("Invalid placement policy JSON: {e}"))?;
        let memory_tier_policy: MemoryTierPolicy = serde_json::from_str(&memory_json)
            .map_err(|e| format!("Invalid memory tier policy JSON: {e}"))?;
        let gpu: GpuCapability =
            serde_json::from_str(&gpu_json).map_err(|e| format!("Invalid GPU JSON: {e}"))?;

        Ok(Some(AgentComputerV2 {
            agent_id,
            computer_id,
            status: parse_status(&status_str),
            backend: parse_backend(&backend_str),
            host,
            revision: revision.max(0) as u64,
            resources,
            guest_os: GuestOs::Linux,
            placement_policy,
            memory_tier_policy,
            gpu,
            root_path: std::path::PathBuf::from(root_path),
            updated_at: parse_ts(&updated_at),
        }))
    }

    pub fn start_computer_v2(
        &self,
        agent_id: AgentId,
        cfg: &ComputerV2Config,
    ) -> Result<AgentComputerV2, String> {
        let mut computer =
            self.create_or_get_computer_v2(agent_id, ComputerPlacementPolicy::default(), cfg)?;
        self.guard_capacity(&computer, cfg)?;

        if computer.backend == ComputerBackend::RemoteKvm {
            let host_cfg = cfg
                .remote_host
                .as_ref()
                .ok_or("computer_v2 remote backend selected but no remote_host configured")?;
            let gpu = self.gpu_preflight(host_cfg, computer.placement_policy.gpu_required)?;
            if gpu.health == GpuHealth::Degraded && computer.placement_policy.gpu_required {
                self.update_gpu_and_status(agent_id, &gpu, "degraded")?;
                self.record_event(NitroEvent::ComputerV2GpuDegraded {
                    agent_id,
                    computer_id: computer.computer_id.clone(),
                    reason: "GPU preflight failed".to_string(),
                })?;
                return Err(
                    "GPU is degraded on remote host; cannot start gpu_required computer"
                        .to_string(),
                );
            }

            self.ensure_domain_defined(agent_id, &computer.resources, host_cfg, cfg)?;
            self.remote_vm_action(agent_id, host_cfg, "start")?;
            self.update_gpu_and_status(agent_id, &gpu, "ready")?;
        } else {
            self.update_status(agent_id, "ready")?;
        }

        self.record_event(NitroEvent::ComputerV2Started {
            agent_id,
            computer_id: computer.computer_id.clone(),
        })?;
        let _ = self.record_v2_event(
            "computer_v2_started",
            serde_json::json!({"agent_id": agent_id.to_string(), "computer_id": computer.computer_id}),
        );
        computer = self
            .get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer missing after start".to_string())?;
        Ok(computer)
    }

    pub fn suspend_computer_v2(
        &self,
        agent_id: AgentId,
        cfg: &ComputerV2Config,
    ) -> Result<AgentComputerV2, String> {
        let computer = self
            .get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer v2 not found".to_string())?;

        if computer.backend == ComputerBackend::RemoteKvm {
            let host_cfg = cfg
                .remote_host
                .as_ref()
                .ok_or("remote_host config missing")?;
            self.remote_vm_action(agent_id, host_cfg, "suspend")?;
        }
        self.update_status(agent_id, "recovering")?;
        self.record_event(NitroEvent::ComputerV2Suspended {
            agent_id,
            computer_id: computer.computer_id.clone(),
        })?;
        self.get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer missing after suspend".to_string())
    }

    pub fn stop_computer_v2(
        &self,
        agent_id: AgentId,
        cfg: &ComputerV2Config,
    ) -> Result<AgentComputerV2, String> {
        let computer = self
            .get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer v2 not found".to_string())?;
        if computer.backend == ComputerBackend::RemoteKvm {
            let host_cfg = cfg
                .remote_host
                .as_ref()
                .ok_or("remote_host config missing")?;
            self.remote_vm_action(agent_id, host_cfg, "stop")?;
        }
        self.update_status(agent_id, "booting")?;
        self.record_event(NitroEvent::ComputerV2Stopped {
            agent_id,
            computer_id: computer.computer_id.clone(),
        })?;
        self.get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer missing after stop".to_string())
    }

    pub fn rebuild_computer_v2(
        &self,
        agent_id: AgentId,
        cfg: &ComputerV2Config,
    ) -> Result<AgentComputerV2, String> {
        let computer = self
            .get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer v2 not found".to_string())?;
        if computer.backend == ComputerBackend::RemoteKvm {
            let host_cfg = cfg
                .remote_host
                .as_ref()
                .ok_or("remote_host config missing")?;
            self.remote_vm_action(agent_id, host_cfg, "rebuild")?;
        }
        self.update_status(agent_id, "ready")?;
        self.record_event(NitroEvent::ComputerV2Rebuilt {
            agent_id,
            computer_id: computer.computer_id.clone(),
        })?;
        self.get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer missing after rebuild".to_string())
    }

    pub fn set_resources_v2(
        &self,
        agent_id: AgentId,
        req: ComputerResourceUpdateRequest,
    ) -> Result<AgentComputerV2, String> {
        let current = self
            .get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer v2 not found".to_string())?;
        if current.revision != req.base_revision {
            return Err(format!(
                "Revision conflict: expected {}, got {}",
                req.base_revision, current.revision
            ));
        }

        let now = Utc::now().to_rfc3339();
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE agent_computer_resources
             SET vcpu = ?2, memory_mb = ?3, disk_gb = ?4, io_weight = ?5, net_profile = ?6, updated_at = ?7
             WHERE agent_id = ?1",
            params![
                agent_id.to_string(),
                req.resources.vcpu as i64,
                req.resources.memory_mb as i64,
                req.resources.disk_gb as i64,
                req.resources.io_weight as i64,
                req.resources.net_profile,
                now,
            ],
        )
        .map_err(|e| format!("Failed to update resources: {e}"))?;
        conn.execute(
            "UPDATE agent_computers_v2 SET revision = revision + 1, updated_at = ?2 WHERE agent_id = ?1",
            params![agent_id.to_string(), now],
        )
        .map_err(|e| format!("Failed to bump revision on resources update: {e}"))?;
        drop(conn);

        self.get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer missing after resources update".to_string())
    }

    pub fn get_resources_v2(&self, agent_id: AgentId) -> Result<ComputerResourcePolicy, String> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare(
                "SELECT vcpu, memory_mb, disk_gb, io_weight, net_profile
                 FROM agent_computer_resources
                 WHERE agent_id = ?1",
            )
            .map_err(|e| format!("Failed to prepare get_resources_v2: {e}"))?;
        let row = stmt
            .query_row([agent_id.to_string()], |row| {
                Ok(ComputerResourcePolicy {
                    vcpu: row.get::<_, i64>(0)?.max(1) as u32,
                    memory_mb: row.get::<_, i64>(1)?.max(256) as u32,
                    disk_gb: row.get::<_, i64>(2)?.max(1) as u32,
                    io_weight: row.get::<_, i64>(3)?.max(1) as u32,
                    net_profile: row.get(4)?,
                })
            })
            .optional()
            .map_err(|e| format!("Failed to query resources: {e}"))?;
        Ok(row.unwrap_or_default())
    }

    pub async fn exec_action_v2(
        &self,
        agent_id: AgentId,
        action: ComputerActionRequest,
        cfg: &ComputerV2Config,
    ) -> Result<ComputerActionResult, String> {
        let computer = self
            .get_computer_v2(agent_id)?
            .ok_or_else(|| "Computer v2 not found".to_string())?;
        let timeout_secs = if action.timeout_secs == 0 {
            60
        } else {
            action.timeout_secs
        };

        let output = if computer.backend == ComputerBackend::RemoteKvm {
            let host_cfg = cfg
                .remote_host
                .as_ref()
                .ok_or("remote_host config missing")?;
            let command = action
                .payload
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("action payload requires a 'command' string")?;
            run_remote_command_async(host_cfg, command, timeout_secs).await?
        } else {
            let command = action
                .payload
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("action payload requires a 'command' string")?;
            run_local_command(command)?
        };

        Ok(ComputerActionResult {
            action_type: action.action_type,
            ok: true,
            output,
        })
    }

    fn update_status(&self, agent_id: AgentId, status: &str) -> Result<(), String> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE agent_computers_v2
             SET status = ?2, revision = revision + 1, updated_at = ?3
             WHERE agent_id = ?1",
            params![agent_id.to_string(), status, Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to update computer_v2 status: {e}"))?;
        Ok(())
    }

    fn update_gpu_and_status(
        &self,
        agent_id: AgentId,
        gpu: &GpuCapability,
        status: &str,
    ) -> Result<(), String> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE agent_computers_v2
             SET gpu_json = ?2, status = ?3, revision = revision + 1, updated_at = ?4
             WHERE agent_id = ?1",
            params![
                agent_id.to_string(),
                serde_json::to_string(gpu).map_err(|e| format!("Failed to serialize GPU: {e}"))?,
                status,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to update gpu/status: {e}"))?;
        Ok(())
    }

    fn guard_capacity(
        &self,
        computer: &AgentComputerV2,
        cfg: &ComputerV2Config,
    ) -> Result<(), String> {
        if computer.backend == ComputerBackend::LocalAppleVf && cfg.local_active_vms_default == 0 {
            return Err("Local fallback VM budget is 0; refusing to start local VM".to_string());
        }
        if computer.backend == ComputerBackend::RemoteKvm {
            let host_cfg = cfg
                .remote_host
                .as_ref()
                .ok_or("remote_host config missing")?;
            let running = self.remote_running_vms(host_cfg)?;
            let max_allowed = cfg.remote_active_vms_default.min(host_cfg.max_active_vms);
            if running >= max_allowed as usize {
                return Err(format!(
                    "Remote VM capacity reached ({running}/{max_allowed}); backpressure engaged"
                ));
            }
        }
        Ok(())
    }

    fn remote_running_vms(&self, host_cfg: &RemoteVmHostConfig) -> Result<usize, String> {
        let cmd = format!(
            "virsh --connect {} list --state-running --name | sed '/^$/d' | wc -l",
            shell_quote(&host_cfg.libvirt_uri)
        );
        let output = run_remote_command(host_cfg, &cmd)?;
        output
            .trim()
            .parse::<usize>()
            .map_err(|e| format!("Failed to parse remote VM count '{}': {e}", output.trim()))
    }

    fn gpu_preflight(
        &self,
        host_cfg: &RemoteVmHostConfig,
        requested: bool,
    ) -> Result<GpuCapability, String> {
        let out = run_remote_command(host_cfg, "nvidia-smi -L 2>/dev/null || true")?;
        let available = !out.trim().is_empty() && !out.to_lowercase().contains("failed");
        let health = if available {
            GpuHealth::Ready
        } else {
            GpuHealth::Degraded
        };
        let mode = if available {
            GpuMode::Shared
        } else {
            GpuMode::None
        };

        let conn = self.lock_conn();
        conn.execute(
            "INSERT OR REPLACE INTO host_gpu_health (host, available, health, details, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                host_cfg.host,
                if available { 1 } else { 0 },
                to_gpu_health_str(health),
                out,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to persist GPU health: {e}"))?;

        Ok(GpuCapability {
            requested,
            available,
            mode,
            health,
        })
    }

    fn ensure_domain_defined(
        &self,
        agent_id: AgentId,
        resources: &ComputerResourcePolicy,
        host_cfg: &RemoteVmHostConfig,
        cfg: &ComputerV2Config,
    ) -> Result<(), String> {
        let vm_name = vm_name(agent_id);
        let check_cmd = format!(
            "virsh --connect {} dominfo {} >/dev/null 2>&1",
            shell_quote(&host_cfg.libvirt_uri),
            shell_quote(&vm_name)
        );
        if run_remote_command(host_cfg, &format!("{check_cmd}; echo $?"))?
            .trim()
            .ends_with('0')
        {
            return Ok(());
        }

        if !cfg.auto_provision {
            return Err(format!(
                "VM '{}' is undefined on remote host and auto_provision=false",
                vm_name
            ));
        }
        let base_image = cfg
            .default_base_image
            .as_ref()
            .ok_or("auto_provision requires computer_v2.default_base_image")?;

        let vm_dir = format!("{}/{}", host_cfg.vm_storage_dir, vm_name);
        let disk_path = format!("{}/disk.qcow2", vm_dir);
        let provision_cmd = format!(
            "set -euo pipefail; \
             mkdir -p {vm_dir_q}; \
             qemu-img create -f qcow2 -F qcow2 -b {base_q} {disk_q}; \
             virt-install --connect {uri_q} --name {name_q} --memory {mem} --vcpus {vcpu} \
               --disk path={disk_q},format=qcow2,bus=virtio --import \
               --os-variant ubuntu22.04 --network network={net_q} --graphics none --noautoconsole",
            vm_dir_q = shell_quote(&vm_dir),
            base_q = shell_quote(base_image),
            disk_q = shell_quote(&disk_path),
            uri_q = shell_quote(&host_cfg.libvirt_uri),
            name_q = shell_quote(&vm_name),
            mem = resources.memory_mb,
            vcpu = resources.vcpu,
            net_q = shell_quote(&host_cfg.default_network),
        );
        let _ = run_remote_command(host_cfg, &provision_cmd)?;
        Ok(())
    }

    fn remote_vm_action(
        &self,
        agent_id: AgentId,
        host_cfg: &RemoteVmHostConfig,
        action: &str,
    ) -> Result<(), String> {
        let vm = vm_name(agent_id);
        let cmd = match action {
            "start" => format!(
                "virsh --connect {} start {} >/dev/null 2>&1 || true",
                shell_quote(&host_cfg.libvirt_uri),
                shell_quote(&vm)
            ),
            "suspend" => format!(
                "virsh --connect {} suspend {} >/dev/null 2>&1 || true",
                shell_quote(&host_cfg.libvirt_uri),
                shell_quote(&vm)
            ),
            "stop" => format!(
                "virsh --connect {} shutdown {} >/dev/null 2>&1 || true; sleep 3; \
                 virsh --connect {} destroy {} >/dev/null 2>&1 || true",
                shell_quote(&host_cfg.libvirt_uri),
                shell_quote(&vm),
                shell_quote(&host_cfg.libvirt_uri),
                shell_quote(&vm)
            ),
            "rebuild" => format!(
                "virsh --connect {} destroy {} >/dev/null 2>&1 || true; \
                 virsh --connect {} undefine {} --nvram >/dev/null 2>&1 || true",
                shell_quote(&host_cfg.libvirt_uri),
                shell_quote(&vm),
                shell_quote(&host_cfg.libvirt_uri),
                shell_quote(&vm)
            ),
            _ => return Err(format!("Unsupported VM action: {action}")),
        };
        let _ = run_remote_command(host_cfg, &cmd)?;
        Ok(())
    }

    fn record_v2_event(&self, event_type: &str, payload: serde_json::Value) -> Result<(), String> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO agent_computer_events (event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3)",
            params![
                event_type,
                serde_json::to_string(&payload)
                    .map_err(|e| format!("Failed to serialize computer event payload: {e}"))?,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to record computer event: {e}"))?;
        Ok(())
    }

    pub fn list_v2_events_since(
        &self,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        let conn = self.lock_conn();
        let mut stmt = match conn.prepare(
            "SELECT id, event_type, payload_json, created_at
             FROM agent_computer_events
             WHERE id > ?1
             ORDER BY id ASC
             LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("no such table") {
                    return Ok(Vec::new());
                }
                return Err(format!("Failed to prepare list_v2_events_since: {e}"));
            }
        };

        let rows = stmt
            .query_map(params![after_id, limit as i64], |row| {
                let payload_str: String = row.get(2)?;
                let parsed: serde_json::Value = serde_json::from_str(&payload_str)
                    .unwrap_or_else(|_| serde_json::json!({"raw": payload_str}));
                Ok(serde_json::json!({
                    "id": row.get::<_, i64>(0)?,
                    "event_type": row.get::<_, String>(1)?,
                    "payload": parsed,
                    "created_at": row.get::<_, String>(3)?,
                }))
            })
            .map_err(|e| format!("Failed to iterate v2 events: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Failed to decode v2 event row: {e}"))?);
        }
        Ok(out)
    }
}

fn default_resources(cfg: &ComputerV2Config) -> ComputerResourcePolicy {
    ComputerResourcePolicy {
        vcpu: cfg.local_fallback_vm_budget_vcpu.max(1),
        memory_mb: cfg.local_fallback_vm_budget_memory_mb.max(512),
        disk_gb: 40,
        io_weight: 100,
        net_profile: "default".to_string(),
    }
}

fn derive_backend_host(
    cfg: &ComputerV2Config,
    placement_policy: &ComputerPlacementPolicy,
) -> (ComputerBackend, String) {
    if cfg.remote_primary && placement_policy.remote_primary {
        if let Some(remote) = &cfg.remote_host {
            return (ComputerBackend::RemoteKvm, remote.host.clone());
        }
    }
    (ComputerBackend::LocalAppleVf, "localhost".to_string())
}

fn parse_backend(s: &str) -> ComputerBackend {
    match s {
        "remote_kvm" => ComputerBackend::RemoteKvm,
        _ => ComputerBackend::LocalAppleVf,
    }
}

fn to_backend_str(backend: ComputerBackend) -> &'static str {
    match backend {
        ComputerBackend::RemoteKvm => "remote_kvm",
        ComputerBackend::LocalAppleVf => "local_apple_vf",
    }
}

fn to_gpu_health_str(health: GpuHealth) -> &'static str {
    match health {
        GpuHealth::Ready => "ready",
        GpuHealth::Degraded => "degraded",
        GpuHealth::Unknown => "unknown",
    }
}

fn vm_name(agent_id: AgentId) -> String {
    format!("openfang-{}", &agent_id.to_string()[..8])
}

fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn run_local_command(command: &str) -> Result<String, String> {
    let output = Command::new("zsh")
        .arg("-lc")
        .arg(command)
        .output()
        .map_err(|e| format!("Failed to execute local command: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

/// Build common SSH options from config (multiplexing, timeouts, keepalive).
fn build_ssh_options(host_cfg: &RemoteVmHostConfig) -> String {
    let mut opts = format!(
        "-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout={}",
        host_cfg.ssh_connect_timeout,
    );
    if host_cfg.ssh_server_alive_interval > 0 {
        opts.push_str(&format!(
            " -o ServerAliveInterval={} -o ServerAliveCountMax=3",
            host_cfg.ssh_server_alive_interval,
        ));
    }
    if host_cfg.ssh_multiplex {
        opts.push_str(
            " -o ControlMaster=auto -o ControlPath=/tmp/ssh-%r@%h:%p -o ControlPersist=600",
        );
    }
    opts
}

/// Build the full SSH command string from host config and remote command.
fn build_ssh_command(host_cfg: &RemoteVmHostConfig, command: &str) -> String {
    let target = format!("{}@{}", host_cfg.username, host_cfg.host);
    let ssh_opts = build_ssh_options(host_cfg);

    if let Some(password) = host_cfg.password.as_ref() {
        let ssh_cmd = format!(
            "ssh -p {} {} {} {}",
            host_cfg.port,
            ssh_opts,
            shell_quote(&target),
            shell_quote(command),
        );
        format!("sshpass -p {} {}", shell_quote(password), ssh_cmd)
    } else if let Some(key_path) = host_cfg.ssh_key_path.as_ref() {
        format!(
            "ssh -i {} -p {} {} {} {}",
            shell_quote(&key_path.display().to_string()),
            host_cfg.port,
            ssh_opts,
            shell_quote(&target),
            shell_quote(command),
        )
    } else {
        format!(
            "ssh -p {} {} {} {}",
            host_cfg.port,
            ssh_opts,
            shell_quote(&target),
            shell_quote(command),
        )
    }
}

/// Check if an SSH error is an auth failure (should not be retried).
fn is_ssh_auth_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("permission denied")
        || lower.contains("authentication failed")
        || lower.contains("no more authentication methods")
        || lower.contains("publickey,password")
}

fn run_remote_command(host_cfg: &RemoteVmHostConfig, command: &str) -> Result<String, String> {
    let final_cmd = build_ssh_command(host_cfg, command);
    let max_retries = host_cfg.ssh_retry_count;
    let base_backoff_secs = host_cfg.ssh_retry_backoff_secs as u64;

    for attempt in 0..=max_retries {
        let output = Command::new("zsh")
            .arg("-lc")
            .arg(&final_cmd)
            .output()
            .map_err(|e| format!("Failed to execute remote command: {e}"))?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if is_ssh_auth_error(&stderr) {
            return Err(format!("SSH authentication failed (not retrying): {stderr}"));
        }

        if attempt < max_retries {
            let backoff = base_backoff_secs * (1u64 << attempt);
            tracing::warn!(
                attempt = attempt + 1,
                max_retries,
                backoff_secs = backoff,
                "SSH command failed, retrying: {stderr}"
            );
            std::thread::sleep(std::time::Duration::from_secs(backoff));
        } else {
            return Err(format!("Remote command failed after {} attempts: {stderr}", max_retries + 1));
        }
    }

    Err("SSH retry loop exhausted".to_string())
}

async fn run_remote_command_async(
    host_cfg: &RemoteVmHostConfig,
    command: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    let final_cmd = build_ssh_command(host_cfg, command);
    let max_retries = host_cfg.ssh_retry_count;
    let base_backoff_secs = host_cfg.ssh_retry_backoff_secs as u64;

    for attempt in 0..=max_retries {
        let child = tokio::process::Command::new("zsh")
            .arg("-lc")
            .arg(&final_cmd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn remote command: {e}"))?;

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| format!("Remote action timed out after {timeout_secs}s"))?
        .map_err(|e| format!("Failed to wait remote command: {e}"))?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if is_ssh_auth_error(&stderr) {
            return Err(format!("SSH authentication failed (not retrying): {stderr}"));
        }

        if attempt < max_retries {
            let backoff = base_backoff_secs * (1u64 << attempt);
            tracing::warn!(
                attempt = attempt + 1,
                max_retries,
                backoff_secs = backoff,
                "SSH async command failed, retrying: {stderr}"
            );
            tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        } else {
            return Err(format!("Remote command failed after {} attempts: {stderr}", max_retries + 1));
        }
    }

    Err("SSH retry loop exhausted".to_string())
}
