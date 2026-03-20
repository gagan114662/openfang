use crate::config::{ExperimentConfig, ModelSpec};
use crate::mutation::{create_mutator, Mutator};
use crate::results::{compute_prompt_hash, save_best_prompt, IterationResult, ResultsLogger};
use crate::scoring::{create_scorer, Scorer};
use crate::ExperimentError;
use chrono::Utc;
use openfang_memory::session::Session;
use openfang_memory::MemorySubstrate;
use openfang_runtime::agent_loop::run_agent_loop;
use openfang_runtime::drivers::create_driver;
use openfang_runtime::llm_driver::{DriverConfig, LlmDriver};
use openfang_types::agent::{AgentId, AgentManifest, ModelConfig, SessionId};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Debug, Serialize)]
pub struct ExperimentSummary {
    pub total_iterations: usize,
    pub best_score: f64,
    pub best_iteration: usize,
    pub best_prompt_hash: String,
    pub best_prompt_path: Option<PathBuf>,
    pub results_path: PathBuf,
    pub total_tokens_input: u64,
    pub total_tokens_output: u64,
    pub total_cost_usd: f64,
}

pub struct ExperimentRunner {
    config: ExperimentConfig,
    scorer: Box<dyn Scorer>,
    mutator: Box<dyn Mutator>,
    driver: Arc<dyn LlmDriver>,
    logger: ResultsLogger,
    output_dir: PathBuf,
}

impl ExperimentRunner {
    pub fn new(config: ExperimentConfig) -> Result<Self, ExperimentError> {
        let output_dir = config
            .output_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("experiment_results"));
        let logger = ResultsLogger::new(&output_dir, &config.name)?;

        let driver_config = model_spec_to_driver_config(&config.model)?;
        let driver =
            create_driver(&driver_config).map_err(|e| ExperimentError::Driver(format!("{e}")))?;

        let scorer = create_scorer(
            &config.scoring,
            Some(Arc::clone(&driver)),
            &config.model.model,
        )?;
        let mutator = create_mutator(
            &config.mutation,
            Some(Arc::clone(&driver)),
            &config.model.model,
        )?;

