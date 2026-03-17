//! Typed error types for A2A (Agent-to-Agent) protocol operations.

use thiserror::Error;

/// Typed errors for A2A protocol operations, replacing bare `String` errors.
#[derive(Error, Debug)]
pub enum A2aError {
    #[error("A2A network error: {0}")]
    Network(String),

    #[error("A2A parse error: {0}")]
    Parse(String),

    #[error("A2A request timeout")]
    Timeout,

    #[error("A2A agent/task not found: {0}")]
    NotFound(String),

    #[error("A2A protocol error: {0}")]
    Protocol(String),

    #[error("A2A internal error: {0}")]
    Internal(String),
}
