//! Production middleware for the OpenFang API server.
//!
//! Provides:
//! - Request ID generation and propagation
//! - Per-endpoint structured request logging
//! - In-memory rate limiting (per IP)

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;
use openfang_runtime::sentry_logs::{capture_structured_log, scope_event_context, EventContext};
use sentry::Level;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Instant;
use tracing::info;

/// Request ID header name (standard).
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Middleware: inject a unique request ID and log the request/response.
pub async fn request_logging(request: Request<Body>, next: Next) -> Response<Body> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let method = request.method().clone();
    let uri = request.uri().path().to_string();
    let remote_addr = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.to_string());
    let start = Instant::now();

    let mut response = scope_event_context(
        EventContext {
            trace_id: Some(request_id.clone()),
            request_id: Some(request_id.clone()),
            run_id: Some(request_id.clone()),
            session_id: None,
            agent_id: None,
            agent_name: None,
            channel_kind: Some("http".to_string()),
            channel_user_id: None,
        },
        next.run(request),
    )
    .await;

    let elapsed = start.elapsed();
    let status = response.status().as_u16();

    info!(
        request_id = %request_id,
        method = %method,
        path = %uri,
        status = status,
        latency_ms = elapsed.as_millis() as u64,
        "API request"
    );

    let mut attrs = BTreeMap::new();
    attrs.insert("event.kind".to_string(), json!("api.request"));
    attrs.insert(
        "event.id".to_string(),
        json!(uuid::Uuid::new_v4().to_string()),
    );
    attrs.insert(
        "occurred_at".to_string(),
        json!(chrono::Utc::now().to_rfc3339()),
    );
    attrs.insert("request.id".to_string(), json!(request_id));
    attrs.insert("run.id".to_string(), json!(request_id));
    attrs.insert("trace.id".to_string(), json!(request_id));
    attrs.insert("channel.kind".to_string(), json!("http"));
    attrs.insert("http.method".to_string(), json!(method.to_string()));
    attrs.insert("http.path".to_string(), json!(uri));
    attrs.insert("http.status_code".to_string(), json!(status));
    attrs.insert(
        "outcome".to_string(),
        json!(if status < 400 { "success" } else { "error" }),
    );
    attrs.insert("duration_ms".to_string(), json!(elapsed.as_millis() as u64));
    if let Some(remote_addr) = remote_addr {
        attrs.insert("client.address".to_string(), json!(remote_addr));
    }
    capture_structured_log(Level::Info, "api.request", attrs);

    // Inject the request ID into the response
    if let Ok(header_val) = request_id.parse() {
        response.headers_mut().insert(REQUEST_ID_HEADER, header_val);
    }

    response
}

