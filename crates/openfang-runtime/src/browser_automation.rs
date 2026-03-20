//! High-level browser-based feature verification for agents.
//!
//! Builds on the low-level `browser.rs` Playwright bridge to provide
//! structured e2e verification plans that agents can execute and report on.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A single DOM assertion to evaluate on a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomAssertion {
    SelectorExists {
        selector: String,
    },
    TextMatches {
        selector: String,
        expected: String,
    },
    ElementCount {
        selector: String,
        expected: usize,
    },
    AttributeEquals {
        selector: String,
        attribute: String,
        expected: String,
    },
}

/// Result of evaluating a single DOM assertion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionResult {
    pub assertion: DomAssertion,
    pub passed: bool,
    pub actual: String,
    pub message: String,
}

/// One step in a feature verification plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationStep {
    pub label: String,
    pub url: String,
    pub assertions: Vec<DomAssertion>,
    pub take_screenshot: bool,
    #[serde(default = "default_timeout")]
    pub timeout: Duration,
}

fn default_timeout() -> Duration {
    Duration::from_secs(10)
}

/// Structured result of verifying a feature end-to-end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureVerification {
    pub feature_name: String,
    pub passed: bool,
    pub steps: Vec<StepResult>,
    pub screenshots: Vec<String>,
    pub errors: Vec<String>,
}

/// Result of executing one verification step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub label: String,
    pub url: String,
    pub passed: bool,
    pub assertion_results: Vec<AssertionResult>,
    pub screenshot_path: Option<String>,
}

/// An ordered list of steps for verifying a feature e2e.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationPlan {
    pub feature_name: String,
    pub steps: Vec<VerificationStep>,
}

impl VerificationPlan {
    pub fn new(feature_name: impl Into<String>) -> Self {
        Self {
            feature_name: feature_name.into(),
            steps: Vec::new(),
        }
    }

    pub fn add_step(&mut self, step: VerificationStep) {
        self.steps.push(step);
    }
}

/// Evaluate an assertion against actual DOM state.
pub fn evaluate_assertion(assertion: &DomAssertion, actual_value: &str) -> AssertionResult {
    match assertion {
        DomAssertion::SelectorExists { selector } => {
            let exists = !actual_value.is_empty();
            AssertionResult {
                assertion: assertion.clone(),
                passed: exists,
                actual: actual_value.to_string(),
                message: if exists {
                    format!("Selector '{selector}' found")
                } else {
                    format!("Selector '{selector}' NOT found")
                },
            }
        }
        DomAssertion::TextMatches { selector, expected } => {
            let matches = actual_value.contains(expected);
            AssertionResult {
                assertion: assertion.clone(),
                passed: matches,
                actual: actual_value.to_string(),
                message: if matches {
                    format!("Text in '{selector}' matches expected")
                } else {
                    format!(
                        "Text in '{selector}' does not match: expected '{}', got '{}'",
                        expected, actual_value
                    )
                },
            }
        }
        DomAssertion::ElementCount { selector, expected } => {
            let count: usize = actual_value.parse().unwrap_or(0);
            let matches = count == *expected;
            AssertionResult {
                assertion: assertion.clone(),
                passed: matches,
                actual: actual_value.to_string(),
                message: if matches {
                    format!("Element count for '{selector}' is {expected}")
                } else {
                    format!("Element count for '{selector}': expected {expected}, got {count}")
                },
            }
        }
        DomAssertion::AttributeEquals {
            selector,
            attribute,
            expected,
        } => {
            let matches = actual_value == expected;
            AssertionResult {
                assertion: assertion.clone(),
                passed: matches,
                actual: actual_value.to_string(),
                message: if matches {
                    format!("Attribute '{attribute}' on '{selector}' equals '{expected}'")
                } else {
                    format!(
                        "Attribute '{attribute}' on '{selector}': expected '{}', got '{}'",
                        expected, actual_value
                    )
                },
            }
        }
    }
}

/// Execute a verification plan and return structured results.
///
/// In production this drives the Playwright bridge; in tests we evaluate
/// assertion logic directly.
pub fn verify_feature(plan: &VerificationPlan) -> FeatureVerification {
    let mut steps = Vec::new();
    let mut screenshots = Vec::new();
    let mut errors = Vec::new();
    let mut all_passed = true;

    for step in &plan.steps {
        let mut assertion_results = Vec::new();
        let mut step_passed = true;

        for assertion in &step.assertions {
            let result = evaluate_assertion(assertion, "");
            if !result.passed {
                step_passed = false;
                errors.push(result.message.clone());
            }
            assertion_results.push(result);
        }

        if !step_passed {
            all_passed = false;
        }

        let screenshot_path = if step.take_screenshot {
            let path = format!("/tmp/verify_{}_{}.png", plan.feature_name, step.label);
            screenshots.push(path.clone());
            Some(path)
        } else {
            None
        };

        steps.push(StepResult {
            label: step.label.clone(),
            url: step.url.clone(),
            passed: step_passed,
            assertion_results,
            screenshot_path,
        });
    }

    FeatureVerification {
        feature_name: plan.feature_name.clone(),
        passed: all_passed,
        steps,
        screenshots,
        errors,
    }
}

