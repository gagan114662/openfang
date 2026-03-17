//! Per-provider proactive rate limiting using token buckets.

use governor::{Quota, RateLimiter};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::warn;

/// Configuration for a single provider's rate limit.
#[derive(Debug, Clone)]
pub struct ThrottleConfig {
    /// Requests per minute.
    pub rpm: u32,
}

/// Per-provider throttle statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderThrottleStats {
    /// Provider name.
    pub provider: String,
    /// Configured requests per minute.
    pub rpm: u32,
    /// Total requests allowed through.
    pub allowed: u64,
    /// Total requests throttled (had to wait).
    pub throttled: u64,
    /// Number of consecutive HTTP 429 responses.
    pub consecutive_429s: u64,
    /// Whether exponential backoff is currently active.
    pub backoff_active: bool,
}

/// Registry of per-provider rate limiters.
pub struct ProviderThrottleRegistry {
    limiters: HashMap<String, ProviderThrottle>,
}

struct ProviderThrottle {
    limiter: Arc<RateLimiter<governor::state::NotKeyed, governor::state::InMemoryState, governor::clock::DefaultClock>>,
    rpm: u32,
    allowed: AtomicU64,
    throttled: AtomicU64,
    consecutive_429s: AtomicU64,
    backoff_until: std::sync::Mutex<Option<std::time::Instant>>,
}

impl ProviderThrottleRegistry {
    /// Create a new registry from provider limit configs.
    pub fn new(configs: &HashMap<String, openfang_types::config::ProviderLimitConfig>) -> Self {
        let mut limiters = HashMap::new();
        for (name, cfg) in configs {
            if cfg.rpm > 0 {
                if let Some(rpm) = NonZeroU32::new(cfg.rpm) {
                    let quota = Quota::per_minute(rpm);
                    let limiter = Arc::new(RateLimiter::direct(quota));
                    limiters.insert(
                        name.clone(),
                        ProviderThrottle {
                            limiter,
                            rpm: cfg.rpm,
                            allowed: AtomicU64::new(0),
                            throttled: AtomicU64::new(0),
                            consecutive_429s: AtomicU64::new(0),
                            backoff_until: std::sync::Mutex::new(None),
                        },
                    );
                }
            }
        }
        Self { limiters }
    }

