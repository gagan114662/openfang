//! Shared HTTP client singleton for connection pooling across utility functions.
//!
//! All utility code (TTS, image gen, media understanding, tool fetches) should
//! use `shared_http_client()` instead of creating per-call `reqwest::Client`s.
//! Per-request timeouts can still be set via `.timeout()` on the RequestBuilder.

use std::sync::OnceLock;
use std::time::Duration;

static SHARED_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub fn shared_http_client() -> &'static reqwest::Client {
    SHARED_HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(20)
            .build()
            .expect("Failed to build shared HTTP client")
    })
}
