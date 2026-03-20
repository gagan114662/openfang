//! Trait-based delegation detection registry.
//!
//! Generalizes the hardcoded `looks_analytic_request()` into a pluggable
//! detection system. Detectors evaluate user messages and return delegation
//! branch descriptors ranked by confidence.

/// Describes a sub-LLM branch for delegation.
#[derive(Debug, Clone)]
pub struct BranchDescriptor {
    /// Branch name (e.g. "analytics", "code_review", "deep_research").
    pub name: String,
    /// Priority (higher = preferred).
    pub priority: u32,
    /// Projected token budget for this branch.
    pub projected_tokens: usize,
    /// Optional system prompt for the sub-LLM branch.
    pub system_prompt: Option<String>,
}

/// Result of a delegation detector evaluation.
#[derive(Debug, Clone)]
pub struct DetectionResult {
    /// Confidence score (0.0 to 1.0).
    pub confidence: f32,
    /// Suggested delegation branches.
    pub branches: Vec<BranchDescriptor>,
}

/// Metadata passed to detectors for context-aware decisions.
#[derive(Debug, Clone)]
pub struct DetectorMetadata {
    /// Whether RLM is enabled for this agent.
    pub rlm_enabled: bool,
    /// Agent metadata tags (from manifest).
    pub agent_tags: Vec<String>,
    /// Number of datasets loaded in the current session.
    pub dataset_count: usize,
}

/// Trait for delegation detectors.
///
/// Implementations evaluate a user message and return a detection result
/// if delegation is appropriate. Return `None` to abstain.
pub trait DelegationDetector: Send + Sync {
    /// Detector name for logging/debugging.
    fn name(&self) -> &str;

    /// Evaluate whether this message should be delegated.
    fn detect(&self, message: &str, metadata: &DetectorMetadata) -> Option<DetectionResult>;
}

/// Registry of delegation detectors, evaluated in order.
#[derive(Default)]
pub struct DetectorRegistry {
    detectors: Vec<Box<dyn DelegationDetector>>,
}

impl DetectorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new detector.
    pub fn register(&mut self, detector: Box<dyn DelegationDetector>) {
        self.detectors.push(detector);
    }

    /// Evaluate all detectors and return the highest-confidence result.
    pub fn evaluate(&self, message: &str, metadata: &DetectorMetadata) -> Option<DetectionResult> {
        let mut best: Option<DetectionResult> = None;

        for detector in &self.detectors {
            if let Some(result) = detector.detect(message, metadata) {
                if best
                    .as_ref()
                    .is_none_or(|b| result.confidence > b.confidence)
                {
                    best = Some(result);
                }
            }
        }

        best
    }
}

/// Built-in analytics detector — port of `looks_analytic_request()`.
pub struct AnalyticsDetector;

impl DelegationDetector for AnalyticsDetector {
    fn name(&self) -> &str {
        "analytics"
    }

    fn detect(&self, message: &str, metadata: &DetectorMetadata) -> Option<DetectionResult> {
        if !metadata.rlm_enabled {
            return None;
        }

        let lower = message.to_ascii_lowercase();
        let hints = [
            "analy",
            "dataset",
            "csv",
            "json",
            "sqlite",
            "postgres",
            "table",
            "distribution",
            "outlier",
            "quality",
            "trend",
        ];
        let matches = hints.iter().filter(|h| lower.contains(**h)).count();
        if matches == 0 {
            return None;
        }

        let confidence = (matches as f32 * 0.15).min(0.95);

        Some(DetectionResult {
            confidence,
            branches: vec![BranchDescriptor {
                name: "analytics".to_string(),
                priority: 10,
                projected_tokens: 4000,
                system_prompt: Some(
                    "You are a data analyst. Use only evidence-backed claims with citations."
                        .to_string(),
                ),
            }],
        })
    }
}

/// Keyword-based detector for custom branch configs.
pub struct KeywordDetector {
    branch_name: String,
    keywords: Vec<String>,
    priority: u32,
    projected_tokens: usize,
    system_prompt: Option<String>,
}

impl KeywordDetector {
    pub fn new(
        branch_name: String,
        keywords: Vec<String>,
        priority: u32,
        projected_tokens: usize,
        system_prompt: Option<String>,
    ) -> Self {
        Self {
            branch_name,
            keywords,
            priority,
            projected_tokens,
            system_prompt,
        }
    }
}

impl DelegationDetector for KeywordDetector {
    fn name(&self) -> &str {
        &self.branch_name
    }

    fn detect(&self, message: &str, _metadata: &DetectorMetadata) -> Option<DetectionResult> {
        let lower = message.to_ascii_lowercase();
        let matches = self
            .keywords
            .iter()
            .filter(|kw| lower.contains(&kw.to_ascii_lowercase()))
            .count();
        if matches == 0 {
            return None;
        }

        let confidence = (matches as f32 * 0.2).min(0.90);

        Some(DetectionResult {
            confidence,
            branches: vec![BranchDescriptor {
                name: self.branch_name.clone(),
                priority: self.priority,
                projected_tokens: self.projected_tokens,
                system_prompt: self.system_prompt.clone(),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_metadata(rlm_enabled: bool) -> DetectorMetadata {
        DetectorMetadata {
            rlm_enabled,
            agent_tags: Vec::new(),
            dataset_count: 0,
        }
    }

    #[test]
    fn test_analytics_detector_fires_for_dataset_queries() {
        let detector = AnalyticsDetector;
        let meta = test_metadata(true);
        let result = detector.detect("analyze this csv dataset for trends", &meta);
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.confidence > 0.0);
        assert_eq!(result.branches[0].name, "analytics");
    }

    #[test]
    fn test_analytics_detector_skips_non_analytics() {
        let detector = AnalyticsDetector;
        let meta = test_metadata(true);
        let result = detector.detect("write a haiku about rust", &meta);
        assert!(result.is_none());
    }

    #[test]
    fn test_analytics_detector_skips_when_rlm_disabled() {
        let detector = AnalyticsDetector;
        let meta = test_metadata(false);
        let result = detector.detect("analyze this csv dataset", &meta);
        assert!(result.is_none());
    }

    #[test]
    fn test_registry_returns_highest_confidence() {
        let mut registry = DetectorRegistry::new();

        // Low-confidence detector
        registry.register(Box::new(KeywordDetector::new(
            "low".to_string(),
            vec!["test".to_string()],
            1,
            1000,
            None,
        )));

        // High-confidence detector (more keyword matches)
        registry.register(Box::new(KeywordDetector::new(
            "high".to_string(),
            vec!["test".to_string(), "code".to_string(), "review".to_string()],
            5,
            2000,
            None,
        )));

        let meta = test_metadata(true);
        let result = registry
            .evaluate("review this test code carefully", &meta)
            .unwrap();
        assert_eq!(result.branches[0].name, "high");
    }

    #[test]
    fn test_empty_registry_returns_none() {
        let registry = DetectorRegistry::new();
        let meta = test_metadata(true);
        let result = registry.evaluate("anything goes", &meta);
        assert!(result.is_none());
    }
}
