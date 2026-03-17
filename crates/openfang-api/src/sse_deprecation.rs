//! Helpers for deprecating SSE streaming endpoints in favor of `/ws`.

use axum::http::header::{HeaderName, HeaderValue};
use axum::response::Response;

/// Attach standard deprecation headers to SSE responses when WS transport is enabled.
///
/// This follows the WS transport rollout plan:
/// - `Deprecation: true`
/// - `Sunset: <HTTP-date>` (90 days from now)
/// - `Link: </ws>; rel="alternate"`
/// - `Warning: 299 openfang "SSE deprecated; use /ws"`
pub fn add_sse_deprecation_headers(enabled: bool, mut resp: Response) -> Response {
    if !enabled {
        return resp;
    }

    let headers = resp.headers_mut();
    headers.insert(
        HeaderName::from_static("deprecation"),
        HeaderValue::from_static("true"),
    );

    // HTTP-date (IMF-fixdate). Use UTC and the canonical "GMT" suffix.
    let sunset = (chrono::Utc::now() + chrono::Duration::days(90))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    if let Ok(v) = HeaderValue::from_str(&sunset) {
        headers.insert(HeaderName::from_static("sunset"), v);
    }

    headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_static("</ws>; rel=\"alternate\""),
    );
    headers.insert(
        HeaderName::from_static("warning"),
        HeaderValue::from_static("299 openfang \"SSE deprecated; use /ws\""),
    );

    resp
}

