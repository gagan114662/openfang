pub mod config;
pub mod mock;
pub mod mutation;
pub mod results;
pub mod runner;
pub mod scoring;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExperimentError {
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("driver error: {0}")]
    Driver(String),
    #[error("agent loop error: {0}")]
    AgentLoop(String),
    #[error("memory error: {0}")]
    Memory(String),
    #[error("scoring error: {0}")]
    Scoring(String),
    #[error("mutation error: {0}")]
    Mutation(String),
    #[error("mutation rejected: {0}")]
    MutationRejected(String),
}

pub type ExperimentResult<T> = std::result::Result<T, ExperimentError>;