/// Bearer token authentication middleware.
///
/// When `api_key` is non-empty, all requests must include
/// `Authorization: Bearer <api_key>`. If the key is empty, auth is bypassed.
pub async fn auth(
    axum::extract::State(api_key): axum::extract::State<String>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // If no API key configured, restrict to loopback addresses only.
    if api_key.is_empty() {
        let is_loopback = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0.ip().is_loopback())
            .unwrap_or(false);

        if !is_loopback {
            tracing::warn!(
                "Rejected non-localhost request: no API key configured. \
                 Set api_key in config.toml for remote access."
            );
            return Response::builder()
                .status(StatusCode::FORBIDDEN)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "error": "No API key configured. Remote access denied. Configure api_key in ~/.openfang/config.toml"
                    })
                    .to_string(),
                ))
                .unwrap_or_default();
        }
        return next.run(request).await;
    }

    // Public endpoints that don't require auth (dashboard needs these)
    let path = request.uri().path();
    let is_loopback = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().is_loopback())
        .unwrap_or(false);
    if is_loopback
        && (path == "/api/telemetry/structured"
            || path == "/api/ops/guard/report"
            || path == "/ops/guard/report")
    {
        return next.run(request).await;
    }
    if path == "/"
        || path == "/api/health"
        || path == "/api/health/detail"
        || path == "/api/status"
        || path == "/api/version"
        || path == "/api/agents"
        || path == "/api/profiles"
        || path == "/api/config"
        || path.starts_with("/api/uploads/")
    {
        return next.run(request).await;
    }

    // Check Authorization: Bearer <token> header
    let bearer_token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    // SECURITY: Use constant-time comparison to prevent timing attacks.
    let header_auth = bearer_token.map(|token| {
        use subtle::ConstantTimeEq;
        if token.len() != api_key.len() {
            return false;
        }
        token.as_bytes().ct_eq(api_key.as_bytes()).into()
    });

    // Also check ?token= query parameter (for EventSource/SSE clients that
    // cannot set custom headers, same approach as WebSocket auth).
    let query_token = request
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token=")));

    // SECURITY: Use constant-time comparison to prevent timing attacks.
    let query_auth = query_token.map(|token| {
        use subtle::ConstantTimeEq;
        if token.len() != api_key.len() {
            return false;
        }
        token.as_bytes().ct_eq(api_key.as_bytes()).into()
    });

    // Accept if either auth method matches
    if header_auth == Some(true) || query_auth == Some(true) {
        return next.run(request).await;
    }

    // Determine error message: was a credential provided but wrong, or missing entirely?
    let credential_provided = header_auth.is_some() || query_auth.is_some();
    let error_msg = if credential_provided {
        "Invalid API key"
    } else {
        "Missing Authorization: Bearer <api_key> header"
    };

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("www-authenticate", "Bearer")
        .body(Body::from(
            serde_json::json!({"error": error_msg}).to_string(),
        ))
        .unwrap_or_default()
}

/// Security headers middleware — applied to ALL API responses.
pub async fn security_headers(request: Request<Body>, next: Next) -> Response<Body> {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("x-xss-protection", "1; mode=block".parse().unwrap());
    // All JS/CSS is bundled inline — only external resource is Google Fonts.
    headers.insert(
        "content-security-policy",
        "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fonts.gstatic.com; img-src 'self' data: blob:; connect-src 'self' ws://localhost:* ws://127.0.0.1:* wss://localhost:* wss://127.0.0.1:*; font-src 'self' https://fonts.gstatic.com; media-src 'self' blob:; frame-src 'self' blob:; object-src 'none'; base-uri 'self'; form-action 'self'"
            .parse()
            .unwrap(),
    );
    headers.insert(
        "referrer-policy",
        "strict-origin-when-cross-origin".parse().unwrap(),
    );
    headers.insert(
        "cache-control",
        "no-store, no-cache, must-revalidate".parse().unwrap(),
    );
    response
}

/// Middleware: create a Sentry transaction for every HTTP request.
///
/// This gives full visibility in Sentry's performance dashboard: every API
/// call appears as a transaction with method, path, status, and latency.
pub async fn sentry_transaction(request: Request<Body>, next: Next) -> Response<Body> {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let tx_name = format!("{method} {path}");

    let tx_ctx = sentry::TransactionContext::new(&tx_name, "http.server");
    let transaction = sentry::start_transaction(tx_ctx);
    sentry::configure_scope(|scope: &mut sentry::Scope| {
        scope.set_span(Some(transaction.clone().into()));
    });

    let response = next.run(request).await;

    let status_code = response.status().as_u16();
    transaction.set_data(
        "http.status_code",
        sentry::protocol::Value::from(status_code),
    );
    transaction.set_status(match status_code {
        200..=299 => sentry::protocol::SpanStatus::Ok,
        400 => sentry::protocol::SpanStatus::InvalidArgument,
        401 => sentry::protocol::SpanStatus::Unauthenticated,
        403 => sentry::protocol::SpanStatus::PermissionDenied,
        404 => sentry::protocol::SpanStatus::NotFound,
        429 => sentry::protocol::SpanStatus::ResourceExhausted,
        500..=599 => sentry::protocol::SpanStatus::InternalError,
        _ => sentry::protocol::SpanStatus::UnknownError,
    });
    transaction.finish();
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_header_constant() {
        assert_eq!(REQUEST_ID_HEADER, "x-request-id");
    }
}