        Ok(Self {
            config,
            scorer,
            mutator,
            driver,
            logger,
            output_dir,
        })
    }

    pub fn new_with_deps(
        config: ExperimentConfig,
        driver: Arc<dyn LlmDriver>,
        scorer: Box<dyn Scorer>,
        mutator: Box<dyn Mutator>,
    ) -> Result<Self, ExperimentError> {
        let output_dir = config
            .output_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("experiment_results"));
        let logger = ResultsLogger::new(&output_dir, &config.name)?;
        Ok(Self {
            config,
            scorer,
            mutator,
            driver,
            logger,
            output_dir,
        })
    }

    pub async fn run(&self) -> Result<ExperimentSummary, ExperimentError> {
        let mut current_prompt = self.config.base_prompt.clone();
        let mut best_score: f64 = 0.0;
        let mut best_prompt = current_prompt.clone();
        let mut best_iteration: usize = 0;
        let mut best_prompt_hash = compute_prompt_hash(&current_prompt);
        let mut best_prompt_path: Option<PathBuf> = None;
        let mut parent_hash = compute_prompt_hash(&current_prompt);
        let mut total_tokens_input: u64 = 0;
        let mut total_tokens_output: u64 = 0;
        let mut total_cost_usd: f64 = 0.0;

        info!(
            experiment = %self.config.name,
            max_iterations = self.config.max_iterations,
            scoring = self.config.scoring_strategy_name(),
            mutation = self.config.mutation_strategy_name(),
            "starting experiment"
        );

        for iteration in 0..self.config.max_iterations {
            let prompt_hash = compute_prompt_hash(&current_prompt);

            sentry::add_breadcrumb(sentry::Breadcrumb {
                category: Some("experiment.iteration".into()),
                message: Some(format!("iteration {iteration}, prompt_hash={prompt_hash}")),
                level: sentry::Level::Info,
                ..Default::default()
            });

            info!(iteration, prompt_hash = %prompt_hash, "running iteration");

            let manifest = build_experiment_manifest(&self.config, &current_prompt);
            let memory = MemorySubstrate::open_in_memory(0.01)
                .map_err(|e| ExperimentError::Memory(format!("{e}")))?;
            let agent_id = AgentId::new();
            let session_id = SessionId::new();
            let mut session = Session {
                id: session_id,
                agent_id,
                messages: vec![],
                context_window_tokens: 0,
                label: Some(format!("experiment-{}-iter-{iteration}", self.config.name)),
            };

            let loop_result = run_agent_loop(
                &manifest,
                &self.config.test_message,
                &mut session,
                &memory,
                Arc::clone(&self.driver),
                &[],  // no tools
                None, // kernel
                None, // skill_registry
                None, // mcp_connections
                None, // web_ctx
                None, // browser_ctx
                None, // embedding_driver
                None, // workspace_root
                None, // on_phase
                None, // media_engine
                None, // tts_engine
                None, // docker_config
                None, // hooks
                None, // context_window_tokens
                None, // process_manager
            )
            .await;

            let (response_text, tokens_in, tokens_out, cost) = match loop_result {
                Ok(result) => {
                    let ti = result.total_usage.input_tokens;
                    let to = result.total_usage.output_tokens;
                    total_tokens_input += ti;
                    total_tokens_output += to;
                    let c = result.cost_usd.unwrap_or(0.0);
                    total_cost_usd += c;
                    (result.response, ti, to, c)
                }
                Err(e) => {
                    warn!(iteration, error = %e, "agent loop failed");
                    let ir = IterationResult {
                        iteration,
                        prompt_hash: prompt_hash.clone(),
                        parent_prompt_hash: parent_hash.clone(),
                        score: 0.0,
                        score_reasoning: String::new(),
                        response_preview: String::new(),
                        tokens_input: 0,
                        tokens_output: 0,
                        cost_usd: None,
                        improved: false,
                        failure_type: Some("agent_error".into()),
                        mutation_strategy: if iteration == 0 {
                            "baseline".into()
                        } else {
                            self.config.mutation_strategy_name().into()
                        },
                        scoring_strategy: self.config.scoring_strategy_name().into(),
                        mutation_diff_size: None,
                        timestamp: Utc::now().to_rfc3339(),
                        prompt_length: current_prompt.len(),
                    };
                    self.logger.log(&ir)?;
                    current_prompt = best_prompt.clone();
                    continue;
                }
            };

            let response_preview: String = response_text.chars().take(200).collect();

            let score_result = match self.scorer.score(&current_prompt, &response_text).await {
                Ok(sr) => sr,
                Err(e) => {
                    warn!(iteration, error = %e, "scoring failed");
                    let ir = IterationResult {
                        iteration,
                        prompt_hash: prompt_hash.clone(),
                        parent_prompt_hash: parent_hash.clone(),
                        score: 0.0,
                        score_reasoning: format!("scoring error: {e}"),
                        response_preview: response_preview.clone(),
                        tokens_input: tokens_in,
                        tokens_output: tokens_out,
                        cost_usd: Some(cost),
                        improved: false,
                        failure_type: Some("scoring_error".into()),
                        mutation_strategy: if iteration == 0 {
                            "baseline".into()
                        } else {
                            self.config.mutation_strategy_name().into()
                        },
                        scoring_strategy: self.config.scoring_strategy_name().into(),
                        mutation_diff_size: None,
                        timestamp: Utc::now().to_rfc3339(),
                        prompt_length: current_prompt.len(),
                    };
                    self.logger.log(&ir)?;
                    current_prompt = best_prompt.clone();
                    continue;
                }
            };

            let improved = score_result.score > best_score;

            if improved {
                best_score = score_result.score;
                best_prompt = current_prompt.clone();
                best_iteration = iteration;
                best_prompt_hash = prompt_hash.clone();
                best_prompt_path = Some(save_best_prompt(
                    &self.output_dir,
                    &self.config.name,
                    &current_prompt,
                    best_score,
                    iteration,
                    &prompt_hash,
                )?);
                info!(iteration, score = score_result.score, "new best score");
            } else {
                current_prompt = best_prompt.clone();
            }

            let mutation_diff_size = if iteration == 0 {
                None
            } else {
                Some(current_prompt.len() as i64 - best_prompt.len() as i64)
            };

            let ir = IterationResult {
                iteration,
                prompt_hash: prompt_hash.clone(),
                parent_prompt_hash: parent_hash.clone(),
                score: score_result.score,
                score_reasoning: score_result.reasoning,
                response_preview,
                tokens_input: tokens_in,
                tokens_output: tokens_out,
                cost_usd: Some(cost),
                improved,
                failure_type: None,
                mutation_strategy: if iteration == 0 {
                    "baseline".into()
                } else {
                    self.config.mutation_strategy_name().into()
                },
                scoring_strategy: self.config.scoring_strategy_name().into(),
                mutation_diff_size,
                timestamp: Utc::now().to_rfc3339(),
                prompt_length: current_prompt.len(),
            };
            self.logger.log(&ir)?;

            if iteration + 1 < self.config.max_iterations {
                match self
                    .mutator
                    .mutate(
                        &current_prompt,
                        &response_text,
                        score_result.score,
                        iteration,
                    )
                    .await
                {
                    Ok(mr) => {
                        if mr.rejected {
                            warn!(iteration, reason = %mr.rationale, "mutation rejected");
                            let reject_ir = IterationResult {
                                iteration,
                                prompt_hash: compute_prompt_hash(&current_prompt),
                                parent_prompt_hash: parent_hash.clone(),
                                score: score_result.score,
                                score_reasoning: mr.rationale,
                                response_preview: String::new(),
                                tokens_input: 0,
                                tokens_output: 0,
                                cost_usd: None,
                                improved: false,
                                failure_type: Some("mutation_rejected".into()),
                                mutation_strategy: self.config.mutation_strategy_name().into(),
                                scoring_strategy: self.config.scoring_strategy_name().into(),
                                mutation_diff_size: None,
                                timestamp: Utc::now().to_rfc3339(),
                                prompt_length: current_prompt.len(),
                            };
                            self.logger.log(&reject_ir)?;
                        } else {
                            parent_hash = compute_prompt_hash(&current_prompt);
                            current_prompt = mr.prompt;
                        }
                    }
                    Err(e) => {
                        error!(iteration, error = %e, "mutation failed");
                        let err_ir = IterationResult {
                            iteration,
                            prompt_hash: compute_prompt_hash(&current_prompt),
                            parent_prompt_hash: parent_hash.clone(),
                            score: 0.0,
                            score_reasoning: format!("mutation error: {e}"),
                            response_preview: String::new(),
                            tokens_input: 0,
                            tokens_output: 0,
                            cost_usd: None,
                            improved: false,
                            failure_type: Some("mutation_error".into()),
                            mutation_strategy: self.config.mutation_strategy_name().into(),
                            scoring_strategy: self.config.scoring_strategy_name().into(),
                            mutation_diff_size: None,
                            timestamp: Utc::now().to_rfc3339(),
                            prompt_length: current_prompt.len(),
                        };
                        self.logger.log(&err_ir)?;
                    }
                }
            }
        }

        info!(
            experiment = %self.config.name,
            best_score,
            best_iteration,
            best_prompt_hash = %best_prompt_hash,
            "experiment complete"
        );

        Ok(ExperimentSummary {
            total_iterations: self.config.max_iterations,
            best_score,
            best_iteration,
            best_prompt_hash,
            best_prompt_path,
            results_path: self.logger.path().to_path_buf(),
            total_tokens_input,
            total_tokens_output,
            total_cost_usd,
        })
    }
}

