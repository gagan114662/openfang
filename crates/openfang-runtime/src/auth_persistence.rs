//! Auth persistence: cookie backup, token refresh, and session health checks.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use reqwest::Client;
use sentry::Level;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// An OAuth token entry stored in the agent KV store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthEntry {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_url: String,
    pub issued_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

/// A service health-check definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealthCheck {
    pub name: String,
    pub check_url: String,
    #[serde(default = "default_expected_status")]
    pub expected_status: u16,
    pub auth_header: Option<String>,
}

fn default_expected_status() -> u16 {
    200
}

/// Events emitted by the auth persistence layer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum AuthEvent {
    TokenRefreshed {
        provider: String,
        new_expiry: DateTime<Utc>,
    },
    TokenRefreshFailed {
        provider: String,
        error: String,
    },
    HealthCheckFailed {
        service: String,
        status: u16,
        error: String,
    },
    CookiesBackedUp {
        domain: String,
        count: usize,
    },
}

/// Manages auth credential persistence and proactive refresh.
pub struct AuthPersistence {
    http_client: Client,
}

impl AuthPersistence {
    pub fn new() -> Self {
        Self {
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|error| {
                    warn!(%error, "auth persistence HTTP client build failed; falling back to default client");
                    crate::sentry_logs::capture_structured_log(
                        Level::Warning,
                        "auth.persistence.client_build_failed",
                        std::collections::BTreeMap::from([
                            ("event.kind".to_string(), serde_json::json!("auth.preflight.failed")),
                            ("failure_reason".to_string(), serde_json::json!("client_build_failed")),
                            ("payload.error".to_string(), serde_json::json!(error.to_string())),
                        ]),
                    );
                    Client::new()
                }),
        }
    }

    /// Attempt to refresh an OAuth token using its refresh_token grant.
    /// Returns the updated entry and an AuthEvent on success or failure.
    pub async fn refresh_oauth_token(
        &self,
        entry: &OAuthEntry,
    ) -> Result<(OAuthEntry, AuthEvent), AuthEvent> {
        let refresh_token =
            entry
                .refresh_token
                .as_deref()
                .ok_or_else(|| AuthEvent::TokenRefreshFailed {
                    provider: entry.token_url.clone(),
                    error: "No refresh token available".to_string(),
                })?;

        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ];
        let client_id_val;
        let client_secret_val;
        if let Some(ref cid) = entry.client_id {
            client_id_val = cid.clone();
            form.push(("client_id", &client_id_val));
        }
        if let Some(ref cs) = entry.client_secret {
            client_secret_val = cs.clone();
            form.push(("client_secret", &client_secret_val));
        }

        let resp = self
            .http_client
            .post(&entry.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| AuthEvent::TokenRefreshFailed {
                provider: entry.token_url.clone(),
                error: format!("HTTP request failed: {e}"),
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthEvent::TokenRefreshFailed {
                provider: entry.token_url.clone(),
                error: format!("Token endpoint returned {status}: {body}"),
            });
        }

        let body: serde_json::Value =
            resp.json()
                .await
                .map_err(|e| AuthEvent::TokenRefreshFailed {
                    provider: entry.token_url.clone(),
                    error: format!("Failed to parse token response: {e}"),
                })?;

        let new_access = body["access_token"]
            .as_str()
            .unwrap_or(&entry.access_token)
            .to_string();
        let new_refresh = body["refresh_token"]
            .as_str()
            .map(String::from)
            .or_else(|| entry.refresh_token.clone());
        let expires_in = body["expires_in"].as_i64().unwrap_or(3600);
        let new_expiry = Utc::now() + ChronoDuration::seconds(expires_in);

        let updated = OAuthEntry {
            access_token: new_access,
            refresh_token: new_refresh,
            token_url: entry.token_url.clone(),
            issued_at: Some(Utc::now()),
            expires_at: Some(new_expiry),
            client_id: entry.client_id.clone(),
            client_secret: entry.client_secret.clone(),
        };

        let event = AuthEvent::TokenRefreshed {
            provider: entry.token_url.clone(),
            new_expiry,
        };

        info!(provider = %entry.token_url, expires = %new_expiry, "OAuth token refreshed");
        Ok((updated, event))
    }

    /// Check if a token should be refreshed (past 80% of its TTL).
    pub fn should_refresh(entry: &OAuthEntry) -> bool {
        let (Some(issued_at), Some(expires_at)) = (entry.issued_at, entry.expires_at) else {
            return false;
        };
        let now = Utc::now();
        if expires_at <= now {
            return true;
        }
        if now <= issued_at {
            return false;
        }
        let total_ttl = (expires_at - issued_at).num_seconds().max(1);
        let elapsed = (now - issued_at).num_seconds().max(0);
        elapsed >= ((total_ttl as f64) * 0.8).ceil() as i64
    }

    /// Run a health check against a service endpoint.
    pub async fn check_service_health(&self, check: &ServiceHealthCheck) -> Result<(), AuthEvent> {
        let mut req = self.http_client.get(&check.check_url);
        if let Some(ref header) = check.auth_header {
            req = req.header("Authorization", header);
        }

        let resp = req.send().await.map_err(|e| AuthEvent::HealthCheckFailed {
            service: check.name.clone(),
            status: 0,
            error: format!("Request failed: {e}"),
        })?;

        let status = resp.status().as_u16();
        if status != check.expected_status {
            warn!(
                service = %check.name,
                expected = check.expected_status,
                actual = status,
                "Service health check failed"
            );
            return Err(AuthEvent::HealthCheckFailed {
                service: check.name.clone(),
                status,
                error: format!("Expected status {}, got {status}", check.expected_status),
            });
        }

        debug!(service = %check.name, "Service health check passed");
        Ok(())
    }
}