    /// Acquire permission to make a request to the given provider.
    /// Blocks until a token is available. Returns immediately if no limit configured.
    /// Also waits for any active exponential backoff from HTTP 429 responses.
    pub async fn acquire(&self, provider: &str) {
        if let Some(throttle) = self.limiters.get(provider) {
            if let Ok(guard) = throttle.backoff_until.lock() {
                if let Some(until) = *guard {
                    let now = std::time::Instant::now();
                    if now < until {
                        let delay = until - now;
                        drop(guard);
                        tokio::time::sleep(delay).await;
                    }
                }
            }
            throttle.limiter.until_ready().await;
            throttle.allowed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record an HTTP 429 response from a provider, increasing exponential backoff.
    pub fn record_429(&self, provider: &str) {
        if let Some(throttle) = self.limiters.get(provider) {
            let count = throttle.consecutive_429s.fetch_add(1, Ordering::Relaxed) + 1;
            let backoff_secs = std::cmp::min(2u64.pow(count as u32), 120);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(backoff_secs);
            *throttle.backoff_until.lock().unwrap_or_else(|e| e.into_inner()) = Some(until);
            warn!(provider, consecutive_429s = count, backoff_secs, "Provider 429 backoff");
        }
    }

    /// Record a successful response from a provider, resetting backoff state.
    pub fn record_success(&self, provider: &str) {
        if let Some(throttle) = self.limiters.get(provider) {
            throttle.consecutive_429s.store(0, Ordering::Relaxed);
            *throttle.backoff_until.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    /// Try to acquire permission without blocking. Returns true if allowed.
    pub fn try_acquire(&self, provider: &str) -> bool {
        if let Some(throttle) = self.limiters.get(provider) {
            match throttle.limiter.check() {
                Ok(_) => {
                    throttle.allowed.fetch_add(1, Ordering::Relaxed);
                    true
                }
                Err(_) => {
                    throttle.throttled.fetch_add(1, Ordering::Relaxed);
                    warn!(provider, "Provider rate limit hit, request throttled");
                    false
                }
            }
        } else {
            true // No limit configured
        }
    }

    /// Get statistics for all configured providers.
    pub fn stats(&self) -> Vec<ProviderThrottleStats> {
        self.limiters
            .iter()
            .map(|(name, t)| {
                let backoff_active = t
                    .backoff_until
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .map(|until| std::time::Instant::now() < until)
                    .unwrap_or(false);
                ProviderThrottleStats {
                    provider: name.clone(),
                    rpm: t.rpm,
                    allowed: t.allowed.load(Ordering::Relaxed),
                    throttled: t.throttled.load(Ordering::Relaxed),
                    consecutive_429s: t.consecutive_429s.load(Ordering::Relaxed),
                    backoff_active,
                }
            })
            .collect()
    }

    /// Check if a provider has a configured rate limit.
    pub fn has_limit(&self, provider: &str) -> bool {
        self.limiters.contains_key(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_registry() {
        let registry = ProviderThrottleRegistry::new(&HashMap::new());
        assert!(registry.try_acquire("openai"));
        assert!(registry.stats().is_empty());
    }

    #[test]
    fn test_try_acquire_blocks_when_exhausted() {
        let mut configs = HashMap::new();
        configs.insert(
            "test_provider".to_string(),
            openfang_types::config::ProviderLimitConfig { rpm: 1, tpm: 0 },
        );
        let registry = ProviderThrottleRegistry::new(&configs);

        // First request should succeed
        assert!(registry.try_acquire("test_provider"));

        // Second request within same minute should fail
        assert!(!registry.try_acquire("test_provider"));

        let stats = registry.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].allowed, 1);
        assert_eq!(stats[0].throttled, 1);
    }

    #[test]
    fn test_unconfigured_provider_always_allowed() {
        let mut configs = HashMap::new();
        configs.insert(
            "groq".to_string(),
            openfang_types::config::ProviderLimitConfig { rpm: 10, tpm: 0 },
        );
        let registry = ProviderThrottleRegistry::new(&configs);

        // Unconfigured provider should always be allowed
        assert!(registry.try_acquire("openai"));
        assert!(!registry.has_limit("openai"));
        assert!(registry.has_limit("groq"));
    }

    #[test]
    fn test_zero_rpm_not_configured() {
        let mut configs = HashMap::new();
        configs.insert(
            "openai".to_string(),
            openfang_types::config::ProviderLimitConfig { rpm: 0, tpm: 0 },
        );
        let registry = ProviderThrottleRegistry::new(&configs);

        // rpm=0 means unlimited, so no limiter should be created
        assert!(!registry.has_limit("openai"));
        assert!(registry.try_acquire("openai"));
    }

    #[test]
    fn test_record_429_increments_backoff() {
        let mut configs = HashMap::new();
        configs.insert(
            "groq".to_string(),
            openfang_types::config::ProviderLimitConfig { rpm: 60, tpm: 0 },
        );
        let registry = ProviderThrottleRegistry::new(&configs);

        registry.record_429("groq");
        let stats = registry.stats();
        let groq = stats.iter().find(|s| s.provider == "groq").unwrap();
        assert_eq!(groq.consecutive_429s, 1);
        assert!(groq.backoff_active);

        registry.record_429("groq");
        let stats = registry.stats();
        let groq = stats.iter().find(|s| s.provider == "groq").unwrap();
        assert_eq!(groq.consecutive_429s, 2);
    }

    #[test]
    fn test_record_success_resets_backoff() {
        let mut configs = HashMap::new();
        configs.insert(
            "openai".to_string(),
            openfang_types::config::ProviderLimitConfig { rpm: 60, tpm: 0 },
        );
        let registry = ProviderThrottleRegistry::new(&configs);

        registry.record_429("openai");
        registry.record_429("openai");
        let stats = registry.stats();
        let oai = stats.iter().find(|s| s.provider == "openai").unwrap();
        assert_eq!(oai.consecutive_429s, 2);

        registry.record_success("openai");
        let stats = registry.stats();
        let oai = stats.iter().find(|s| s.provider == "openai").unwrap();
        assert_eq!(oai.consecutive_429s, 0);
        assert!(!oai.backoff_active);
    }

    #[test]
    fn test_backoff_exponential_cap() {
        let mut configs = HashMap::new();
        configs.insert(
            "test".to_string(),
            openfang_types::config::ProviderLimitConfig { rpm: 60, tpm: 0 },
        );
        let registry = ProviderThrottleRegistry::new(&configs);

        // Record many 429s — backoff should cap at 120 seconds
        for _ in 0..20 {
            registry.record_429("test");
        }

        let throttle = registry.limiters.get("test").unwrap();
        let until = throttle.backoff_until.lock().unwrap();
        let remaining = until
            .map(|u| u.duration_since(std::time::Instant::now()))
            .unwrap_or_default();
        assert!(remaining.as_secs() <= 120, "Backoff should cap at 120 seconds");
    }
}
