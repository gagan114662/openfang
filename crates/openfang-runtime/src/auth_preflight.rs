//! Pre-flight auth checks for validating credentials before unattended operation.

use reqwest::Client;
use sentry::Level;
use serde::Serialize;
use std::time::Duration;
use tracing::{debug, warn};

/// Result of a single pre-flight check.
#[derive(Debug, Clone, Serialize)]
pub struct PreflightCheck {
    pub name: String,
    pub status: PreflightStatus,
    pub ttl: Option<String>,
    pub details: Option<String>,
    pub action_required: Option<String>,
}

/// Status of a pre-flight check.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreflightStatus {
    Pass,
    Warn,
    Fail,
}

/// Full pre-flight result.
#[derive(Debug, Clone, Serialize)]
pub struct PreflightResult {
    pub checks: Vec<PreflightCheck>,
    pub all_passed: bool,
}

/// Run pre-flight checks on configured LLM provider API keys.
pub async fn check_provider_keys(providers: &[(String, String)]) -> Vec<PreflightCheck> {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| {
            warn!(%error, "auth preflight HTTP client build failed; falling back to default client");
            crate::sentry_logs::capture_structured_log(
                Level::Warning,
                "auth.preflight.client_build_failed",
                std::collections::BTreeMap::from([
                    ("event.kind".to_string(), serde_json::json!("auth.preflight.failed")),
                    ("failure_reason".to_string(), serde_json::json!("client_build_failed")),
                    ("payload.error".to_string(), serde_json::json!(error.to_string())),
                ]),
            );
            Client::new()
        });

    let mut checks = Vec::new();

    for (name, api_key) in providers {
        let (url, header_name, header_value) = match name.as_str() {
            "anthropic" => (
                "https://api.anthropic.com/v1/models".to_string(),
                "x-api-key",
                api_key.to_string(),
            ),
            "openai" => (
                "https://api.openai.com/v1/models".to_string(),
                "Authorization",
                format!("Bearer {api_key}"),
            ),
            "groq" => (
                "https://api.groq.com/openai/v1/models".to_string(),
                "Authorization",
                format!("Bearer {api_key}"),
            ),
            _ => {
                checks.push(PreflightCheck {
                    name: name.clone(),
                    status: PreflightStatus::Warn,
                    ttl: Some("unknown".to_string()),
                    details: Some(
                        "No validation endpoint configured for this provider".to_string(),
                    ),
                    action_required: None,
                });
                continue;
            }
        };

        let result = client
            .get(&url)
            .header(header_name, &header_value)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                debug!(provider = %name, "API key validated successfully");
                checks.push(PreflightCheck {
                    name: name.clone(),
                    status: PreflightStatus::Pass,
                    ttl: Some("no_expiry".to_string()),
                    details: Some("API key is valid".to_string()),
                    action_required: None,
                });
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                warn!(provider = %name, status, "API key validation failed");
                checks.push(PreflightCheck {
                    name: name.clone(),
                    status: PreflightStatus::Fail,
                    ttl: None,
                    details: Some(format!("HTTP {status}: {body}")),
                    action_required: Some(format!("Update API key for {name}")),
                });
            }
            Err(e) => {
                warn!(provider = %name, error = %e, "API key validation request failed");
                checks.push(PreflightCheck {
                    name: name.clone(),
                    status: PreflightStatus::Fail,
                    ttl: None,
                    details: Some(format!("Request failed: {e}")),
                    action_required: Some(format!("Check network connectivity for {name}")),
                });
            }
        }
    }

    checks
}

/// Check if a browser profile directory exists and is accessible.
pub fn check_browser_profile(path: &str) -> PreflightCheck {
    let exists = std::path::Path::new(path).exists();
    if exists {
        PreflightCheck {
            name: "browser_profile".to_string(),
            status: PreflightStatus::Pass,
            ttl: Some("no_expiry".to_string()),
            details: Some(format!("Profile directory exists: {path}")),
            action_required: None,
        }
    } else {
        PreflightCheck {
            name: "browser_profile".to_string(),
            status: PreflightStatus::Warn,
            ttl: None,
            details: Some(format!("Profile directory does not exist: {path}")),
            action_required: Some(
                "Run browser once to create profile, or create directory manually".to_string(),
            ),
        }
    }
}

/// Check SSH connectivity to a remote host by running `echo ok`.
pub async fn check_ssh_connectivity(host: &str, port: u16, username: &str) -> PreflightCheck {
    let target = format!("{username}@{host}");
    let ssh_cmd = format!(
        "ssh -p {port} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=10 {} echo ok",
        target,
    );

    let result = tokio::process::Command::new("zsh")
        .arg("-lc")
        .arg(&ssh_cmd)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => PreflightCheck {
            name: "ssh_connectivity".to_string(),
            status: PreflightStatus::Pass,
            ttl: Some("no_expiry".to_string()),
            details: Some(format!("SSH to {target}:{port} succeeded")),
            action_required: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            PreflightCheck {
                name: "ssh_connectivity".to_string(),
                status: PreflightStatus::Fail,
                ttl: None,
                details: Some(format!("SSH failed: {stderr}")),
                action_required: Some(format!("Check SSH access to {target}:{port}")),
            }
        }
        Err(e) => PreflightCheck {
            name: "ssh_connectivity".to_string(),
            status: PreflightStatus::Fail,
            ttl: None,
            details: Some(format!("Failed to run SSH: {e}")),
            action_required: Some("Ensure SSH client is installed".to_string()),
        },
    }
}

/// Assemble all checks into a PreflightResult.
pub fn assemble_result(checks: Vec<PreflightCheck>) -> PreflightResult {
    let all_passed = checks.iter().all(|c| c.status != PreflightStatus::Fail);
    PreflightResult { checks, all_passed }
}
