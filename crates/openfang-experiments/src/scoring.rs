use crate::config::ScoringConfig;
use crate::ExperimentError;
use async_trait::async_trait;
use openfang_runtime::llm_driver::{CompletionRequest, LlmDriver};
use std::sync::Arc;

pub struct ScoreResult {
    pub score: f64,
    pub reasoning: String,
}

#[async_trait]
pub trait Scorer: Send + Sync {
    async fn score(&self, prompt: &str, response: &str) -> Result<ScoreResult, ExperimentError>;
}

pub struct RegexMatchScorer {
    patterns: Vec<regex_lite::Regex>,
    expected_matches: usize,
}

impl RegexMatchScorer {
    pub fn new(patterns: &[String], expected_matches: usize) -> Result<Self, ExperimentError> {
        let compiled: Result<Vec<_>, _> = patterns
            .iter()
            .map(|p| {
                regex_lite::Regex::new(p)
                    .map_err(|e| ExperimentError::Scoring(format!("invalid regex '{p}': {e}")))
            })
            .collect();
        Ok(Self {
            patterns: compiled?,
            expected_matches,
        })
    }
}

#[async_trait]
impl Scorer for RegexMatchScorer {
    async fn score(&self, _prompt: &str, response: &str) -> Result<ScoreResult, ExperimentError> {
        let lower = response.to_lowercase();
        let matched: usize = self
            .patterns
            .iter()
            .filter(|re| re.is_match(&lower))
            .count();
        let score = if self.expected_matches > 0 {
            (matched as f64 / self.expected_matches as f64 * 100.0).min(100.0)
        } else {
            100.0
        };
        let reasoning = format!(
            "matched {matched}/{} patterns (expected {})",
            self.patterns.len(),
            self.expected_matches
        );
        Ok(ScoreResult { score, reasoning })
    }
}

pub struct LlmJudgeScorer {
    driver: Arc<dyn LlmDriver>,
    criteria: String,
    model: String,
}

impl LlmJudgeScorer {
    pub fn new(driver: Arc<dyn LlmDriver>, criteria: String, model: String) -> Self {
        Self {
            driver,
            criteria,
            model,
        }
    }
}

#[async_trait]
impl Scorer for LlmJudgeScorer {
    async fn score(&self, prompt: &str, response: &str) -> Result<ScoreResult, ExperimentError> {
        let judge_prompt = format!(
            "You are a prompt quality judge. Score the following agent response on a scale of 0-100.\n\n\
             Criteria: {}\n\n\
             System prompt being tested:\n{}\n\n\
             Agent response:\n{}\n\n\
             Respond with ONLY a JSON object: {{\"score\": <number 0-100>, \"reasoning\": \"<explanation>\"}}",
            self.criteria, prompt, response
        );
        let messages = vec![openfang_types::message::Message {
            role: openfang_types::message::Role::User,
            content: openfang_types::message::MessageContent::Text(judge_prompt),
        }];
        let request = CompletionRequest {
            model: self.model.clone(),
            messages,
            tools: vec![],
            max_tokens: 512,
            temperature: 0.0,
            system: Some("You are a strict scoring judge. Return only valid JSON.".into()),
            thinking: None,
            sentry_parent_span: None,
        };
        let resp = self
            .driver
            .complete(request)
            .await
            .map_err(|e| ExperimentError::Scoring(format!("judge LLM error: {e}")))?;
        let text = resp.text();
        let parsed: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| {
            ExperimentError::Scoring(format!("judge returned invalid JSON: {e} — raw: {text}"))
        })?;
        let score = parsed["score"]
            .as_f64()
            .ok_or_else(|| ExperimentError::Scoring("judge JSON missing 'score' field".into()))?;
        let reasoning = parsed["reasoning"]
            .as_str()
            .unwrap_or("no reasoning provided")
            .to_string();
        Ok(ScoreResult { score, reasoning })
    }
}

pub fn create_scorer(
    config: &ScoringConfig,
    default_driver: Option<Arc<dyn LlmDriver>>,
    default_model: &str,
) -> Result<Box<dyn Scorer>, ExperimentError> {
    match config {
        ScoringConfig::RegexMatch {
            patterns,
            expected_matches,
        } => Ok(Box::new(RegexMatchScorer::new(
            patterns,
            *expected_matches,
        )?)),
        ScoringConfig::LlmJudge {
            criteria,
            judge_model,
        } => {
            let driver = default_driver
                .ok_or_else(|| ExperimentError::Scoring("LlmJudge requires a driver".into()))?;
            let model = judge_model
                .as_ref()
                .map(|m| m.model.clone())
                .unwrap_or_else(|| default_model.to_string());
            Ok(Box::new(LlmJudgeScorer::new(
                driver,
                criteria.clone(),
                model,
            )))
        }
    }
}
