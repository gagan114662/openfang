use crate::config::MutationConfig;
use crate::results::compute_prompt_hash;
use crate::ExperimentError;
use async_trait::async_trait;
use openfang_runtime::llm_driver::{CompletionRequest, LlmDriver};
use std::sync::Arc;
use tracing::info;

pub struct MutationResult {
    pub prompt: String,
    pub rationale: String,
    pub rejected: bool,
}

#[async_trait]
pub trait Mutator: Send + Sync {
    async fn mutate(
        &self,
        current_prompt: &str,
        last_response: &str,
        last_score: f64,
        iteration: usize,
    ) -> Result<MutationResult, ExperimentError>;
}

pub struct LlmMutator {
    driver: Arc<dyn LlmDriver>,
    model: String,
    max_prompt_growth_pct: usize,
    max_prompt_length: usize,
}

impl LlmMutator {
    pub fn new(
        driver: Arc<dyn LlmDriver>,
        model: String,
        max_prompt_growth_pct: usize,
        max_prompt_length: usize,
    ) -> Self {
        Self {
            driver,
            model,
            max_prompt_growth_pct,
            max_prompt_length,
        }
    }
}

#[async_trait]
impl Mutator for LlmMutator {
    async fn mutate(
        &self,
        current_prompt: &str,
        last_response: &str,
        last_score: f64,
        iteration: usize,
    ) -> Result<MutationResult, ExperimentError> {
        let parent_hash = compute_prompt_hash(current_prompt);
        let mutation_prompt = format!(
            "You are a prompt engineering expert. Your task is to improve the following system prompt.\n\n\
             Current prompt (iteration {iteration}, score {last_score:.1}/100):\n\
             ---\n{current_prompt}\n---\n\n\
             The agent's last response with this prompt:\n\
             ---\n{last_response}\n---\n\n\
             Rules:\n\
             - Make small, targeted edits to improve the prompt. Do NOT rewrite from scratch.\n\
             - Focus on clarity, specificity, and structure.\n\
             - Keep changes minimal — tweak wording, add constraints, or restructure slightly.\n\n\
             Respond with ONLY the improved prompt text. No explanations, no markdown fences."
        );
        let messages = vec![openfang_types::message::Message {
            role: openfang_types::message::Role::User,
            content: openfang_types::message::MessageContent::Text(mutation_prompt),
        }];
        let request = CompletionRequest {
            model: self.model.clone(),
            messages,
            tools: vec![],
            max_tokens: (self.max_prompt_length as u32).max(1024),
            temperature: 0.8,
            system: None,
            thinking: None,
            sentry_parent_span: None,
        };
        let resp = self
            .driver
            .complete(request)
            .await
            .map_err(|e| ExperimentError::Mutation(format!("mutator LLM error: {e}")))?;
        let mutated = resp.text().trim().to_string();

        if mutated.len() > self.max_prompt_length {
            let child_hash = compute_prompt_hash(&mutated);
            info!(
                parent_hash = %parent_hash,
                child_hash = %child_hash,
                mutated_len = mutated.len(),
                max = self.max_prompt_length,
                "mutation rejected: exceeds max_prompt_length"
            );
            return Ok(MutationResult {
                prompt: current_prompt.to_string(),
                rationale: format!(
                    "rejected: mutated prompt length {} exceeds max {}",
                    mutated.len(),
                    self.max_prompt_length
                ),
                rejected: true,
            });
        }

        let growth_pct = if current_prompt.is_empty() {
            0
        } else {
            let growth = mutated.len() as i64 - current_prompt.len() as i64;
            if growth > 0 {
                (growth as usize * 100) / current_prompt.len()
            } else {
                0
            }
        };

        if growth_pct > self.max_prompt_growth_pct {
            let child_hash = compute_prompt_hash(&mutated);
            info!(
                parent_hash = %parent_hash,
                child_hash = %child_hash,
                growth_pct = growth_pct,
                max_pct = self.max_prompt_growth_pct,
                "mutation rejected: exceeds max_prompt_growth_pct"
            );
            return Ok(MutationResult {
                prompt: current_prompt.to_string(),
                rationale: format!(
                    "rejected: growth {growth_pct}% exceeds max {}%",
                    self.max_prompt_growth_pct
                ),
                rejected: true,
            });
        }

        let child_hash = compute_prompt_hash(&mutated);
        let diff_size = mutated.len() as i64 - current_prompt.len() as i64;
        info!(
            parent_hash = %parent_hash,
            child_hash = %child_hash,
            diff_size = diff_size,
            "mutation accepted"
        );

        Ok(MutationResult {
            prompt: mutated,
            rationale: format!("mutated (diff_size: {diff_size})"),
            rejected: false,
        })
    }
}

pub struct TemplateMutator {
    variables: std::collections::HashMap<String, Vec<String>>,
}

impl TemplateMutator {
    pub fn new(variables: std::collections::HashMap<String, Vec<String>>) -> Self {
        Self { variables }
    }
}

#[async_trait]
impl Mutator for TemplateMutator {
    async fn mutate(
        &self,
        current_prompt: &str,
        _last_response: &str,
        _last_score: f64,
        iteration: usize,
    ) -> Result<MutationResult, ExperimentError> {
        let mut result = current_prompt.to_string();
        for (key, values) in &self.variables {
            if !values.is_empty() {
                let placeholder = format!("{{{{{key}}}}}");
                let value = &values[iteration % values.len()];
                result = result.replace(&placeholder, value);
            }
        }
        Ok(MutationResult {
            prompt: result,
            rationale: format!("template substitution iteration {iteration}"),
            rejected: false,
        })
    }
}

pub fn create_mutator(
    config: &MutationConfig,
    default_driver: Option<Arc<dyn LlmDriver>>,
    default_model: &str,
) -> Result<Box<dyn Mutator>, ExperimentError> {
    match config {
        MutationConfig::LlmMutator {
            mutator_model,
            max_prompt_growth_pct,
            max_prompt_length,
        } => {
            let driver = default_driver
                .ok_or_else(|| ExperimentError::Mutation("LlmMutator requires a driver".into()))?;
            let model = mutator_model
                .as_ref()
                .map(|m| m.model.clone())
                .unwrap_or_else(|| default_model.to_string());
            Ok(Box::new(LlmMutator::new(
                driver,
                model,
                *max_prompt_growth_pct,
                *max_prompt_length,
            )))
        }
        MutationConfig::TemplateMutator { variables } => {
            Ok(Box::new(TemplateMutator::new(variables.clone())))
        }
    }
}
