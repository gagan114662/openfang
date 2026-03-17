//! Re-export circuit breaker from openfang-runtime.
//!
//! The implementation lives in openfang-runtime so the agent loop can use it
//! directly without a circular dependency.

pub use openfang_runtime::circuit_breaker::*;
