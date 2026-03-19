use crate::kernel_handle::KernelHandle;
use crate::llm_driver::{CompletionRequest, LlmDriver};
use crate::rlm_bridge::BunBridge;
use crate::rlm_dataset::{load_dataset, DatasetLoadRequest, RlmFrame};
use crate::rlm_fanout;
use crate::rlm_state::{session_memory_key, RlmMirrorState};
use crate::routing::ModelRouter;
use dashmap::DashMap;
use openfang_types::agent::{AgentManifest, ModelRoutingConfig};
use openfang_types::config::RlmConfig;
use openfang_types::message::Message;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};
use tokio::sync::Mutex;

pub mod prelude {
    pub use super::{agent_rlm_enabled, configure, maybe_prepare_auto_context, runtime};
}

#[derive(Debug)]
struct RlmSession {
    key: String,
    bridge: Mutex<BunBridge>,
    mirror: Mutex<RlmMirrorState>,
}

#[derive(Debug)]
pub struct RlmRuntime {
    config: StdRwLock<RlmConfig>,
    sessions: DashMap<String, Arc<RlmSession>>,
}

#[derive(Debug, Deserialize)]
struct JsEvalRequest {
    pub code: String,
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FanoutRequest {
    pub question: String,
    #[serde(default)]
    pub dataset_ids: Vec<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StateInspectRequest {
    #[serde(default)]
    pub session_id: Option<String>,
}

static RLM_RUNTIME: OnceLock<Arc<RlmRuntime>> = OnceLock::new();

pub fn configure(cfg: RlmConfig) {
    let rt = runtime();
    // Using std::sync::RwLock, so write() is safe to call from any context
    let mut lock = rt.config.write().unwrap();
    *lock = cfg;
}

pub fn runtime() -> Arc<RlmRuntime> {
    RLM_RUNTIME
        .get_or_init(|| Arc::new(RlmRuntime::new(RlmConfig::default())))
        .clone()
}

impl RlmRuntime {
    pub fn new(config: RlmConfig) -> Self {
        Self {
            config: StdRwLock::new(config),
            sessions: DashMap::new(),
        }
    }

    pub async fn config(&self) -> RlmConfig {
        self.config.read().unwrap().clone()
    }

    pub async fn is_enabled(&self) -> bool {
        self.config.read().unwrap().enabled
    }

    async fn ensure_session(
        &self,
        agent_id: &str,
        session_id: &str,
        kernel: Option<&Arc<dyn KernelHandle>>,
    ) -> Result<Arc<RlmSession>, String> {
        let key = session_memory_key(agent_id, session_id);
        if let Some(existing) = self.sessions.get(&key) {
            return Ok(existing.clone());
        }

        let cfg = self.config().await;
        if !cfg.enabled {
            return Err("RLM is disabled in config (rlm.enabled=false)".to_string());
        }

        let mirror = load_mirror_from_kernel(kernel, &key)?;
        let mut bridge = BunBridge::start(&cfg.bun_path).await?;
        bridge.restore(mirror.js_snapshot.clone()).await?;

        let session = Arc::new(RlmSession {
            key: key.clone(),
            bridge: Mutex::new(bridge),
            mirror: Mutex::new(mirror),
        });
        self.sessions.insert(key, session.clone());
        Ok(session)
    }

    async fn persist_mirror(
        &self,
        session: &Arc<RlmSession>,
        kernel: Option<&Arc<dyn KernelHandle>>,
    ) -> Result<(), String> {
        if let Some(kh) = kernel {
            let mirror = session.mirror.lock().await.clone();
            let payload = serde_json::to_value(&mirror)
                .map_err(|e| format!("Failed to serialize RLM mirror state: {e}"))?;
            kh.memory_store(&session.key, payload)?;
        }
        Ok(())
    }

    async fn eval_with_recovery(
        &self,
        session: &Arc<RlmSession>,
        code: &str,
        input: Option<Value>,
    ) -> Result<Value, String> {
        let snapshot_before = { session.mirror.lock().await.js_snapshot.clone() };
        let mut bridge = session.bridge.lock().await;

        let eval_result = match bridge.eval(code, input.clone()).await {
            Ok(v) => v,
            Err(primary_err) => {
                bridge
                    .restart_with_snapshot(&snapshot_before)
                    .await
                    .map_err(|e| {
                        format!(
                            "Bun bridge eval failed ({primary_err}); restart/restore failed: {e}"
                        )
                    })?;
                bridge
                    .eval(code, input)
                    .await
                    .map_err(|e| format!("Bun bridge eval retry failed after restart: {e}"))?
            }
        };

        let snapshot = bridge.snapshot().await.map_err(|e| {
            format!("Bun bridge snapshot failed after eval (state may be stale): {e}")
        })?;
        drop(bridge);

        session.mirror.lock().await.set_snapshot(snapshot);
        Ok(eval_result)
    }

    pub async fn tool_dataset_load(
        &self,
        input: &Value,
        kernel: Option<&Arc<dyn KernelHandle>>,
        caller_agent_id: Option<&str>,
        workspace_root: Option<&Path>,
    ) -> Result<String, String> {
        let req: DatasetLoadRequest = serde_json::from_value(input.clone())
            .map_err(|e| format!("Invalid rlm_dataset_load input: {e}"))?;
        let session_id = req
            .session_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_string());
        let agent_id = caller_agent_id.unwrap_or("agent");
        let session = self.ensure_session(agent_id, &session_id, kernel).await?;
        let cfg = self.config().await;

        let frame = load_dataset(&req, &cfg, workspace_root).await?;
        self.upsert_dataset_and_js(&session, &frame).await?;
        self.persist_mirror(&session, kernel).await?;

        let summary = json!({
            "dataset_id": frame.dataset_id,
            "source_id": frame.source_id,
            "query_id": frame.query_id,
            "rows": frame.profile.row_count,
            "columns": frame.profile.column_count,
            "numeric_columns": frame.profile.numeric_columns,
            "null_cells": frame.profile.null_cells,
            "session_key": session.key,
        });
        serde_json::to_string_pretty(&summary).map_err(|e| format!("Serialize error: {e}"))
    }

    async fn upsert_dataset_and_js(
        &self,
        session: &Arc<RlmSession>,
        frame: &RlmFrame,
    ) -> Result<(), String> {
        session.mirror.lock().await.upsert_dataset(frame.clone());

        // Mirror a compact version into Bun state for persistent JS variables.
        let js_input = json!({
            "dataset_id": frame.dataset_id,
            "columns": frame.columns,
            "rows": frame.rows,
            "profile": frame.profile,
        });
        let code = r#"
state.datasets = state.datasets || {};
state.datasets[input.dataset_id] = {
  columns: input.columns,
  rows: input.rows,
  profile: input.profile,
};
return { dataset_count: Object.keys(state.datasets).length, dataset_id: input.dataset_id };
"#;
        let _ = self
            .eval_with_recovery(session, code, Some(js_input))
            .await?;
        Ok(())
    }

    pub async fn tool_js_eval(
        &self,
        input: &Value,
        kernel: Option<&Arc<dyn KernelHandle>>,
        caller_agent_id: Option<&str>,
    ) -> Result<String, String> {
        let req: JsEvalRequest = serde_json::from_value(input.clone())
            .map_err(|e| format!("Invalid rlm_js_eval input: {e}"))?;
        let session_id = req
            .session_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_string());
        let agent_id = caller_agent_id.unwrap_or("agent");
        let session = self.ensure_session(agent_id, &session_id, kernel).await?;

        let result = self
            .eval_with_recovery(&session, &req.code, req.input.clone())
            .await?;
        self.persist_mirror(&session, kernel).await?;
        serde_json::to_string_pretty(&json!({"result": result, "session_key": session.key}))
            .map_err(|e| format!("Serialize error: {e}"))
    }

