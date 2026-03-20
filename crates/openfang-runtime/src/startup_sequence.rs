//! Startup sequence protocol for agent sessions.
//!
//! Defines the canonical 5-step startup sequence that agents execute
//! when beginning a new session, providing orientation context from
//! progress and feature state.

use serde::{Deserialize, Serialize};

/// Steps in the agent startup sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupStep {
    CheckWorkDir,
    ReadProgress,
    ReadFeatures,
    RunInit,
    SmokeTest,
}

impl StartupStep {
    pub fn label(&self) -> &'static str {
        match self {
            Self::CheckWorkDir => "Check working directory",
            Self::ReadProgress => "Read PROGRESS.md",
            Self::ReadFeatures => "Read FEATURES.json",
            Self::RunInit => "Run init.sh",
            Self::SmokeTest => "Run smoke test",
        }
    }

    pub fn order(&self) -> u8 {
        match self {
            Self::CheckWorkDir => 0,
            Self::ReadProgress => 1,
            Self::ReadFeatures => 2,
            Self::RunInit => 3,
            Self::SmokeTest => 4,
        }
    }
}

/// Result of executing a single startup step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step: StartupStep,
    pub passed: bool,
    pub detail: String,
}

/// The full startup sequence with results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartupSequence {
    pub results: Vec<StepResult>,
}

impl StartupSequence {
    pub fn all_passed(&self) -> bool {
        self.results.iter().all(|r| r.passed)
    }

    pub fn first_failure(&self) -> Option<&StepResult> {
        self.results.iter().find(|r| !r.passed)
    }

    pub fn passed_count(&self) -> usize {
        self.results.iter().filter(|r| r.passed).count()
    }

    pub fn total_count(&self) -> usize {
        self.results.len()
    }
}

/// Return the canonical 5-step startup sequence.
pub fn default_sequence() -> Vec<StartupStep> {
    vec![
        StartupStep::CheckWorkDir,
        StartupStep::ReadProgress,
        StartupStep::ReadFeatures,
        StartupStep::RunInit,
        StartupStep::SmokeTest,
    ]
}

/// Build an orientation context section from progress and features summaries.
pub fn orientation_context(
    progress_summary: &str,
    features_summary: &str,
    work_dir: &str,
) -> String {
    let mut ctx = String::new();
    ctx.push_str("## Session Orientation\n\n");
    ctx.push_str(&format!("Working directory: `{work_dir}`\n\n"));

    if !progress_summary.is_empty() {
        ctx.push_str("### Progress\n");
        ctx.push_str(progress_summary);
        ctx.push_str("\n\n");
    }

    if !features_summary.is_empty() {
        ctx.push_str("### Feature Status\n");
        ctx.push_str(features_summary);
        ctx.push_str("\n\n");
    }

    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_sequence_order() {
        let seq = default_sequence();
        assert_eq!(seq.len(), 5);
        assert_eq!(seq[0], StartupStep::CheckWorkDir);
        assert_eq!(seq[1], StartupStep::ReadProgress);
        assert_eq!(seq[2], StartupStep::ReadFeatures);
        assert_eq!(seq[3], StartupStep::RunInit);
        assert_eq!(seq[4], StartupStep::SmokeTest);
    }

    #[test]
    fn test_step_ordering_monotonic() {
        let seq = default_sequence();
        for i in 1..seq.len() {
            assert!(seq[i].order() > seq[i - 1].order());
        }
    }

    #[test]
    fn test_startup_sequence_all_passed() {
        let seq = StartupSequence {
            results: vec![
                StepResult {
                    step: StartupStep::CheckWorkDir,
                    passed: true,
                    detail: "Directory exists".to_string(),
                },
                StepResult {
                    step: StartupStep::ReadProgress,
                    passed: true,
                    detail: "3 items pending".to_string(),
                },
            ],
        };
        assert!(seq.all_passed());
        assert_eq!(seq.passed_count(), 2);
        assert_eq!(seq.total_count(), 2);
        assert!(seq.first_failure().is_none());
    }

    #[test]
    fn test_startup_sequence_with_failure() {
        let seq = StartupSequence {
            results: vec![
                StepResult {
                    step: StartupStep::CheckWorkDir,
                    passed: true,
                    detail: "ok".to_string(),
                },
                StepResult {
                    step: StartupStep::RunInit,
                    passed: false,
                    detail: "init.sh not found".to_string(),
                },
            ],
        };
        assert!(!seq.all_passed());
        assert_eq!(seq.passed_count(), 1);
        let fail = seq.first_failure().unwrap();
        assert_eq!(fail.step, StartupStep::RunInit);
    }

    #[test]
    fn test_startup_sequence_empty() {
        let seq = StartupSequence::default();
        assert!(seq.all_passed());
        assert_eq!(seq.passed_count(), 0);
        assert_eq!(seq.total_count(), 0);
    }

    #[test]
    fn test_orientation_context_full() {
        let ctx = orientation_context(
            "- [x] Auth done\n- [ ] Dashboard",
            "5 pass / 2 fail",
            "/tmp/project",
        );
        assert!(ctx.contains("Session Orientation"));
        assert!(ctx.contains("/tmp/project"));
        assert!(ctx.contains("Auth done"));
        assert!(ctx.contains("5 pass / 2 fail"));
    }

    #[test]
    fn test_orientation_context_empty_progress() {
        let ctx = orientation_context("", "all passing", "/tmp/project");
        assert!(!ctx.contains("### Progress"));
        assert!(ctx.contains("### Feature Status"));
    }

    #[test]
    fn test_orientation_context_empty_features() {
        let ctx = orientation_context("some progress", "", "/tmp/project");
        assert!(ctx.contains("### Progress"));
        assert!(!ctx.contains("### Feature Status"));
    }

    #[test]
    fn test_step_labels_not_empty() {
        for step in default_sequence() {
            assert!(!step.label().is_empty());
        }
    }
}
