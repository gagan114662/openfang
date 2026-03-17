use chrono::Utc;
use openfang_kernel::OpenFangKernel;
use openfang_types::config::KernelConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ArtifactPaths {
    pub repo_root: PathBuf,
    pub autonomy_dir: PathBuf,
    pub current_state_path: PathBuf,
    pub deploy_history_path: PathBuf,
    pub triage_latest_path: PathBuf,
    pub ops_latest_path: PathBuf,
    pub daily_brief_latest_path: PathBuf,
    pub workload_registry_path: PathBuf,
    pub safety_policy_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AutonomyStateDocument {
    pub generated_at: String,
    pub primary_host: String,
    pub telegram_owner: String,
    pub primary_sentry_browser_host: String,
    pub fallback_sentry_browser_host: String,
    pub ops_agent: Option<Value>,
    pub ops_agent_budget: Option<Value>,
    pub local_daemon: Value,
    pub remote_daemon: Value,
    pub browser_profiles: Value,
    pub last_guard_heartbeat: Option<Value>,
    pub last_guard_trace: Option<Value>,
    pub last_guard_outcome: Option<String>,
    pub last_triage_run: Option<Value>,
    pub last_triage_status: Option<String>,
    pub last_autofix_run: Option<Value>,
    pub last_autofix_status: Option<String>,
    pub last_deploy: Option<Value>,
    pub last_auth_escalation: Option<Value>,
    pub workloads: Vec<Value>,
    pub telegram: Value,
    pub blockers: Vec<String>,
    pub workload_registry: Value,
    pub safety_policy: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeployHistoryEntry {
    pub timestamp: String,
    pub kind: String,
    pub sha: Option<String>,
    pub target: Option<String>,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub remediation_action: Option<String>,
    pub remediation_result: Option<String>,
    pub request_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub payload: Value,
}

pub fn artifact_paths(config: &KernelConfig) -> ArtifactPaths {
    let repo_root = config
        .autonomy
        .artifacts_dir
        .as_ref()
        .and_then(|path| {
            if path.ends_with("artifacts/autonomy") {
                path.parent().and_then(Path::parent).map(Path::to_path_buf)
            } else {
                None
            }
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| config.home_dir.clone());

    let autonomy_dir = config
        .autonomy
        .artifacts_dir
        .clone()
        .unwrap_or_else(|| repo_root.join("artifacts").join("autonomy"));

    let workload_registry_path = config
        .autonomy
        .workload_registry_path
        .clone()
        .unwrap_or_else(|| repo_root.join("config").join("unattended_workloads.toml"));

    let safety_policy_path = config
        .autonomy
        .safety_policy_path
        .clone()
        .unwrap_or_else(|| {
            repo_root
                .join("docs")
                .join("ops")
                .join("autofix-safety-policy.md")
        });

    ArtifactPaths {
        repo_root,
        current_state_path: autonomy_dir.join("current-state.json"),
        deploy_history_path: autonomy_dir.join("deploy-history.jsonl"),
        triage_latest_path: autonomy_dir.join("triage-latest.md"),
        ops_latest_path: autonomy_dir.join("ops-latest.md"),
        daily_brief_latest_path: autonomy_dir.join("daily-brief-latest.md"),
        autonomy_dir,
        workload_registry_path,
        safety_policy_path,
    }
}

pub fn ensure_artifact_scaffold(config: &KernelConfig) -> std::io::Result<ArtifactPaths> {
    let paths = artifact_paths(config);
    fs::create_dir_all(&paths.autonomy_dir)?;
    if let Some(parent) = paths.workload_registry_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = paths.safety_policy_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if !paths.current_state_path.exists() {
        write_json(
            &paths.current_state_path,
            &default_state_document(config, &paths),
        )?;
    }
    if !paths.deploy_history_path.exists() {
        fs::write(&paths.deploy_history_path, b"")?;
    }
    if !paths.triage_latest_path.exists() {
        fs::write(
            &paths.triage_latest_path,
            "# Latest Triage\n\nNo triage cycle has reported yet.\n",
        )?;
    }
    if !paths.daily_brief_latest_path.exists() {
        fs::write(
            &paths.daily_brief_latest_path,
            "# Daily Brief\n\nNo unattended daily brief has been generated yet.\n",
        )?;
    }
    Ok(paths)
}

pub fn default_state_document(
    config: &KernelConfig,
    paths: &ArtifactPaths,
) -> AutonomyStateDocument {
    let workload_registry = if paths.workload_registry_path.exists() {
        json!({
            "path": paths.workload_registry_path,
            "present": true,
        })
    } else {
        json!({
            "path": paths.workload_registry_path,
            "present": false,
        })
    };
    let safety_policy = json!({
        "path": paths.safety_policy_path,
        "present": paths.safety_policy_path.exists(),
    });

    AutonomyStateDocument {
        generated_at: Utc::now().to_rfc3339(),
        primary_host: config.autonomy.primary_host.clone(),
        telegram_owner: config.autonomy.primary_host.clone(),
        primary_sentry_browser_host: config.autonomy.primary_sentry_browser_host.clone(),
        fallback_sentry_browser_host: config.autonomy.fallback_sentry_browser_host.clone(),
        ops_agent: None,
        ops_agent_budget: None,
        local_daemon: json!({
            "pid": std::process::id(),
            "listen_addr": config.api_listen.clone(),
            "state": "running",
        }),
        remote_daemon: json!({
            "host": config.autonomy.primary_host.clone(),
            "state": "unknown",
        }),
        browser_profiles: browser_profiles_summary(config),
        last_guard_heartbeat: None,
        last_guard_trace: None,
        last_guard_outcome: None,
        last_triage_run: None,
        last_triage_status: None,
        last_autofix_run: None,
        last_autofix_status: None,
        last_deploy: None,
        last_auth_escalation: None,
        workloads: Vec::new(),
        telegram: telegram_summary(config),
        blockers: Vec::new(),
        workload_registry,
        safety_policy,
    }
}

pub fn load_state_document(config: &KernelConfig) -> AutonomyStateDocument {
    let paths = match ensure_artifact_scaffold(config) {
        Ok(paths) => paths,
        Err(_) => return default_state_document(config, &artifact_paths(config)),
    };
    match fs::read_to_string(&paths.current_state_path) {
        Ok(raw) => {
            serde_json::from_str(&raw).unwrap_or_else(|_| default_state_document(config, &paths))
        }
        Err(_) => default_state_document(config, &paths),
    }
}

pub fn update_state_document<F>(
    config: &KernelConfig,
    mutator: F,
) -> std::io::Result<AutonomyStateDocument>
where
    F: FnOnce(&mut AutonomyStateDocument),
{
    let paths = ensure_artifact_scaffold(config)?;
    let mut state = load_state_document(config);
    mutator(&mut state);
    state.generated_at = Utc::now().to_rfc3339();
    state.workload_registry = registry_summary(&paths.workload_registry_path);
    state.browser_profiles = browser_profiles_summary(config);
    state.telegram = telegram_summary(config);
    state.safety_policy = json!({
        "path": paths.safety_policy_path,
        "present": paths.safety_policy_path.exists(),
    });
    write_json(&paths.current_state_path, &state)?;
    Ok(state)
}

pub fn append_deploy_history(
    config: &KernelConfig,
    entry: &DeployHistoryEntry,
) -> std::io::Result<()> {
    let paths = ensure_artifact_scaffold(config)?;
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(paths.deploy_history_path)?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string())
    )?;
    Ok(())
}

pub fn update_triage_markdown(config: &KernelConfig, markdown: &str) -> std::io::Result<()> {
    let paths = ensure_artifact_scaffold(config)?;
    fs::write(paths.triage_latest_path, markdown)
}

pub fn hydrate_live_state(kernel: &OpenFangKernel, doc: &mut AutonomyStateDocument) {
    doc.browser_profiles = browser_profiles_summary(&kernel.config);
    doc.telegram = telegram_summary(&kernel.config);
    doc.workloads = live_workloads(kernel);
    doc.ops_agent = kernel
        .registry
        .list()
        .into_iter()
        .find(|entry| entry.name == kernel.config.autonomy.ops_agent_name)
        .map(|entry| {
            json!({
                "id": entry.id.to_string(),
                "name": entry.name,
                "state": format!("{:?}", entry.state),
                "provider": entry.manifest.model.provider,
                "model": entry.manifest.model.model,
                "tools": entry.manifest.capabilities.tools,
                "last_active": entry.last_active.to_rfc3339(),
            })
        });
    if let Some(agent) = doc.ops_agent.as_ref() {
        if let Some(agent_id) = agent.get("id").and_then(Value::as_str) {
            let usage = openfang_memory::usage::UsageStore::new(kernel.memory.usage_conn());
            if let Ok(uuid) = agent_id.parse() {
                let agent_id = openfang_types::agent::AgentId(uuid);
                if let Some(entry) = kernel.registry.get(agent_id) {
                    doc.ops_agent_budget = Some(json!({
                        "hourly": {
                            "spend": usage.query_hourly(agent_id).unwrap_or(0.0),
                            "limit": entry.manifest.resources.max_cost_per_hour_usd,
                        },
                        "daily": {
                            "spend": usage.query_daily(agent_id).unwrap_or(0.0),
                            "limit": entry.manifest.resources.max_cost_per_day_usd,
                        },
                        "monthly": {
                            "spend": usage.query_monthly(agent_id).unwrap_or(0.0),
                            "limit": entry.manifest.resources.max_cost_per_month_usd,
                        },
                    }));
                }
            }
        }
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    fs::write(path, bytes)
}

fn registry_summary(path: &Path) -> Value {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            return json!({
                "path": path,
                "present": false,
                "workload_count": 0,
            });
        }
    };
    match raw.parse::<toml::Value>() {
        Ok(value) => {
            let workload_count = value
                .get("workloads")
                .and_then(|v| v.as_array())
                .map(|items| items.len())
                .unwrap_or(0);
            json!({
                "path": path,
                "present": true,
                "workload_count": workload_count,
            })
        }
        Err(_) => json!({
            "path": path,
            "present": true,
            "parse_error": true,
        }),
    }
}