    pub async fn tool_fanout(
        &self,
        input: &Value,
        kernel: Option<&Arc<dyn KernelHandle>>,
        caller_agent_id: Option<&str>,
        driver: Option<&Arc<dyn LlmDriver>>,
        model_name: Option<&str>,
        routing: Option<&ModelRoutingConfig>,
    ) -> Result<String, String> {
        let req: FanoutRequest = serde_json::from_value(input.clone())
            .map_err(|e| format!("Invalid rlm_fanout input: {e}"))?;
        let session_id = req
            .session_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_string());
        let agent_id = caller_agent_id.unwrap_or("agent");
        let session = self.ensure_session(agent_id, &session_id, kernel).await?;
        let cfg = self.config().await;

        let (frames, provenance) = {
            let mirror = session.mirror.lock().await;
            let frames = if req.dataset_ids.is_empty() {
                mirror.datasets.values().cloned().collect::<Vec<_>>()
            } else {
                req.dataset_ids
                    .iter()
                    .filter_map(|id| mirror.datasets.get(id).cloned())
                    .collect::<Vec<_>>()
            };
            (frames, mirror.provenance.clone())
        };

        if frames.is_empty() {
            return Err(
                "No datasets available in RLM session. Load data first with rlm_dataset_load"
                    .to_string(),
            );
        }

