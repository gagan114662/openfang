use crate::mutation::{MutationResult, Mutator};
use crate::scoring::{ScoreResult, Scorer};
use crate::ExperimentError;
use async_trait::async_trait;
use openfang_runtime::llm_driver::{
    CompletionRequest, CompletionResponse, LlmDriver, LlmError, StreamEvent,
};
use openfang_types::message::{ContentBlock, StopReason, TokenUsage};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct MockDriver {
    responses: Vec<String>,
    call_index: AtomicUsize,
}

impl MockDriver {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses,
            call_index: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmDriver for MockDriver {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let idx = self.call_index.fetch_add(1, Ordering::SeqCst);
        let text = if self.responses.is_empty() {
            "mock response".to_string()
        } else {
            self.responses[idx % self.responses.len()].clone()
        };
        Ok(CompletionResponse {
            content: vec![ContentBlock::Text { text }],
            stop_reason: StopReason::EndTurn,
            tool_calls: vec![],
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
            },
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<CompletionResponse, LlmError> {
        let response = self.complete(request).await?;
        let text = response.text();
        if !text.is_empty() {
            let _ = tx.send(StreamEvent::TextDelta { text }).await;
        }
        let _ = tx
            .send(StreamEvent::ContentComplete {
                stop_reason: response.stop_reason,
                usage: response.usage,
            })
            .await;
        Ok(response)
    }
}

pub struct MockScorer {
    scores: Vec<f64>,
    call_index: AtomicUsize,
}

impl MockScorer {
    pub fn new(scores: Vec<f64>) -> Self {
        Self {
            scores,
            call_index: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Scorer for MockScorer {
    async fn score(&self, _prompt: &str, _response: &str) -> Result<ScoreResult, ExperimentError> {
        let idx = self.call_index.fetch_add(1, Ordering::SeqCst);
        let score = if self.scores.is_empty() {
            50.0
        } else {
            self.scores[idx % self.scores.len()]
        };
        Ok(ScoreResult {
            score,
            reasoning: format!("mock score {score} (call {idx})"),
        })
    }
}

pub struct MockMutator {
    mutations: Vec<String>,
    call_index: AtomicUsize,
}

impl MockMutator {
    pub fn new(mutations: Vec<String>) -> Self {
        Self {
            mutations,
            call_index: AtomicUsize::new(0),
        }
    }

    pub fn identity() -> Self {
        Self {
            mutations: vec![],
            call_index: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Mutator for MockMutator {
    async fn mutate(
        &self,
        current_prompt: &str,
        _last_response: &str,
        _last_score: f64,
        _iteration: usize,
    ) -> Result<MutationResult, ExperimentError> {
        if self.mutations.is_empty() {
            return Ok(MutationResult {
                prompt: current_prompt.to_string(),
                rationale: "identity mutation".into(),
                rejected: false,
            });
        }
        let idx = self.call_index.fetch_add(1, Ordering::SeqCst);
        let prompt = self.mutations[idx % self.mutations.len()].clone();
        Ok(MutationResult {
            prompt,
            rationale: format!("mock mutation (call {idx})"),
            rejected: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_driver_cycles_responses() {
        let driver = MockDriver::new(vec!["first".into(), "second".into()]);
        let req = CompletionRequest {
            model: "test".into(),
            messages: vec![],
            tools: vec![],
            max_tokens: 100,
            temperature: 0.0,
            system: None,
            thinking: None,
            sentry_parent_span: None,
        };
        let r1 = driver.complete(req.clone()).await.unwrap();
        assert_eq!(r1.text(), "first");
        let r2 = driver.complete(req.clone()).await.unwrap();
        assert_eq!(r2.text(), "second");
        let r3 = driver.complete(req).await.unwrap();
        assert_eq!(r3.text(), "first");
    }

    #[tokio::test]
    async fn test_mock_scorer_cycles_scores() {
        let scorer = MockScorer::new(vec![50.0, 80.0, 60.0]);
        let s1 = scorer.score("", "").await.unwrap();
        assert_eq!(s1.score, 50.0);
        let s2 = scorer.score("", "").await.unwrap();
        assert_eq!(s2.score, 80.0);
        let s3 = scorer.score("", "").await.unwrap();
        assert_eq!(s3.score, 60.0);
    }

    #[tokio::test]
    async fn test_mock_mutator_identity() {
        let mutator = MockMutator::identity();
        let result = mutator.mutate("original", "", 50.0, 0).await.unwrap();
        assert_eq!(result.prompt, "original");
        assert!(!result.rejected);
    }

    #[tokio::test]
    async fn test_mock_mutator_cycles() {
        let mutator = MockMutator::new(vec!["v1".into(), "v2".into()]);
        let r1 = mutator.mutate("", "", 50.0, 0).await.unwrap();
        assert_eq!(r1.prompt, "v1");
        let r2 = mutator.mutate("", "", 50.0, 1).await.unwrap();
        assert_eq!(r2.prompt, "v2");
    }
}
