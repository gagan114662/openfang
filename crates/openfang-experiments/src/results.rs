use crate::ExperimentError;
use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct IterationResult {
    pub iteration: usize,
    pub prompt_hash: String,
    pub parent_prompt_hash: String,
    pub score: f64,
    pub score_reasoning: String,
    pub response_preview: String,
    pub tokens_input: u64,
    pub tokens_output: u64,
    pub cost_usd: Option<f64>,
    pub improved: bool,
    pub failure_type: Option<String>,
    pub mutation_strategy: String,
    pub scoring_strategy: String,
    pub mutation_diff_size: Option<i64>,
    pub timestamp: String,
    pub prompt_length: usize,
}

pub struct ResultsLogger {
    path: PathBuf,
}

impl ResultsLogger {
    pub fn new(output_dir: &Path, experiment_name: &str) -> Result<Self, ExperimentError> {
        fs::create_dir_all(output_dir)?;
        let path = output_dir.join(format!("{experiment_name}_results.jsonl"));
        Ok(Self { path })
    }

    pub fn log(&self, result: &IterationResult) -> Result<(), ExperimentError> {
        let line = serde_json::to_string(result)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn save_best_prompt(
    output_dir: &Path,
    experiment_name: &str,
    prompt: &str,
    score: f64,
    iteration: usize,
    prompt_hash: &str,
) -> Result<PathBuf, ExperimentError> {
    fs::create_dir_all(output_dir)?;
    let prompt_path = output_dir.join(format!("{experiment_name}_best_prompt.txt"));
    fs::write(&prompt_path, prompt)?;

    let meta_path = output_dir.join(format!("{experiment_name}_best_prompt_meta.json"));
    let meta = serde_json::json!({
        "score": score,
        "iteration": iteration,
        "prompt_hash": prompt_hash,
        "prompt_length": prompt.len(),
        "saved_at": Utc::now().to_rfc3339(),
    });
    fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
    Ok(prompt_path)
}

pub fn compute_prompt_hash(prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_prompt_hash_deterministic() {
        let h1 = compute_prompt_hash("hello world");
        let h2 = compute_prompt_hash("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn test_compute_prompt_hash_differs() {
        let h1 = compute_prompt_hash("prompt a");
        let h2 = compute_prompt_hash("prompt b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_results_logger_writes_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let logger = ResultsLogger::new(dir.path(), "test_exp").unwrap();
        let result = IterationResult {
            iteration: 0,
            prompt_hash: "abc123".into(),
            parent_prompt_hash: "000000".into(),
            score: 75.0,
            score_reasoning: "matched 3/4".into(),
            response_preview: "Hello...".into(),
            tokens_input: 100,
            tokens_output: 50,
            cost_usd: None,
            improved: true,
            failure_type: None,
            mutation_strategy: "baseline".into(),
            scoring_strategy: "regex_match".into(),
            mutation_diff_size: None,
            timestamp: Utc::now().to_rfc3339(),
            prompt_length: 42,
        };
        logger.log(&result).unwrap();
        let content = fs::read_to_string(logger.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["iteration"], 0);
        assert_eq!(parsed["score"], 75.0);
        assert_eq!(parsed["improved"], true);
        assert!(parsed["failure_type"].is_null());
        assert_eq!(parsed["parent_prompt_hash"], "000000");
    }

    #[test]
    fn test_save_best_prompt_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_best_prompt(
            dir.path(),
            "test_exp",
            "best prompt text",
            90.0,
            3,
            "abc123",
        )
        .unwrap();
        assert!(path.exists());
        let prompt_text = fs::read_to_string(&path).unwrap();
        assert_eq!(prompt_text, "best prompt text");

        let meta_path = dir.path().join("test_exp_best_prompt_meta.json");
        assert!(meta_path.exists());
        let meta: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(meta_path).unwrap()).unwrap();
        assert_eq!(meta["score"], 90.0);
        assert_eq!(meta["iteration"], 3);
    }
}