        let driver = driver
            .cloned()
            .ok_or("rlm_fanout requires an active LLM driver in this execution context")?;
        let base_model = model_name
            .map(str::to_string)
            .ok_or("rlm_fanout requires the active model name in this execution context")?;
        let selected_model = select_branch_model(&req.question, &base_model, routing);

        let response = rlm_fanout::run_fanout_llm(
            &req.question,
            &frames,
            &provenance,
            &cfg,
            driver,
            &selected_model,
        )
        .await;
        session.mirror.lock().await.set_fanout(response.clone());
        self.persist_mirror(&session, kernel).await?;

        serde_json::to_string_pretty(&response).map_err(|e| format!("Serialize error: {e}"))
    }

    pub async fn tool_state_inspect(
        &self,
        input: &Value,
        kernel: Option<&Arc<dyn KernelHandle>>,
        caller_agent_id: Option<&str>,
    ) -> Result<String, String> {
        let req: StateInspectRequest = serde_json::from_value(input.clone())
            .map_err(|e| format!("Invalid rlm_state_inspect input: {e}"))?;
        let session_id = req
            .session_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_string());
        let agent_id = caller_agent_id.unwrap_or("agent");

        let session_key = session_memory_key(agent_id, &session_id);
        let session = match self.sessions.get(&session_key) {
            Some(s) => s.clone(),
            None => {
                let mirror = load_mirror_from_kernel(kernel, &session_key)?;
                let summary = json!({
                    "session_key": session_key,
                    "loaded_in_process": false,
                    "datasets": mirror
                        .datasets
                        .iter()
                        .map(|(id, f)| json!({"dataset_id": id, "rows": f.profile.row_count, "columns": f.profile.column_count}))
                        .collect::<Vec<_>>(),
                    "evidence_count": mirror.provenance.entries.len(),
                    "last_fanout": mirror.last_fanout,
                    "updated_at": mirror.updated_at,
                });
                return serde_json::to_string_pretty(&summary)
                    .map_err(|e| format!("Serialize error: {e}"));
            }
        };

        let health = {
            let mut bridge = session.bridge.lock().await;
            bridge.health().await.is_ok()
        };

        let mirror = session.mirror.lock().await.clone();
        let summary = json!({
            "session_key": session.key,
            "loaded_in_process": true,
            "bun_health": health,
            "datasets": mirror
                .datasets
                .iter()
                .map(|(id, f)| json!({"dataset_id": id, "rows": f.profile.row_count, "columns": f.profile.column_count, "source_id": f.source_id}))
                .collect::<Vec<_>>(),
            "evidence_count": mirror.provenance.entries.len(),
            "last_fanout": mirror.last_fanout,
            "updated_at": mirror.updated_at,
        });
        serde_json::to_string_pretty(&summary).map_err(|e| format!("Serialize error: {e}"))
    }
}