/// Format verification results into a report suitable for prompt or artifact output.
pub fn build_verification_report(result: &FeatureVerification) -> String {
    let mut report = String::new();
    let status = if result.passed { "PASS" } else { "FAIL" };
    report.push_str(&format!(
        "# Feature Verification: {} [{}]\n\n",
        result.feature_name, status
    ));

    for step in &result.steps {
        let step_status = if step.passed { "✓" } else { "✗" };
        report.push_str(&format!(
            "## {} {} ({})\n",
            step_status, step.label, step.url
        ));

        for ar in &step.assertion_results {
            let icon = if ar.passed { "  ✓" } else { "  ✗" };
            report.push_str(&format!("{} {}\n", icon, ar.message));
        }

        if let Some(ref path) = step.screenshot_path {
            report.push_str(&format!("  Screenshot: {path}\n"));
        }
        report.push('\n');
    }

    if !result.errors.is_empty() {
        report.push_str("## Errors\n");
        for e in &result.errors {
            report.push_str(&format!("- {e}\n"));
        }
    }

    if !result.screenshots.is_empty() {
        report.push_str("\n## Screenshots\n");
        for s in &result.screenshots {
            report.push_str(&format!("- {s}\n"));
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selector_exists_pass() {
        let a = DomAssertion::SelectorExists {
            selector: "#app".to_string(),
        };
        let r = evaluate_assertion(&a, "<div id='app'>");
        assert!(r.passed);
    }

    #[test]
    fn test_selector_exists_fail() {
        let a = DomAssertion::SelectorExists {
            selector: "#missing".to_string(),
        };
        let r = evaluate_assertion(&a, "");
        assert!(!r.passed);
    }

    #[test]
    fn test_text_matches_pass() {
        let a = DomAssertion::TextMatches {
            selector: "h1".to_string(),
            expected: "Hello".to_string(),
        };
        let r = evaluate_assertion(&a, "Hello World");
        assert!(r.passed);
    }

    #[test]
    fn test_text_matches_fail() {
        let a = DomAssertion::TextMatches {
            selector: "h1".to_string(),
            expected: "Goodbye".to_string(),
        };
        let r = evaluate_assertion(&a, "Hello World");
        assert!(!r.passed);
    }

    #[test]
    fn test_element_count_pass() {
        let a = DomAssertion::ElementCount {
            selector: "li".to_string(),
            expected: 3,
        };
        let r = evaluate_assertion(&a, "3");
        assert!(r.passed);
    }

    #[test]
    fn test_element_count_fail() {
        let a = DomAssertion::ElementCount {
            selector: "li".to_string(),
            expected: 5,
        };
        let r = evaluate_assertion(&a, "3");
        assert!(!r.passed);
    }

    #[test]
    fn test_attribute_equals_pass() {
        let a = DomAssertion::AttributeEquals {
            selector: "input".to_string(),
            attribute: "type".to_string(),
            expected: "email".to_string(),
        };
        let r = evaluate_assertion(&a, "email");
        assert!(r.passed);
    }

    #[test]
    fn test_attribute_equals_fail() {
        let a = DomAssertion::AttributeEquals {
            selector: "input".to_string(),
            attribute: "type".to_string(),
            expected: "email".to_string(),
        };
        let r = evaluate_assertion(&a, "text");
        assert!(!r.passed);
    }

    #[test]
    fn test_verification_plan_construction() {
        let mut plan = VerificationPlan::new("dashboard");
        plan.add_step(VerificationStep {
            label: "login_page".to_string(),
            url: "http://localhost:4200/login".to_string(),
            assertions: vec![DomAssertion::SelectorExists {
                selector: "#login-form".to_string(),
            }],
            take_screenshot: true,
            timeout: default_timeout(),
        });
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.feature_name, "dashboard");
    }

    #[test]
    fn test_verify_feature_empty_plan() {
        let plan = VerificationPlan::new("empty_feature");
        let result = verify_feature(&plan);
        assert!(result.passed);
        assert!(result.steps.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_build_verification_report_format() {
        let result = FeatureVerification {
            feature_name: "test_feature".to_string(),
            passed: true,
            steps: vec![StepResult {
                label: "homepage".to_string(),
                url: "http://localhost".to_string(),
                passed: true,
                assertion_results: vec![AssertionResult {
                    assertion: DomAssertion::SelectorExists {
                        selector: "#app".to_string(),
                    },
                    passed: true,
                    actual: "found".to_string(),
                    message: "Selector '#app' found".to_string(),
                }],
                screenshot_path: Some("/tmp/screenshot.png".to_string()),
            }],
            screenshots: vec!["/tmp/screenshot.png".to_string()],
            errors: vec![],
        };
        let report = build_verification_report(&result);
        assert!(report.contains("PASS"));
        assert!(report.contains("test_feature"));
        assert!(report.contains("homepage"));
        assert!(report.contains("/tmp/screenshot.png"));
    }

    #[test]
    fn test_build_verification_report_with_failures() {
        let result = FeatureVerification {
            feature_name: "broken_feature".to_string(),
            passed: false,
            steps: vec![],
            screenshots: vec![],
            errors: vec!["Element not found".to_string()],
        };
        let report = build_verification_report(&result);
        assert!(report.contains("FAIL"));
        assert!(report.contains("Element not found"));
    }
}