fn build_experiment_manifest(config: &ExperimentConfig, prompt: &str) -> AgentManifest {
    AgentManifest {
        name: format!("experiment-{}", config.name),
        model: ModelConfig {
            provider: config.model.provider.clone(),
            model: config.model.model.clone(),
            max_tokens: config.model.max_tokens,
            temperature: config.model.temperature,
            system_prompt: prompt.to_string(),
            api_key_env: config.model.api_key_env.clone(),
            base_url: config.model.base_url.clone(),
        },
        ..AgentManifest::default()
    }
}

fn model_spec_to_driver_config(spec: &ModelSpec) -> Result<DriverConfig, ExperimentError> {
    let api_key = if let Some(env_var) = &spec.api_key_env {
        std::env::var(env_var).ok()
    } else {
        match spec.provider.as_str() {
            "anthropic" => std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
                .ok()
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok()),
            "groq" => std::env::var("GROQ_API_KEY").ok(),
            "openai" => std::env::var("OPENAI_API_KEY").ok(),
            "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
            _ => None,
        }
    };
    Ok(DriverConfig {
        provider: spec.provider.clone(),
        api_key,
        base_url: spec.base_url.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::mock::*;

    fn test_config() -> ExperimentConfig {
        ExperimentConfig {
            name: "test-experiment".into(),
            base_prompt: "You are a helpful assistant.".into(),
            test_message: "Hello".into(),
            max_iterations: 3,
            scoring: ScoringConfig::RegexMatch {
                patterns: vec!["hello".into(), "help".into()],
                expected_matches: 2,
            },
            mutation: MutationConfig::LlmMutator {
                mutator_model: None,
                max_prompt_growth_pct: 20,
                max_prompt_length: 4096,
            },
            model: ModelSpec {
                provider: "mock".into(),
                model: "mock-model".into(),
                api_key_env: None,
                base_url: None,
                max_tokens: 100,
                temperature: 0.0,
            },
            sentry: None,
            output_dir: None,
        }
    }

    #[tokio::test]
    async fn test_experiment_loop_with_mocks() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config();
        config.output_dir = Some(dir.path().to_path_buf());

        let driver = Arc::new(MockDriver::new(vec![
            "Hello! I'm here to help you with anything.".into(),
            "Hi there, how can I help you today?".into(),
            "Hello! Let me help you out.".into(),
        ]));
        let scorer = Box::new(MockScorer::new(vec![50.0, 75.0, 60.0]));
        let mutator = Box::new(MockMutator::new(vec![
            "You are a very helpful assistant. Be friendly.".into(),
            "You are a helpful, empathetic assistant.".into(),
        ]));

        let runner = ExperimentRunner::new_with_deps(config, driver, scorer, mutator).unwrap();
        let summary = runner.run().await.unwrap();

        assert_eq!(summary.total_iterations, 3);
        assert_eq!(summary.best_score, 75.0);
        assert_eq!(summary.best_iteration, 1);
        assert!(summary.results_path.exists());
        assert!(summary.best_prompt_path.is_some());

        let content = std::fs::read_to_string(&summary.results_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert!(lines.len() >= 3);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["iteration"], 0);
        assert_eq!(first["mutation_strategy"], "baseline");
        assert!(first["parent_prompt_hash"].is_string());
        assert!(first["failure_type"].is_null());
    }

    #[tokio::test]
    async fn test_experiment_loop_keeps_best() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config();
        config.max_iterations = 4;
        config.output_dir = Some(dir.path().to_path_buf());

        let driver = Arc::new(MockDriver::new(vec!["response".into()]));
        let scorer = Box::new(MockScorer::new(vec![80.0, 60.0, 70.0, 90.0]));
        let mutator = Box::new(MockMutator::identity());

        let runner = ExperimentRunner::new_with_deps(config, driver, scorer, mutator).unwrap();
        let summary = runner.run().await.unwrap();

        assert_eq!(summary.best_score, 90.0);
        assert_eq!(summary.best_iteration, 3);
    }
}
