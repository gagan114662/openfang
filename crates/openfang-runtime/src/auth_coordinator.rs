//! Runtime auth coordinator for unattended operation.

use crate::auth_escalation::{AuthEscalation, AuthFailure};
use crate::auth_persistence::OAuthEntry;
use crate::auth_preflight::{
    assemble_result, check_browser_profile, check_provider_keys, check_ssh_connectivity,
    PreflightCheck, PreflightStatus,
};
use crate::sentry_logs::{capture_structured_log, flatten_with_prefix};
use chrono::{DateTime, Utc};
use openfang_memory::MemorySubstrate;
use openfang_types::agent::AgentId;
use openfang_types::config::{BrowserConfig, KernelConfig};
use sentry::Level;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const AUTH_DEGRADED_KEY: &str = "__auth_degraded__";
const AUTH_OAUTH_PREFIX: &str = "auth.oauth.";
const AUTH_CREDENTIAL_PREFIX: &str = "auth.credentials.";
const DEFAULT_ESCALATION_COOLDOWN_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStoredCredential {
    pub service: String,
    pub credential: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradedAuthState {
    pub agent_id: String,
    pub service: String,
    pub error: String,
    pub retry_count: u32,
    pub first_failure_at: DateTime<Utc>,
    pub last_alert_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthReadinessStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenStatus {
    pub service: String,
    pub expires_at: Option<String>,
    pub refreshable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthPreflightReport {
    pub status: AuthReadinessStatus,
    pub generated_at: String,
    pub checks: Vec<PreflightCheck>,
    pub providers: Vec<PreflightCheck>,
    pub browser_profiles: Vec<PreflightCheck>,
    pub ssh: Vec<PreflightCheck>,
    pub tokens: Vec<TokenStatus>,
    pub degraded_services: Vec<DegradedAuthState>,
}

#[derive(Clone)]
pub struct AuthCoordinator {
    memory: Arc<MemorySubstrate>,
    browser: BrowserConfig,
    default_provider: String,
    default_api_key_env: String,
}

impl AuthCoordinator {
    pub fn new(memory: Arc<MemorySubstrate>, config: &KernelConfig) -> Self {
        Self {
            memory,
            browser: config.browser.clone(),
            default_provider: config.default_model.provider.clone(),
            default_api_key_env: config.default_model.api_key_env.clone(),
        }
    }

    pub async fn run_preflight(&self) -> AuthPreflightReport {
        let _ = self.memory.structured_cleanup_expired();

        let mut provider_envs = BTreeMap::new();
        if !self.default_api_key_env.trim().is_empty() {
            if let Ok(value) = std::env::var(&self.default_api_key_env) {
                if !value.trim().is_empty() {
                    provider_envs.insert(self.default_provider.clone(), value);
                }
            }
        }
        for (provider, env_name) in [
            ("openai", "OPENAI_API_KEY"),
            ("anthropic", "ANTHROPIC_API_KEY"),
            ("groq", "GROQ_API_KEY"),
        ] {
            if let Ok(value) = std::env::var(env_name) {
                if !value.trim().is_empty() {
                    provider_envs.entry(provider.to_string()).or_insert(value);
                }
            }
        }

        let provider_pairs = provider_envs.into_iter().collect::<Vec<_>>();
        let providers = check_provider_keys(&provider_pairs).await;

        let browser_profiles = match &self.browser.user_data_dir {
            Some(path) => vec![check_browser_profile(&path.to_string_lossy())],
            None => vec![PreflightCheck {
                name: "browser_profile".to_string(),
                status: PreflightStatus::Warn,
                ttl: None,
                details: Some("No persistent browser profile configured".to_string()),
                action_required: Some(
                    "Set browser.user_data_dir to preserve sessions across restarts".to_string(),
                ),
            }],
        };

        let ssh = if let Some((username, host, port)) = remote_ssh_target() {
            vec![check_ssh_connectivity(&host, port, &username).await]
        } else {
            vec![PreflightCheck {
                name: "ssh_connectivity".to_string(),
                status: PreflightStatus::Warn,
                ttl: None,
                details: Some("No REMOTE_HOST/OPENFANG_REMOTE_HOST configured".to_string()),
                action_required: Some(
                    "Set OPENFANG_REMOTE_HOST or REMOTE_HOST for unattended GPU checks".to_string(),
                ),
            }]
        };

        let tokens = self.list_oauth_tokens();
        let degraded_services = self.list_degraded().unwrap_or_default();

        let mut checks = Vec::new();
        checks.extend(providers.clone());
        checks.extend(browser_profiles.clone());
        checks.extend(ssh.clone());

        let assembled = assemble_result(checks.clone());
        let has_warn = checks
            .iter()
            .any(|check| check.status == PreflightStatus::Warn);
        let status = if checks
            .iter()
            .any(|check| check.status == PreflightStatus::Fail)
        {
            AuthReadinessStatus::Fail
        } else if has_warn {
            AuthReadinessStatus::Warn
        } else {
            AuthReadinessStatus::Pass
        };

        let report = AuthPreflightReport {
            status,
            generated_at: Utc::now().to_rfc3339(),
            checks,
            providers,
            browser_profiles,
            ssh,
            tokens,
            degraded_services,
        };

        let mut attrs = BTreeMap::new();
        attrs.insert(
            "event.kind".to_string(),
            json!(if assembled.all_passed {
                "auth.preflight.completed"
            } else {
                "auth.preflight.failed"
            }),
        );
        attrs.insert(
            "outcome".to_string(),
            json!(report_status_str(&report.status)),
        );
        attrs.insert("check.count".to_string(), json!(report.checks.len() as u64));
        attrs.insert(
            "degraded.count".to_string(),
            json!(report.degraded_services.len() as u64),
        );
        attrs.extend(flatten_with_prefix("payload", &json!(report)));
        capture_structured_log(
            if assembled.all_passed {
                Level::Info
            } else {
                Level::Warning
            },
            "auth.preflight",
            attrs,
        );

        report
    }

    pub fn apply_credential(
        &self,
        agent_id: Option<AgentId>,
        service: &str,
        credential: &str,
    ) -> Result<String, String> {
        let service = service.trim();
        if service.is_empty() || credential.trim().is_empty() {
            return Err("service and credential are required".to_string());
        }
        let target_agent = agent_id.unwrap_or_else(global_agent_id);
        let key = format!("{AUTH_CREDENTIAL_PREFIX}{service}");
        let payload = serde_json::to_value(AuthStoredCredential {
            service: service.to_string(),
            credential: credential.to_string(),
            updated_at: Utc::now(),
        })
        .map_err(|e| e.to_string())?;
        self.memory
            .structured_set(target_agent, &key, payload)
            .map_err(|e| e.to_string())?;
        self.mark_recovered(agent_id, service)?;

        let mut attrs = BTreeMap::new();
        attrs.insert("event.kind".to_string(), json!("auth.escalation.resolved"));
        attrs.insert("auth.service".to_string(), json!(service));
        attrs.insert("auth.mode".to_string(), json!("credential"));
        attrs.insert(
            "agent.id".to_string(),
            json!(agent_id.map(|id| id.to_string()).unwrap_or_default()),
        );
        attrs.insert("outcome".to_string(), json!("resolved"));
        capture_structured_log(Level::Info, "auth credential updated", attrs);

        Ok(format!(
            "Stored credential for {service}{}.",
            agent_id
                .map(|id| format!(" on agent {}", id))
                .unwrap_or_default()
        ))
    }

    pub fn resume_service(
        &self,
        agent_id: Option<AgentId>,
        service: &str,
    ) -> Result<String, String> {
        self.mark_recovered(agent_id, service)?;

        let mut attrs = BTreeMap::new();
        attrs.insert("event.kind".to_string(), json!("auth.escalation.resolved"));
        attrs.insert("auth.service".to_string(), json!(service));
        attrs.insert("auth.mode".to_string(), json!("resume"));
        attrs.insert(
            "agent.id".to_string(),
            json!(agent_id.map(|id| id.to_string()).unwrap_or_default()),
        );
        attrs.insert("outcome".to_string(), json!("resumed"));
        capture_structured_log(Level::Info, "auth service resumed", attrs);

        Ok(format!(
            "Auth surface for {service} marked healthy{}.",
            agent_id
                .map(|id| format!(" on agent {}", id))
                .unwrap_or_default()
        ))
    }

    pub fn record_auth_failure(&self, failure: &AuthFailure) -> Result<Option<String>, String> {
        let mut states = self.read_degraded_state()?;
        let now = Utc::now();
        let key = degraded_key(&failure.agent_id, &failure.service);

        let mut should_alert = false;
        let mut retry_count = 1;
        if let Some(existing) = states.get_mut(&key) {
            existing.error = failure.error.clone();
            existing.retry_count = existing.retry_count.saturating_add(1);
            retry_count = existing.retry_count;
            let cooldown_elapsed = existing
                .last_alert_at
                .map(|last| (now - last).num_seconds() >= DEFAULT_ESCALATION_COOLDOWN_SECS)
                .unwrap_or(true);
            if cooldown_elapsed {
                existing.last_alert_at = Some(now);
                should_alert = true;
            }
        } else {
            states.insert(
                key,
                DegradedAuthState {
                    agent_id: failure.agent_id.clone(),
                    service: failure.service.clone(),
                    error: failure.error.clone(),
                    retry_count,
                    first_failure_at: now,
                    last_alert_at: Some(now),
                },
            );
            should_alert = true;
        }

        self.write_degraded_state(&states)?;
        if !should_alert {
            return Ok(None);
        }

        let alert = AuthEscalation::format_alert(failure);
        let mut attrs = BTreeMap::new();
        attrs.insert("event.kind".to_string(), json!("auth.escalation.sent"));
        attrs.insert("agent.id".to_string(), json!(failure.agent_id));
        attrs.insert("auth.service".to_string(), json!(failure.service));
        attrs.insert("auth.mode".to_string(), json!("manual_intervention"));
        attrs.insert("outcome".to_string(), json!("degraded"));
        attrs.insert("retry.count".to_string(), json!(retry_count));
        attrs.extend(flatten_with_prefix("payload", &json!(failure)));
        capture_structured_log(Level::Warning, "auth escalation sent", attrs);
        Ok(Some(alert))
    }

    pub fn list_degraded(&self) -> Result<Vec<DegradedAuthState>, String> {
        let mut values = self
            .read_degraded_state()?
            .into_values()
            .collect::<Vec<DegradedAuthState>>();
        values.sort_by(|a, b| a.agent_id.cmp(&b.agent_id).then(a.service.cmp(&b.service)));
        Ok(values)
    }

    fn list_oauth_tokens(&self) -> Vec<TokenStatus> {
        let Ok(entries) = self.memory.list_kv(global_agent_id()) else {
            return Vec::new();
        };
        entries
            .into_iter()
            .filter_map(|(key, value)| {
                if !key.starts_with(AUTH_OAUTH_PREFIX) {
                    return None;
                }
                let entry: OAuthEntry = serde_json::from_value(value).ok()?;
                Some(TokenStatus {
                    service: key.trim_start_matches(AUTH_OAUTH_PREFIX).to_string(),
                    expires_at: entry.expires_at.map(|dt| dt.to_rfc3339()),
                    refreshable: entry.refresh_token.is_some(),
                })
            })
            .collect()
    }

    fn mark_recovered(&self, agent_id: Option<AgentId>, service: &str) -> Result<(), String> {
        let mut states = self.read_degraded_state()?;
        let target_agent = agent_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| global_agent_id().to_string());
        states.remove(&degraded_key(&target_agent, service));
        self.write_degraded_state(&states)
    }

    fn read_degraded_state(&self) -> Result<BTreeMap<String, DegradedAuthState>, String> {
        match self
            .memory
            .structured_get(global_agent_id(), AUTH_DEGRADED_KEY)
            .map_err(|e| e.to_string())?
        {
            Some(value) => serde_json::from_value(value).map_err(|e| e.to_string()),
            None => Ok(BTreeMap::new()),
        }
    }

    fn write_degraded_state(
        &self,
        states: &BTreeMap<String, DegradedAuthState>,
    ) -> Result<(), String> {
        self.memory
            .structured_set(
                global_agent_id(),
                AUTH_DEGRADED_KEY,
                serde_json::to_value(states).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
    }

    pub fn store_oauth_entry(&self, service: &str, entry: &OAuthEntry) -> Result<(), String> {
        let ttl = entry
            .expires_at
            .and_then(|expires_at| (expires_at - Utc::now()).to_std().ok());
        let key = format!("{AUTH_OAUTH_PREFIX}{service}");
        let value = serde_json::to_value(entry).map_err(|e| e.to_string())?;
        match ttl {
            Some(ttl) if !ttl.is_zero() => self
                .memory
                .structured_set_with_ttl(global_agent_id(), &key, value, ttl)
                .map_err(|e| e.to_string()),
            _ => self
                .memory
                .structured_set(global_agent_id(), &key, value)
                .map_err(|e| e.to_string()),
        }
    }
}

fn global_agent_id() -> AgentId {
    AgentId(Uuid::nil())
}

fn degraded_key(agent_id: &str, service: &str) -> String {
    format!("{agent_id}:{service}")
}

fn remote_ssh_target() -> Option<(String, String, u16)> {
    let raw = std::env::var("OPENFANG_REMOTE_HOST")
        .ok()
        .or_else(|| std::env::var("REMOTE_HOST").ok())?;
    let port = std::env::var("OPENFANG_REMOTE_SSH_PORT")
        .ok()
        .or_else(|| std::env::var("REMOTE_SSH_PORT").ok())
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(22);
    let (username, host) = if let Some((user, host)) = raw.split_once('@') {
        (user.to_string(), host.to_string())
    } else {
        (
            std::env::var("OPENFANG_REMOTE_SSH_USER")
                .ok()
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "root".to_string()),
            raw,
        )
    };
    Some((username, host, port))
}

fn report_status_str(status: &AuthReadinessStatus) -> &'static str {
    match status {
        AuthReadinessStatus::Pass => "pass",
        AuthReadinessStatus::Warn => "warn",
        AuthReadinessStatus::Fail => "fail",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfang_types::config::KernelConfig;

    fn setup() -> AuthCoordinator {
        let memory = Arc::new(MemorySubstrate::open_in_memory(0.1).unwrap());
        AuthCoordinator::new(memory, &KernelConfig::default())
    }

    #[test]
    fn test_apply_credential_and_resume() {
        let coordinator = setup();
        let agent_id = AgentId::new();
        coordinator
            .apply_credential(Some(agent_id), "claude.ai", "token")
            .unwrap();
        let key = format!("{AUTH_CREDENTIAL_PREFIX}{}", "claude.ai");
        let stored = coordinator.memory.structured_get(agent_id, &key).unwrap();
        assert!(stored.is_some());
        coordinator
            .resume_service(Some(agent_id), "claude.ai")
            .unwrap();
    }

    #[test]
    fn test_record_auth_failure_dedupes_within_cooldown() {
        let coordinator = setup();
        let failure = AuthFailure {
            agent_id: "agent-1".to_string(),
            service: "claude.ai".to_string(),
            error: "session expired".to_string(),
            auto_refresh_attempted: false,
            auto_refresh_error: None,
        };
        assert!(coordinator.record_auth_failure(&failure).unwrap().is_some());
        assert!(coordinator.record_auth_failure(&failure).unwrap().is_none());
    }

    #[test]
    fn test_store_oauth_entry_uses_ttl() {
        let coordinator = setup();
        let entry = OAuthEntry {
            access_token: "token".to_string(),
            refresh_token: Some("refresh".to_string()),
            token_url: "https://example.com/token".to_string(),
            issued_at: Some(Utc::now()),
            expires_at: Some(Utc::now() + chrono::Duration::seconds(1)),
            client_id: None,
            client_secret: None,
        };
        coordinator.store_oauth_entry("svc", &entry).unwrap();
        let key = format!("{AUTH_OAUTH_PREFIX}svc");
        assert!(coordinator
            .memory
            .structured_get(global_agent_id(), &key)
            .unwrap()
            .is_some());
    }
}
