use crate::llm_driver::{CompletionRequest, LlmDriver};
use crate::rlm_dataset::RlmFrame;
use crate::rlm_provenance::ProvenanceLedger;
use futures::future::join_all;
use openfang_types::config::RlmConfig;
use openfang_types::message::Message;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchKind {
    Categorize,
    Anomaly,
    Distribution,
    Quality,
    SynthesisPrep,
}

impl BranchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BranchKind::Categorize => "categorize",
            BranchKind::Anomaly => "anomaly",
            BranchKind::Distribution => "distribution",
            BranchKind::Quality => "quality",
            BranchKind::SynthesisPrep => "synthesis_prep",
        }
    }

    fn priority(self) -> u8 {
        match self {
            BranchKind::Categorize => 5,
            BranchKind::Quality => 4,
            BranchKind::Distribution => 3,
            BranchKind::Anomaly => 2,
            BranchKind::SynthesisPrep => 1,
        }
    }

    fn projected_tokens(self) -> usize {
        match self {
            BranchKind::Categorize => 600,
            BranchKind::Quality => 700,
            BranchKind::Distribution => 900,
            BranchKind::Anomaly => 1200,
            BranchKind::SynthesisPrep => 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RlmFinding {
    pub branch: String,
    pub finding: String,
    pub evidence_ids: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FanoutResponse {
    pub planned_branches: Vec<String>,
    pub executed_branches: Vec<String>,
    pub dropped_branches: Vec<String>,
    pub findings: Vec<RlmFinding>,
    pub branch_errors: Vec<String>,
}

pub fn plan_branches(question: &str, frames: &[RlmFrame]) -> Vec<BranchKind> {
    let lowered = question.to_ascii_lowercase();
    let mut branches = vec![BranchKind::Categorize, BranchKind::SynthesisPrep];

    if lowered.contains("quality")
        || lowered.contains("missing")
        || lowered.contains("null")
        || frames.iter().any(|f| f.profile.null_cells > 0)
    {
        branches.push(BranchKind::Quality);
    }

    if lowered.contains("distribution")
        || lowered.contains("histogram")
        || lowered.contains("percentile")
        || frames.iter().any(|f| !f.profile.numeric_columns.is_empty())
    {
        branches.push(BranchKind::Distribution);
    }

    if lowered.contains("anomaly")
        || lowered.contains("outlier")
        || lowered.contains("spike")
        || lowered.contains("weird")
    {
        branches.push(BranchKind::Anomaly);
    }

    branches.sort_by_key(|b| std::cmp::Reverse(b.priority()));
    branches.dedup();
    branches
}

pub fn apply_budget_degrade(
    branches: &[BranchKind],
    cfg: &RlmConfig,
) -> (Vec<BranchKind>, Vec<BranchKind>) {
    let hard_cap = ((cfg.max_fanout_tokens as f32) * cfg.degrade_threshold)
        .round()
        .clamp(1.0, cfg.max_fanout_tokens as f32) as usize;

    let mut selected = Vec::new();
    let mut dropped = Vec::new();
    let mut used = 0usize;

    for b in branches.iter().copied() {
        let projected = b.projected_tokens();
        let within_budget = used + projected <= hard_cap;
        let within_parallel = selected.len() < cfg.max_parallel_branches;
        if within_budget && within_parallel {
            selected.push(b);
            used += projected;
        } else {
            dropped.push(b);
        }
    }

    (selected, dropped)
}

pub fn validate_findings(
    findings: Vec<RlmFinding>,
    ledger: &ProvenanceLedger,
) -> (Vec<RlmFinding>, usize) {
    let mut valid = Vec::new();
    let mut dropped = 0usize;
    for f in findings {
        if ledger.validate_ids(&f.evidence_ids) {
            valid.push(f);
        } else {
            dropped += 1;
        }
    }
    (valid, dropped)
}

pub async fn run_fanout_llm(
    question: &str,
    frames: &[RlmFrame],
    ledger: &ProvenanceLedger,
    cfg: &RlmConfig,
    driver: Arc<dyn LlmDriver>,
    model: &str,
) -> FanoutResponse {
    let planned = plan_branches(question, frames);
    let (selected, dropped) = apply_budget_degrade(&planned, cfg);
    let evidence_index = build_dataset_evidence_index(ledger);

    let sem = Arc::new(Semaphore::new(cfg.max_parallel_branches.max(1)));
    let mut futures = Vec::new();

    for branch in selected.iter().copied() {
        let permit_sem = sem.clone();
        let frames = frames.to_vec();
        let evidence_index = evidence_index.clone();
        let question = question.to_string();
        let model = model.to_string();
        let driver = driver.clone();
        let max_tokens =
            ((cfg.max_fanout_tokens / selected.len().max(1)).max(256) as u32).min(2048);
        let fut = async move {
            let _permit = permit_sem
                .acquire_owned()
                .await
                .map_err(|e| format!("Semaphore acquire failed for {}: {e}", branch.as_str()))?;
            execute_branch_llm(
                branch,
                &question,
                &frames,
                &evidence_index,
                driver,
                &model,
                max_tokens,
            )
            .await
        };
        futures.push(fut);
    }

    let mut findings = Vec::new();
    let mut branch_errors = Vec::new();
    for result in join_all(futures).await {
        match result {
            Ok(mut f) => findings.append(&mut f),
            Err(e) => branch_errors.push(e),
        }
    }

    let (findings, dropped_invalid) = validate_findings(findings, ledger);
    if dropped_invalid > 0 {
        branch_errors.push(format!(
            "Dropped {dropped_invalid} uncited/invalid finding(s)."
        ));
    }

    FanoutResponse {
        planned_branches: planned.iter().map(|b| b.as_str().to_string()).collect(),
        executed_branches: selected.iter().map(|b| b.as_str().to_string()).collect(),
        dropped_branches: dropped.iter().map(|b| b.as_str().to_string()).collect(),
        findings,
        branch_errors,
    }
}

fn build_dataset_evidence_index(ledger: &ProvenanceLedger) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for entry in &ledger.entries {
        map.entry(entry.dataset_id.clone())
            .or_insert_with(|| entry.evidence_id.clone());
    }
    map
}

#[derive(Debug, Deserialize)]
struct BranchFindingsEnvelope {
    findings: Vec<BranchFinding>,
}

#[derive(Debug, Deserialize)]
struct BranchFinding {
    finding: String,
    evidence_ids: Vec<String>,
    confidence: Option<f32>,
}

async fn execute_branch_llm(
    branch: BranchKind,
    question: &str,
    frames: &[RlmFrame],
    evidence: &HashMap<String, String>,
    driver: Arc<dyn LlmDriver>,
    model: &str,
    max_tokens: u32,
) -> Result<Vec<RlmFinding>, String> {
    let prompt = branch_prompt(branch, question, frames, evidence);
    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![Message::user(prompt)],
        tools: vec![],
        max_tokens,
        temperature: 0.2,
        system: Some(
            "You are a strict analytics sub-LLM. Return ONLY valid JSON with evidence IDs from the allowed list."
                .to_string(),
        ),
        thinking: None,
        sentry_parent_span: None,
    };

    let response = driver
        .complete(request)
        .await
        .map_err(|e| format!("{} branch LLM error: {e}", branch.as_str()))?;
    let text = response.text();

    let parsed = parse_branch_findings(&text).map_err(|e| {
        format!(
            "{} branch returned non-JSON output: {e}; output_snippet={}",
            branch.as_str(),
            text.chars().take(160).collect::<String>()
        )
    })?;

    let findings = parsed
        .findings
        .into_iter()
        .map(|f| RlmFinding {
            branch: branch.as_str().to_string(),
            finding: f.finding,
            evidence_ids: f.evidence_ids,
            confidence: f.confidence.unwrap_or(0.7).clamp(0.0, 1.0),
        })
        .collect::<Vec<_>>();

    Ok(findings)
}

fn parse_branch_findings(text: &str) -> Result<BranchFindingsEnvelope, String> {
    if let Ok(v) = serde_json::from_str::<BranchFindingsEnvelope>(text.trim()) {
        return Ok(v);
    }

    let first = text.find('{').ok_or("No JSON object start found")?;
    let last = text.rfind('}').ok_or("No JSON object end found")?;
    if first >= last {
        return Err("Invalid JSON object bounds".to_string());
    }
    let candidate = &text[first..=last];
    serde_json::from_str::<BranchFindingsEnvelope>(candidate)
        .map_err(|e| format!("Failed to parse branch JSON payload: {e}"))
}

fn branch_prompt(
    branch: BranchKind,
    question: &str,
    frames: &[RlmFrame],
    evidence: &HashMap<String, String>,
) -> String {
    let mut dataset_summaries = Vec::new();
    let mut allowed_evidence = Vec::new();
    for f in frames {
        if let Some(eid) = evidence.get(&f.dataset_id) {
            allowed_evidence.push(eid.clone());
        }
        dataset_summaries.push(json!({
            "dataset_id": f.dataset_id,
            "source_id": f.source_id,
            "row_count": f.profile.row_count,
            "column_count": f.profile.column_count,
            "numeric_columns": f.profile.numeric_columns,
            "null_cells": f.profile.null_cells,
            "sample_rows": f.rows.iter().take(5).collect::<Vec<_>>(),
        }));
    }

    format!(
        "Analyze branch '{branch}' for the user question.\n\nQuestion:\n{question}\n\nDatasets:\n{datasets}\n\nAllowed evidence IDs (must use only these):\n{allowed}\n\nOutput JSON only in this schema:\n{{\"findings\":[{{\"finding\":\"...\",\"evidence_ids\":[\"...\"],\"confidence\":0.0}}]}}\n\nRules:\n- Every finding must have at least one evidence_id from allowed list.\n- No markdown, no prose, only JSON.",
        branch = branch.as_str(),
        datasets = serde_json::to_string_pretty(&dataset_summaries).unwrap_or_default(),
        allowed = serde_json::to_string_pretty(&allowed_evidence).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rlm_dataset::{DatasetProfile, RlmFrame};
    use crate::rlm_provenance::ProvenanceLedger;

    fn frame(id: &str, rows: usize, numeric: &[&str], null_cells: usize) -> RlmFrame {
        RlmFrame {
            dataset_id: id.to_string(),
            source_id: format!("file:{id}.csv"),
            query_id: "q1".to_string(),
            columns: vec!["a".to_string(), "b".to_string()],
            rows: vec![],
            profile: DatasetProfile {
                row_count: rows,
                column_count: 2,
                numeric_columns: numeric.iter().map(|s| s.to_string()).collect(),
                null_cells,
            },
        }
    }

    #[test]
    fn planner_is_adaptive() {
        let f = frame("d1", 100, &["value"], 5);
        let a = plan_branches(
            "check quality and outlier anomalies",
            std::slice::from_ref(&f),
        );
        let b = plan_branches("quick summary", std::slice::from_ref(&f));
        assert!(a.len() > b.len());
        assert!(a.contains(&BranchKind::Anomaly));
        assert!(!b.contains(&BranchKind::Anomaly));
    }

    #[test]
    fn degrade_drops_low_priority_deterministically() {
        let cfg = RlmConfig {
            max_parallel_branches: 2,
            max_fanout_tokens: 900,
            degrade_threshold: 1.0,
            ..RlmConfig::default()
        };
        let planned = vec![
            BranchKind::Categorize,
            BranchKind::Quality,
            BranchKind::Distribution,
            BranchKind::Anomaly,
            BranchKind::SynthesisPrep,
        ];
        let (selected, dropped) = apply_budget_degrade(&planned, &cfg);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0], BranchKind::Categorize);
        assert!(dropped.contains(&BranchKind::Anomaly));
    }

    #[test]
    fn citation_validator_rejects_missing_evidence() {
        let mut ledger = ProvenanceLedger::default();
        let good = ledger.register_span("d1", "file:d1.csv", "q1", 1, 10);
        let findings = vec![
            RlmFinding {
                branch: "quality".to_string(),
                finding: "ok".to_string(),
                evidence_ids: vec![good],
                confidence: 0.9,
            },
            RlmFinding {
                branch: "quality".to_string(),
                finding: "bad".to_string(),
                evidence_ids: vec!["evidence:missing".to_string()],
                confidence: 0.9,
            },
        ];
        let (valid, dropped) = validate_findings(findings, &ledger);
        assert_eq!(valid.len(), 1);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn parse_findings_from_wrapped_text() {
        let text = "prefix {\"findings\":[{\"finding\":\"x\",\"evidence_ids\":[\"evidence:d:q:r1-1\"],\"confidence\":0.9}]} suffix";
        let parsed = parse_branch_findings(text).unwrap();
        assert_eq!(parsed.findings.len(), 1);
    }
}