fn load_mirror_from_kernel(
    kernel: Option<&Arc<dyn KernelHandle>>,
    key: &str,
) -> Result<RlmMirrorState, String> {
    let Some(kh) = kernel else {
        return Ok(RlmMirrorState::default());
    };

    let recalled = kh.memory_recall(key)?;
    if let Some(v) = recalled {
        serde_json::from_value(v).map_err(|e| format!("Invalid stored RLM mirror state: {e}"))
    } else {
        Ok(RlmMirrorState::default())
    }
}

pub fn agent_rlm_enabled(manifest: &AgentManifest) -> bool {
    manifest
        .metadata
        .get("rlm_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn looks_analytic_request(message: &str) -> bool {
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
    hints.iter().any(|h| lower.contains(h))
}

pub async fn maybe_prepare_auto_context(
    manifest: &AgentManifest,
    user_message: &str,
    session_id: &str,
    caller_agent_id: &str,
    kernel: Option<&Arc<dyn KernelHandle>>,
    workspace_root: Option<&Path>,
    driver: &Arc<dyn LlmDriver>,
) -> Result<Option<String>, String> {
    if !agent_rlm_enabled(manifest) {
        return Ok(None);
    }

    let rt = runtime();
    if !rt.is_enabled().await {
        return Ok(Some(
            "RLM auto-mode requested by agent metadata but disabled in config (`rlm.enabled=false`)."
                .to_string(),
        ));
    }

    if !looks_analytic_request(user_message) {
        return Ok(None);
    }

    let session = rt
        .ensure_session(caller_agent_id, session_id, kernel)
        .await
        .map_err(|e| format!("RLM session init failed: {e}"))?;

    // Auto-load datasets referenced in the user message.
    let paths = extract_dataset_paths(user_message);
    let mut load_errors = Vec::new();
    let mut successful_loads = 0usize;
    if !paths.is_empty() {
        let cfg = rt.config().await;
        for path in paths.iter().take(3) {
            let req = DatasetLoadRequest {
                dataset_id: Some(slug_dataset_id(path)),
                kind: if path.ends_with(".db") || path.ends_with(".sqlite") {
                    "sqlite".to_string()
                } else {
                    "file".to_string()
                },
                session_id: None,
                path: Some(path.clone()),
                format: None,
                query: if path.ends_with(".db") || path.ends_with(".sqlite") {
                    Some("SELECT * FROM sqlite_master LIMIT 100".to_string())
                } else {
                    None
                },
                connection: None,
                sanitize: None,
            };
            match load_dataset(&req, &cfg, workspace_root).await {
                Ok(frame) => match rt.upsert_dataset_and_js(&session, &frame).await {
                    Ok(_) => successful_loads += 1,
                    Err(e) => load_errors.push(format!("{path}: JS mirror failed: {e}")),
                },
                Err(e) => load_errors.push(format!("{path}: {e}")),
            }
        }
        let _ = rt.persist_mirror(&session, kernel).await;
    }

    // If the user explicitly referenced datasets but none loaded this turn,
    // do not fall back to stale prior-session frames.
    if !paths.is_empty() && successful_loads == 0 {
        let mut context = String::new();
        context.push_str("\n\n[RLM AUTO-MODE DATASET LOAD ERROR]\n");
        context.push_str("Detected dataset paths but failed to load them:\n");
        for err in &load_errors {
            context.push_str(&format!("- {err}\n"));
        }
        context.push_str(
            "Use workspace-local paths (or quoted absolute paths within allowed scope), then retry.\n",
        );
        return Ok(Some(context));
    }

    let (frames, provenance) = {
        let mirror = session.mirror.lock().await;
        (
            mirror.datasets.values().cloned().collect::<Vec<_>>(),
            mirror.provenance.clone(),
        )
    };

    if frames.is_empty() {
        return Ok(None);
    }

    let cfg = rt.config().await;
    let selected_model = select_branch_model(
        user_message,
        &manifest.model.model,
        manifest.routing.as_ref(),
    );
    let fanout = rlm_fanout::run_fanout_llm(
        user_message,
        &frames,
        &provenance,
        &cfg,
        driver.clone(),
        &selected_model,
    )
    .await;
    session.mirror.lock().await.set_fanout(fanout.clone());
    let _ = rt.persist_mirror(&session, kernel).await;

    let mut context = String::new();
    context.push_str("\n\n[RLM AUTO-MODE EVIDENCE]\n");
    context.push_str("Use only claims that have evidence citations.\n");
    if !load_errors.is_empty() {
        context.push_str("Dataset load warnings:\n");
        for err in &load_errors {
            context.push_str(&format!("- {err}\n"));
        }
    }
    for finding in &fanout.findings {
        if let Some(evidence_id) = finding.evidence_ids.first() {
            context.push_str(&format!("- {} [{}]\n", finding.finding, evidence_id));
        }
    }

    Ok(Some(context))
}

pub async fn enforce_response_citations(
    response: &str,
    session_id: &str,
    caller_agent_id: &str,
    kernel: Option<&Arc<dyn KernelHandle>>,
) -> Result<String, String> {
    let session_key = session_memory_key(caller_agent_id, session_id);
    let mirror = if let Some(session) = runtime().sessions.get(&session_key) {
        session.mirror.lock().await.clone()
    } else {
        load_mirror_from_kernel(kernel, &session_key)?
    };

    // Non-analytic turns may never produce fanout; keep response unchanged.
    let Some(fanout) = mirror.last_fanout else {
        return Ok(response.to_string());
    };

    if fanout.findings.is_empty() {
        return Ok(
            "[Evidence enforcement] No validated evidence was available for this response."
                .to_string(),
        );
    }

    let valid_ids: HashSet<String> = fanout
        .findings
        .iter()
        .flat_map(|f| f.evidence_ids.iter().cloned())
        .collect();
    let cited_ids = extract_cited_evidence_ids(response);
    let has_uncited_content = contains_uncited_content(response);
    if !cited_ids.is_empty()
        && cited_ids.iter().all(|id| valid_ids.contains(id))
        && !has_uncited_content
    {
        return Ok(response.to_string());
    }

    let mut out = String::new();
    if cited_ids.is_empty() {
        out.push_str(
            "[Evidence enforcement] Replaced model output because it lacked evidence citations.\n\n",
        );
    } else if cited_ids.iter().any(|id| !valid_ids.contains(id)) {
        out.push_str(
            "[Evidence enforcement] Replaced model output because it cited invalid evidence IDs.\n\n",
        );
    } else {
        out.push_str(
            "[Evidence enforcement] Replaced model output because it included uncited claims.\n\n",
        );
    }
    out.push_str("Validated Evidence:\n");
    for finding in fanout.findings.iter().take(8) {
        if let Some(eid) = finding.evidence_ids.first() {
            out.push_str(&format!("- {} [{}]\n", finding.finding, eid));
        }
    }
    Ok(out)
}

fn select_branch_model(
    user_message: &str,
    fallback_model: &str,
    routing: Option<&ModelRoutingConfig>,
) -> String {
    let Some(routing_cfg) = routing.cloned() else {
        return fallback_model.to_string();
    };

    let router = ModelRouter::new(routing_cfg);
    let probe = CompletionRequest {
        model: fallback_model.to_string(),
        messages: vec![Message::user(user_message)],
        tools: vec![],
        max_tokens: 512,
        temperature: 0.2,
        system: None,
        thinking: None,
        sentry_parent_span: None,
    };
    let (_, model) = router.select_model(&probe);
    model
}

fn extract_dataset_paths(message: &str) -> Vec<String> {
    let quoted_re = regex_lite::Regex::new(
        r#"(?:"([^"\n]+\.(?:csv|tsv|jsonl|json|db|sqlite))"|'([^'\n]+\.(?:csv|tsv|jsonl|json|db|sqlite))')"#,
    )
    .expect("quoted dataset path regex must compile");
    let path_re = regex_lite::Regex::new(r"[A-Za-z0-9_./\\-]+\.(csv|tsv|jsonl|json|db|sqlite)")
        .expect("dataset path regex must compile");
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut quoted_spans: Vec<(usize, usize)> = Vec::new();

    for caps in quoted_re.captures_iter(message) {
        if let Some(full) = caps.get(0) {
            quoted_spans.push((full.start(), full.end()));
        }
        let raw = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str())
            .unwrap_or_default();
        let normalized = raw.replace('\\', "/");
        if seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }

    for m in path_re.find_iter(message) {
        if quoted_spans
            .iter()
            .any(|(start, end)| m.start() < *end && *start < m.end())
        {
            continue;
        }
        let normalized = m.as_str().replace('\\', "/");
        if seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }

    out
}

