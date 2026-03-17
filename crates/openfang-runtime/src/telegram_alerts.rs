use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tracing::warn;

const DEFAULT_BOT_TOKEN_ENV: &str = "TELEGRAM_BOT_TOKEN";

fn resolve_admin_chat_id(explicit_chat_id: Option<i64>) -> Option<i64> {
    explicit_chat_id.or_else(|| {
        std::env::var("OPENFANG_ADMIN_TELEGRAM_CHAT_ID")
            .ok()
            .or_else(|| std::env::var("TELEGRAM_ADMIN_CHAT_ID").ok())
            .and_then(|raw| raw.parse::<i64>().ok())
    })
}

pub async fn send_admin_alert(
    token_env: Option<&str>,
    explicit_chat_id: Option<i64>,
    message: &str,
) -> Result<bool, String> {
    let Some(chat_id) = resolve_admin_chat_id(explicit_chat_id) else {
        return Ok(false);
    };
    let token_env = token_env.unwrap_or(DEFAULT_BOT_TOKEN_ENV);
    let token = match std::env::var(token_env) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return Ok(false),
    };

    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let client = Client::new();
    let body = json!({
        "chat_id": chat_id,
        "text": message,
    });
    let mut backoff = Duration::from_secs(1);

    for attempt in 1..=5 {
        let resp = client.post(&url).json(&body).send().await;
        match resp {
            Ok(resp) if resp.status().is_success() => return Ok(true),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                warn!(chat_id, token_env, attempt, http_status = %status, body, "Telegram admin alert failed");
                if attempt == 5 || (!status.is_server_error() && status.as_u16() != 429) {
                    return Err(body);
                }
            }
            Err(err) => {
                warn!(chat_id, token_env, attempt, error = %err, "Telegram admin alert network failure");
                if attempt == 5 {
                    return Err(err.to_string());
                }
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(20));
    }
    Ok(false)
}
