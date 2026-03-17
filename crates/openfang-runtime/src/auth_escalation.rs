//! Auth escalation: Telegram alerts for auth failures and graceful degradation.

use dashmap::DashMap;
use serde::Serialize;
use std::time::{Duration, Instant};
use tracing::info;

/// Default cooldown between repeated Telegram alerts for the same service.
const DEFAULT_ESCALATION_COOLDOWN: Duration = Duration::from_secs(1800);
/// Default retry interval for degraded services.
const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(1800);

/// Tracks a service that is currently degraded.
#[derive(Debug, Clone)]
struct DegradedService {
    agent_id: String,
    service: String,
    error: String,
    _first_failure: Instant,
    last_alert: Option<Instant>,
    retry_count: u32,
}

/// Result of parsing an /auth command from Telegram.
#[derive(Debug, Clone)]
pub struct AuthCommand {
    pub agent_id: String,
    pub service: String,
    pub credential: String,
}

/// Auth failure info for formatting alerts.
#[derive(Debug, Clone, Serialize)]
pub struct AuthFailure {
    pub agent_id: String,
    pub service: String,
    pub error: String,
    pub auto_refresh_attempted: bool,
    pub auto_refresh_error: Option<String>,
}

/// Manages auth failure escalation and graceful degradation.
pub struct AuthEscalation {
    degraded: DashMap<String, DegradedService>,
    escalation_cooldown: Duration,
    retry_interval: Duration,
}

impl AuthEscalation {
    pub fn new() -> Self {
        Self {
            degraded: DashMap::new(),
            escalation_cooldown: DEFAULT_ESCALATION_COOLDOWN,
            retry_interval: DEFAULT_RETRY_INTERVAL,
        }
    }

    /// Build the map key from agent + service.
    fn key(agent_id: &str, service: &str) -> String {
        format!("{agent_id}:{service}")
    }

    /// Record an auth failure. Returns a formatted Telegram alert if one should be sent.
    pub fn record_failure(&self, failure: &AuthFailure) -> Option<String> {
        let k = Self::key(&failure.agent_id, &failure.service);
        let now = Instant::now();

        let should_alert = if let Some(mut entry) = self.degraded.get_mut(&k) {
            entry.retry_count += 1;
            entry.error = failure.error.clone();
            match entry.last_alert {
                Some(last) if now.duration_since(last) < self.escalation_cooldown => false,
                _ => {
                    entry.last_alert = Some(now);
                    true
                }
            }
        } else {
            self.degraded.insert(
                k,
                DegradedService {
                    agent_id: failure.agent_id.clone(),
                    service: failure.service.clone(),
                    error: failure.error.clone(),
                    _first_failure: now,
                    last_alert: Some(now),
                    retry_count: 1,
                },
            );
            true
        };

        if should_alert {
            Some(Self::format_alert(failure))
        } else {
            None
        }
    }

    /// Format a Telegram-ready auth failure alert.
    pub fn format_alert(failure: &AuthFailure) -> String {
        let mut msg = format!(
            "AUTH FAILURE\nAgent: {}\nService: {}\nError: {}",
            failure.agent_id, failure.service, failure.error,
        );
        if failure.auto_refresh_attempted {
            let refresh_status = match &failure.auto_refresh_error {
                Some(e) => format!("Failed ({e})"),
                None => "Succeeded".to_string(),
            };
            msg.push_str(&format!("\nAuto-refresh: {refresh_status}"));
        }
        msg.push_str(&format!(
            "\n\nReply with:\n/auth {} {} <new-token>",
            failure.agent_id, failure.service,
        ));
        msg
    }

    /// Parse a `/auth agent_id service credential` command string.
    pub fn parse_auth_command(text: &str) -> Option<AuthCommand> {
        let text = text.trim();
        if !text.starts_with("/auth ") {
            return None;
        }
        let parts: Vec<&str> = text.splitn(4, ' ').collect();
        if parts.len() < 4 {
            return None;
        }
        Some(AuthCommand {
            agent_id: parts[1].to_string(),
            service: parts[2].to_string(),
            credential: parts[3].to_string(),
        })
    }

    /// Mark a service as recovered (remove from degraded list).
    pub fn mark_recovered(&self, agent_id: &str, service: &str) {
        let k = Self::key(agent_id, service);
        if self.degraded.remove(&k).is_some() {
            info!(agent_id, service, "Service recovered from degraded state");
        }
    }

