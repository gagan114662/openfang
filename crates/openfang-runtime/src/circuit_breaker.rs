//! Per-agent circuit breaker to prevent cascading failures.

use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Failures exceeded threshold — requests are rejected.
    Open,
    /// Timeout elapsed — one probe request is allowed.
    HalfOpen,
}

/// Runtime statistics for a circuit breaker.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CircuitBreakerStats {
    /// Current state.
    pub state: CircuitState,
    /// Consecutive failure count.
    pub failure_count: u32,
    /// Total successful requests since last reset.
    pub success_count: u64,
    /// Total rejected requests (while Open).
    pub rejected_count: u64,
    /// Failure threshold before tripping.
    pub failure_threshold: u32,
    /// Recovery timeout in seconds.
    pub recovery_secs: u64,
}

/// Per-agent circuit breaker with state machine: Closed -> Open -> HalfOpen -> Closed.
pub struct CircuitBreaker {
    inner: RwLock<CircuitBreakerInner>,
    failure_threshold: u32,
    recovery_timeout: Duration,
}

struct CircuitBreakerInner {
    state: CircuitState,
    failure_count: u32,
    success_count: u64,
    rejected_count: u64,
    last_failure_time: Option<Instant>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given threshold and recovery timeout.
    pub fn new(failure_threshold: u32, recovery_secs: u64) -> Self {
        Self {
            inner: RwLock::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                failure_count: 0,
                success_count: 0,
                rejected_count: 0,
                last_failure_time: None,
            }),
            failure_threshold,
            recovery_timeout: Duration::from_secs(recovery_secs),
        }
    }

    /// Check if a request is allowed. Returns true if the request can proceed.
    pub fn allow_request(&self) -> bool {
        let mut inner = self.inner.write().unwrap();

        match inner.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last_failure) = inner.last_failure_time {
                    if last_failure.elapsed() >= self.recovery_timeout {
                        inner.state = CircuitState::HalfOpen;
                        true
                    } else {
                        inner.rejected_count += 1;
                        false
                    }
                } else {
                    inner.rejected_count += 1;
                    false
                }
            }
            CircuitState::HalfOpen => {
                inner.rejected_count += 1;
                false
            }
        }
    }

    /// Record a successful request.
    pub fn record_success(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.success_count += 1;

        match inner.state {
            CircuitState::HalfOpen => {
                inner.state = CircuitState::Closed;
                inner.failure_count = 0;
            }
            CircuitState::Closed => {
                inner.failure_count = 0;
            }
            CircuitState::Open => {}
        }
    }

    /// Record a failed request.
    pub fn record_failure(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.failure_count += 1;
        inner.last_failure_time = Some(Instant::now());

        match inner.state {
            CircuitState::Closed => {
                if inner.failure_count >= self.failure_threshold {
                    inner.state = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                inner.state = CircuitState::Open;
            }
            CircuitState::Open => {}
        }
    }

    /// Get current state.
    pub fn state(&self) -> CircuitState {
        let inner = self.inner.read().unwrap();
        inner.state
    }

    /// Get runtime statistics.
    pub fn stats(&self) -> CircuitBreakerStats {
        let inner = self.inner.read().unwrap();
        CircuitBreakerStats {
            state: inner.state,
            failure_count: inner.failure_count,
            success_count: inner.success_count,
            rejected_count: inner.rejected_count,
            failure_threshold: self.failure_threshold,
            recovery_secs: self.recovery_timeout.as_secs(),
        }
    }

    /// Reset the circuit breaker to Closed state.
    pub fn reset(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.state = CircuitState::Closed;
        inner.failure_count = 0;
        inner.last_failure_time = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starts_closed() {
        let cb = CircuitBreaker::new(5, 60);
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn test_stays_closed_under_threshold() {
        let cb = CircuitBreaker::new(5, 60);
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn test_opens_at_threshold() {
        let cb = CircuitBreaker::new(5, 60);
        for _ in 0..5 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_half_open_after_timeout() {
        let cb = CircuitBreaker::new(5, 0);
        for _ in 0..5 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_success_closes() {
        let cb = CircuitBreaker::new(5, 0);
        for _ in 0..5 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.allow_request());

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let cb = CircuitBreaker::new(5, 0);
        for _ in 0..5 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.allow_request()); // -> HalfOpen

        cb.record_failure(); // probe failed -> back to Open
        assert_eq!(cb.state(), CircuitState::Open);

        // Use a breaker with non-zero timeout to verify Open rejects
        let cb2 = CircuitBreaker::new(3, 600);
        for _ in 0..3 {
            cb2.record_failure();
        }
        assert_eq!(cb2.state(), CircuitState::Open);
        assert!(!cb2.allow_request()); // rejected — timeout not elapsed
    }

    #[test]
    fn test_success_resets_failure_count() {
        let cb = CircuitBreaker::new(5, 60);
        for _ in 0..4 {
            cb.record_failure();
        }
        cb.record_success();
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_stats() {
        let cb = CircuitBreaker::new(3, 30);
        cb.record_success();
        cb.record_failure();
        cb.record_failure();

        let stats = cb.stats();
        assert_eq!(stats.state, CircuitState::Closed);
        assert_eq!(stats.failure_count, 2);
        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.failure_threshold, 3);
        assert_eq!(stats.recovery_secs, 30);
    }

    #[test]
    fn test_reset() {
        let cb = CircuitBreaker::new(3, 60);
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }
}
