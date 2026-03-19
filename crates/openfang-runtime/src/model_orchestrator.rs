//! Model orchestration module for routing tasks to specialized models.

use crate::llm_driver::CompletionRequest;
use openfang_types::config::{ModelSpec, OrchestratorConfig};

/// Task type classification for model routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    /// Research or analysis tasks.
    Research,
    /// Code generation or debugging tasks.
    Coding,
    /// Quick Q&A tasks.
    QuickQA,
    /// Image generation tasks.
    ImageGen,
    /// Long context tasks (>10k chars).
    LongContext,
    /// Default/unclassified tasks.
    Default,
}

/// Model orchestrator for task-based model selection.
pub struct ModelOrchestrator {
    config: OrchestratorConfig,
}

impl ModelOrchestrator {
    /// Create a new model orchestrator with the given configuration.
    pub fn new(config: OrchestratorConfig) -> Self {
        Self { config }
    }

    /// Classify a task based on message content.
    pub fn classify_task(&self, request: &CompletionRequest) -> TaskType {
        // Concatenate all message content for analysis
        let content = request
            .messages
            .iter()
            .map(|m| match &m.content {
                openfang_types::message::MessageContent::Text(t) => t.as_str(),
                _ => "",
            })
            .collect::<String>()
            .to_lowercase();

        // Long context check
        if content.len() > 10000 {
            return TaskType::LongContext;
        }

        // Keyword matching for task type classification
        if content.contains("research")
            || content.contains("analyze")
            || content.contains("investigate")
        {
            return TaskType::Research;
        }

        if content.contains("code") || content.contains("implement") || content.contains("debug") {
            return TaskType::Coding;
        }

        if content.contains("image")
            || content.contains("picture")
            || content.contains("generate visual")
        {
            return TaskType::ImageGen;
        }

        // Short queries are quick Q&A
        if content.len() < 200 {
            return TaskType::QuickQA;
        }

        TaskType::Default
    }

    /// Select the appropriate model for a given task type.
    pub fn select_model(&self, task_type: TaskType) -> Option<ModelSpec> {
        if !self.config.enabled {
            return None;
        }

        match task_type {
            TaskType::Research => self.config.routing.research.clone(),
            TaskType::Coding => self.config.routing.coding.clone(),
            TaskType::QuickQA => self.config.routing.quick.clone(),
            TaskType::ImageGen => self.config.routing.image.clone(),
            TaskType::LongContext => self.config.routing.research.clone(), // Gemini has good context
            TaskType::Default => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfang_types::message::{Message, MessageContent, Role};

    #[test]
    fn test_classify_research() {
        let orchestrator = ModelOrchestrator::new(OrchestratorConfig::default());

        let request = CompletionRequest {
            model: "test-model".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Please research Bitcoin trends".to_string()),
            }],
            tools: vec![],
            max_tokens: 1000,
            temperature: 0.7,
            system: None,
            thinking: None,
            sentry_parent_span: None,
        };

        assert_eq!(orchestrator.classify_task(&request), TaskType::Research);
    }

    #[test]
    fn test_classify_coding() {
        let orchestrator = ModelOrchestrator::new(OrchestratorConfig::default());

        let request = CompletionRequest {
            model: "test-model".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Write code for authentication".to_string()),
            }],
            tools: vec![],
            max_tokens: 1000,
            temperature: 0.7,
            system: None,
            thinking: None,
            sentry_parent_span: None,
        };

        assert_eq!(orchestrator.classify_task(&request), TaskType::Coding);
    }

    #[test]
    fn test_classify_quick_qa() {
        let orchestrator = ModelOrchestrator::new(OrchestratorConfig::default());

        let request = CompletionRequest {
            model: "test-model".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("What is 2+2?".to_string()),
            }],
            tools: vec![],
            max_tokens: 1000,
            temperature: 0.7,
            system: None,
            thinking: None,
            sentry_parent_span: None,
        };

        assert_eq!(orchestrator.classify_task(&request), TaskType::QuickQA);
    }
}