    /// Check if a service is currently degraded.
    pub fn is_degraded(&self, agent_id: &str, service: &str) -> bool {
        self.degraded.contains_key(&Self::key(agent_id, service))
    }

    /// Get list of all currently degraded services.
    pub fn list_degraded(&self) -> Vec<(String, String, String, u32)> {
        self.degraded
            .iter()
            .map(|entry| {
                let v = entry.value();
                (
                    v.agent_id.clone(),
                    v.service.clone(),
                    v.error.clone(),
                    v.retry_count,
                )
            })
            .collect()
    }

    /// Get services that are due for retry (past the retry interval since last alert).
    pub fn services_due_for_retry(&self) -> Vec<(String, String)> {
        let now = Instant::now();
        self.degraded
            .iter()
            .filter(|entry| {
                let v = entry.value();
                match v.last_alert {
                    Some(last) => now.duration_since(last) >= self.retry_interval,
                    None => true,
                }
            })
            .map(|entry| {
                let v = entry.value();
                (v.agent_id.clone(), v.service.clone())
            })
            .collect()
    }
}

impl Default for AuthEscalation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_auth_command_valid() {
        let cmd = AuthEscalation::parse_auth_command("/auth researcher-01 claude.ai sk-abc123");
        assert!(cmd.is_some());
        let cmd = cmd.unwrap();
        assert_eq!(cmd.agent_id, "researcher-01");
        assert_eq!(cmd.service, "claude.ai");
        assert_eq!(cmd.credential, "sk-abc123");
    }

    #[test]
    fn test_parse_auth_command_missing_parts() {
        assert!(AuthEscalation::parse_auth_command("/auth agent service").is_none());
        assert!(AuthEscalation::parse_auth_command("/auth agent").is_none());
        assert!(AuthEscalation::parse_auth_command("not a command").is_none());
    }

    #[test]
    fn test_record_failure_first_time_sends_alert() {
        let esc = AuthEscalation::new();
        let failure = AuthFailure {
            agent_id: "agent-1".to_string(),
            service: "test-svc".to_string(),
            error: "401 Unauthorized".to_string(),
            auto_refresh_attempted: false,
            auto_refresh_error: None,
        };
        let alert = esc.record_failure(&failure);
        assert!(alert.is_some());
        assert!(alert.unwrap().contains("AUTH FAILURE"));
    }

    #[test]
    fn test_record_failure_suppresses_duplicate_within_cooldown() {
        let esc = AuthEscalation {
            degraded: DashMap::new(),
            escalation_cooldown: Duration::from_secs(3600),
            retry_interval: DEFAULT_RETRY_INTERVAL,
        };
        let failure = AuthFailure {
            agent_id: "agent-1".to_string(),
            service: "test-svc".to_string(),
            error: "401".to_string(),
            auto_refresh_attempted: false,
            auto_refresh_error: None,
        };
        let first = esc.record_failure(&failure);
        assert!(first.is_some());
        let second = esc.record_failure(&failure);
        assert!(
            second.is_none(),
            "Second alert within cooldown should be suppressed"
        );
    }

    #[test]
    fn test_mark_recovered() {
        let esc = AuthEscalation::new();
        let failure = AuthFailure {
            agent_id: "agent-1".to_string(),
            service: "svc".to_string(),
            error: "err".to_string(),
            auto_refresh_attempted: false,
            auto_refresh_error: None,
        };
        esc.record_failure(&failure);
        assert!(esc.is_degraded("agent-1", "svc"));
        esc.mark_recovered("agent-1", "svc");
        assert!(!esc.is_degraded("agent-1", "svc"));
    }

    #[test]
    fn test_format_alert_with_refresh() {
        let failure = AuthFailure {
            agent_id: "researcher".to_string(),
            service: "claude.ai".to_string(),
            error: "Session expired".to_string(),
            auto_refresh_attempted: true,
            auto_refresh_error: Some("Invalid refresh token".to_string()),
        };
        let alert = AuthEscalation::format_alert(&failure);
        assert!(alert.contains("Auto-refresh: Failed"));
        assert!(alert.contains("/auth researcher claude.ai"));
    }
}