fn browser_profiles_summary(config: &KernelConfig) -> Value {
    let primary_path = config.browser.user_data_dir.clone().unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}/.openfang/browser/profiles/sentry-primary",
            std::env::var("HOME").unwrap_or_default()
        ))
    });
    let fallback_path = if config.autonomy.fallback_sentry_browser_host == "mac" {
        PathBuf::from("/Users/gaganarora/.openfang/browser/profiles/sentry-fallback")
    } else {
        PathBuf::from(format!(
            "{}/.openfang/browser/profiles/sentry-fallback",
            std::env::var("HOME").unwrap_or_default()
        ))
    };

    json!({
        "primary": {
            "host": config.autonomy.primary_sentry_browser_host,
            "path": primary_path,
            "present": primary_path.exists(),
        },
        "fallback": {
            "host": config.autonomy.fallback_sentry_browser_host,
            "path": fallback_path,
            "present": fallback_path.exists(),
        },
    })
}

fn telegram_summary(config: &KernelConfig) -> Value {
    let runtime = openfang_channels::telegram::runtime_status();
    let owner_host = runtime
        .owner_host
        .clone()
        .or_else(|| std::env::var("OPENFANG_TELEGRAM_OWNER").ok())
        .unwrap_or_else(|| config.autonomy.primary_host.clone());
    let admin_chat_id_present = config
        .channels
        .telegram
        .as_ref()
        .and_then(|cfg| cfg.admin_chat_id)
        .is_some()
        || std::env::var("OPENFANG_ADMIN_TELEGRAM_CHAT_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_some()
        || std::env::var("TELEGRAM_ADMIN_CHAT_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_some();

    json!({
        "owner_host": owner_host,
        "admin_chat_id_present": admin_chat_id_present,
        "outbox_depth": runtime.outbox_depth,
        "active_socket_count": runtime.active_socket_count,
        "last_send_failure": runtime.last_send_failure,
        "last_send_success": runtime.last_send_success,
        "last_poll_success": runtime.last_poll_success,
        "polling_conflict": runtime.polling_conflict,
        "rate_limited_until": runtime.rate_limited_until,
    })
}

fn live_workloads(kernel: &OpenFangKernel) -> Vec<Value> {
    let shared_id = openfang_types::agent::AgentId(uuid::Uuid::from_bytes([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01,
    ]));
    match kernel
        .memory
        .structured_get(shared_id, "__openfang_schedules")
        .ok()
        .flatten()
    {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfang_types::config::KernelConfig;
    use tempfile::tempdir;

    #[test]
    fn test_ensure_artifact_scaffold_writes_defaults() {
        let tmp = tempdir().unwrap();
        let mut config = KernelConfig::default();
        config.autonomy.enabled = true;
        config.autonomy.artifacts_dir = Some(tmp.path().join("artifacts").join("autonomy"));
        config.autonomy.workload_registry_path =
            Some(tmp.path().join("config").join("workloads.toml"));
        config.autonomy.safety_policy_path =
            Some(tmp.path().join("docs").join("ops").join("policy.md"));

        let paths = ensure_artifact_scaffold(&config).unwrap();
        assert!(paths.current_state_path.exists());
        assert!(paths.deploy_history_path.exists());
        assert!(paths.triage_latest_path.exists());
        assert!(paths.daily_brief_latest_path.exists());
    }

    #[test]
    fn test_update_state_document_persists_blockers() {
        let tmp = tempdir().unwrap();
        let mut config = KernelConfig::default();
        config.autonomy.enabled = true;
        config.autonomy.artifacts_dir = Some(tmp.path().join("artifacts").join("autonomy"));
        config.autonomy.workload_registry_path =
            Some(tmp.path().join("config").join("workloads.toml"));
        config.autonomy.safety_policy_path =
            Some(tmp.path().join("docs").join("ops").join("policy.md"));

        update_state_document(&config, |state| {
            state
                .blockers
                .push("sentry browser unavailable".to_string());
        })
        .unwrap();

        let state = load_state_document(&config);
        assert_eq!(state.blockers, vec!["sentry browser unavailable"]);
    }
}
