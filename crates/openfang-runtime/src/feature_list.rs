//! FEATURES.json parser for structured feature status tracking.
//!
//! Follows the Anthropic `feature_list.json` pattern: a structured checklist
//! of features with pass/fail status that agents can read on startup to
//! understand what's working vs broken.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A list of features with their statuses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureList {
    pub features: Vec<Feature>,
}

/// A single feature entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    #[serde(default)]
    pub test: String,
    pub status: FeatureStatus,
    #[serde(default)]
    pub group: String,
}

/// Feature status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureStatus {
    Pass,
    Fail,
    Blocked,
    Unknown,
}

impl FeatureList {
    /// Parse a FEATURES.json string.
    pub fn parse(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    /// One-line summary: "Features: N pass, M fail, K blocked. Failing: [...]"
    pub fn summary(&self) -> String {
        let mut pass = 0usize;
        let mut fail = 0usize;
        let mut blocked = 0usize;
        let mut unknown = 0usize;
        let mut failing_names: Vec<&str> = Vec::new();

        for f in &self.features {
            match f.status {
                FeatureStatus::Pass => pass += 1,
                FeatureStatus::Fail => {
                    fail += 1;
                    failing_names.push(&f.name);
                }
                FeatureStatus::Blocked => blocked += 1,
                FeatureStatus::Unknown => unknown += 1,
            }
        }

        let mut out = format!(
            "Features: {} pass, {} fail, {} blocked",
            pass, fail, blocked
        );
        if unknown > 0 {
            out.push_str(&format!(", {} unknown", unknown));
        }
        out.push('.');
        if !failing_names.is_empty() {
            out.push_str(&format!(" Failing: [{}]", failing_names.join(", ")));
        }
        out
    }

    /// Return only the failing features.
    pub fn failing(&self) -> Vec<&Feature> {
        self.features
            .iter()
            .filter(|f| f.status == FeatureStatus::Fail)
            .collect()
    }

    /// Group features by their `group` field.
    pub fn by_group(&self) -> BTreeMap<String, Vec<&Feature>> {
        let mut map: BTreeMap<String, Vec<&Feature>> = BTreeMap::new();
        for f in &self.features {
            map.entry(f.group.clone()).or_default().push(f);
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "features": [
                { "name": "LLM completion", "test": "cargo test llm_driver", "status": "pass", "group": "core" },
                { "name": "Telegram channel", "test": "cargo test telegram", "status": "fail", "group": "channels" },
                { "name": "Discord channel", "test": "", "status": "pass", "group": "channels" },
                { "name": "Web search", "test": "cargo test web", "status": "blocked", "group": "tools" }
            ]
        }"#
    }

    #[test]
    fn test_parse_valid_json() {
        let fl = FeatureList::parse(sample_json()).unwrap();
        assert_eq!(fl.features.len(), 4);
        assert_eq!(fl.features[0].name, "LLM completion");
        assert_eq!(fl.features[0].status, FeatureStatus::Pass);
        assert_eq!(fl.features[1].status, FeatureStatus::Fail);
    }

    #[test]
    fn test_summary_format() {
        let fl = FeatureList::parse(sample_json()).unwrap();
        let summary = fl.summary();
        assert!(summary.contains("2 pass"));
        assert!(summary.contains("1 fail"));
        assert!(summary.contains("1 blocked"));
        assert!(summary.contains("Failing: [Telegram channel]"));
    }

    #[test]
    fn test_failing_returns_only_failures() {
        let fl = FeatureList::parse(sample_json()).unwrap();
        let failing = fl.failing();
        assert_eq!(failing.len(), 1);
        assert_eq!(failing[0].name, "Telegram channel");
    }

    #[test]
    fn test_by_group_organizes() {
        let fl = FeatureList::parse(sample_json()).unwrap();
        let groups = fl.by_group();
        assert_eq!(groups.len(), 3); // core, channels, tools
        assert_eq!(groups["channels"].len(), 2);
        assert_eq!(groups["core"].len(), 1);
        assert_eq!(groups["tools"].len(), 1);
    }

    #[test]
    fn test_empty_list() {
        let fl = FeatureList::parse(r#"{"features": []}"#).unwrap();
        assert_eq!(fl.summary(), "Features: 0 pass, 0 fail, 0 blocked.");
        assert!(fl.failing().is_empty());
        assert!(fl.by_group().is_empty());
    }

    #[test]
    fn test_unknown_status() {
        let json = r#"{
            "features": [
                { "name": "mystery", "status": "unknown", "group": "misc" }
            ]
        }"#;
        let fl = FeatureList::parse(json).unwrap();
        assert_eq!(fl.features[0].status, FeatureStatus::Unknown);
        let summary = fl.summary();
        assert!(summary.contains("1 unknown"));
    }
}
