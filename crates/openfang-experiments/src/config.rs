use crate::ExperimentError;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct ExperimentConfig {
    pub name: String,
    pub base_prompt: String,
    pub test_message: String,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    pub scoring: ScoringConfig,
    pub mutation: MutationConfig,
    pub model: ModelSpec,
    #[serde(default)]
    pub sentry: Option<SentrySpec>,
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
}

fn default_max_iterations() -> usize {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelSpec {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

fn default_provider() -> String {
    "anthropic".to_string()
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_temperature() -> f32 {
    0.7
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "strategy")]
pub enum ScoringConfig {
    #[serde(rename = "regex_match")]
    RegexMatch {
        patterns: Vec<String>,
        #[serde(default = "default_expected_matches")]
        expected_matches: usize,
    },
    #[serde(rename = "llm_judge")]
    LlmJudge {
        criteria: String,
        #[serde(default)]
        judge_model: Option<ModelSpec>,
    },
}

fn default_expected_matches() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "strategy")]
pub enum MutationConfig {
    #[serde(rename = "llm_mutator")]
    LlmMutator {
        #[serde(default)]
        mutator_model: Option<ModelSpec>,
        #[serde(default = "default_max_prompt_growth_pct")]
        max_prompt_growth_pct: usize,
        #[serde(default = "default_max_prompt_length")]
        max_prompt_length: usize,
    },
    #[serde(rename = "template_mutator")]
    TemplateMutator {
        variables: std::collections::HashMap<String, Vec<String>>,
    },
}

fn default_max_prompt_growth_pct() -> usize {
    20
}

fn default_max_prompt_length() -> usize {
    4096
}

#[derive(Debug, Clone, Deserialize)]
pub struct SentrySpec {
    pub dsn: Option<String>,
    #[serde(default = "default_sentry_env")]
    pub environment: String,
    #[serde(default = "default_traces_sample_rate")]
    pub traces_sample_rate: f32,
}

fn default_sentry_env() -> String {
    "experiments".to_string()
}

fn default_traces_sample_rate() -> f32 {
    1.0
}

impl ExperimentConfig {
    pub fn load(path: &Path) -> Result<Self, ExperimentError> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ExperimentError> {
        if self.name.is_empty() {
            return Err(ExperimentError::Config("name must not be empty".into()));
        }
        if self.base_prompt.is_empty() {
            return Err(ExperimentError::Config(
                "base_prompt must not be empty".into(),
            ));
        }
        if self.test_message.is_empty() {
            return Err(ExperimentError::Config(
                "test_message must not be empty".into(),
            ));
        }
        if self.max_iterations == 0 {
            return Err(ExperimentError::Config("max_iterations must be > 0".into()));
        }
        if self.model.model.is_empty() {
            return Err(ExperimentError::Config(
                "model.model must not be empty".into(),
            ));
        }
        if let ScoringConfig::RegexMatch { patterns, .. } = &self.scoring {
            if patterns.is_empty() {
                return Err(ExperimentError::Config(
                    "regex_match scoring requires at least one pattern".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn scoring_strategy_name(&self) -> &'static str {
        match &self.scoring {
            ScoringConfig::RegexMatch { .. } => "regex_match",
            ScoringConfig::LlmJudge { .. } => "llm_judge",
        }
    }

    pub fn mutation_strategy_name(&self) -> &'static str {
        match &self.mutation {
            MutationConfig::LlmMutator { .. } => "llm_mutator",
            MutationConfig::TemplateMutator { .. } => "template_mutator",
        }
    }
}
