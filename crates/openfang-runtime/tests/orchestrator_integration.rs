//! Integration test for model orchestrator.

use openfang_runtime::llm_driver::CompletionRequest;
use openfang_runtime::model_orchestrator::{ModelOrchestrator, TaskType};
use openfang_types::config::{ModelSpec, OrchestratorConfig, OrchestratorRouting};
use openfang_types::message::{Message, MessageContent, Role};

#[test]
fn test_orchestrator_routing() {
    let config = OrchestratorConfig {
        enabled: true,
        routing: OrchestratorRouting {
            research: Some(ModelSpec {
                provider: "gemini".to_string(),
                model: "gemini-2.0-flash-exp".to_string(),
            }),
            coding: Some(ModelSpec {
                provider: "anthropic".to_string(),
                model: "claude-opus-4-6".to_string(),
            }),
            quick: None,
            image: None,
        },
    };

    let orchestrator = ModelOrchestrator::new(config);

    // Test research routing
    let research_request = CompletionRequest {
        model: "test-model".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("Research quantum computing advances".to_string()),
        }],
        tools: vec![],
        max_tokens: 1000,
        temperature: 0.7,
        system: None,
        thinking: None,
        sentry_parent_span: None,
    };

    let task_type = orchestrator.classify_task(&research_request);
    assert_eq!(task_type, TaskType::Research);

    let model_spec = orchestrator.select_model(task_type);
    assert!(model_spec.is_some());
    assert_eq!(model_spec.unwrap().provider, "gemini");
}