fn extract_cited_evidence_ids(response: &str) -> Vec<String> {
    let citation_re = regex_lite::Regex::new(r"\[(evidence:[A-Za-z0-9:_\-]+)\]")
        .expect("evidence citation regex must compile");
    citation_re
        .captures_iter(response)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn contains_uncited_content(response: &str) -> bool {
    response.lines().any(|line| {
        let t = line.trim();
        if t.is_empty()
            || t == "Validated Evidence:"
            || t.starts_with("[Evidence enforcement]")
            || t.contains("[evidence:")
        {
            return false;
        }
        t.chars().any(|c| c.is_ascii_alphanumeric())
    })
}

fn slug_dataset_id(path: &str) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("dataset")
        .to_ascii_lowercase();
    stem.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_analytic_request() {
        assert!(looks_analytic_request("analyze this csv dataset"));
        assert!(!looks_analytic_request("write a haiku about rust"));
    }

    #[test]
    fn extract_paths() {
        let paths = extract_dataset_paths("Use data/sales.csv and ./tmp/events.jsonl");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn extract_paths_with_spaces_when_quoted() {
        let paths = extract_dataset_paths(
            "Analyze \"/Users/gaganarora/Desktop/my projects/open_fang/data/sales.csv\" next.",
        );
        assert_eq!(paths.len(), 1);
        assert!(paths[0].contains("my projects"));
    }

    #[test]
    fn extract_cited_ids() {
        let ids = extract_cited_evidence_ids(
            "Revenue grew [evidence:sales:q1:r1-5]. Another [evidence:metrics:q2:r10-20].",
        );
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "evidence:sales:q1:r1-5");
    }

    #[test]
    fn uncited_content_detection() {
        assert!(!contains_uncited_content(
            "Validated Evidence:\n- Revenue grew [evidence:sales:q1:r1-5]"
        ));
        assert!(contains_uncited_content(
            "Revenue grew strongly.\n- Revenue grew [evidence:sales:q1:r1-5]"
        ));
    }

    #[test]
    fn routing_model_selection_uses_router_when_present() {
        let routing = ModelRoutingConfig {
            simple_model: "simple-model".to_string(),
            medium_model: "medium-model".to_string(),
            complex_model: "complex-model".to_string(),
            simple_threshold: 10,
            complex_threshold: 100,
        };
        let selected = select_branch_model("very small question", "fallback-model", Some(&routing));
        assert_eq!(selected, "simple-model");
    }
}
