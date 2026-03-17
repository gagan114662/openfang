//! MiroFish simulation engine HTTP client.
//!
//! Wraps the current upstream MiroFish Flask API and emits one structured
//! Sentry log per high-level operation, with child spans for each upstream hop.

use reqwest::Method;
use sentry::Level;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const POLL_INTERVAL_MS: u64 = 1500;

// ── Response types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirofishStatus {
    pub reachable: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphBuildResult {
    pub project_id: String,
    pub graph_id: String,
    #[serde(default)]
    pub entity_count: usize,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationInfo {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub agent_count: Option<usize>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub graph_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimAgent {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, rename = "type")]
    pub agent_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterviewResult {
    pub agent_name: String,
    pub response: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportResult {
    pub content: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub report_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

// ── Client ───────────────────────────────────────────────────────────────────

pub struct MirofishClient {
    http: reqwest::Client,
    base_url: String,
    timeout_secs: u64,
}

impl MirofishClient {
    pub fn new(base_url: &str, timeout_secs: u64, api_key: Option<&str>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(key) = api_key {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
        }

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .default_headers(headers)
            .build()
            .unwrap_or_default();

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            timeout_secs,
        }
    }

    /// Create a client from KernelConfig's MirofishConfig.
    pub fn from_config(cfg: &openfang_types::config::MirofishConfig) -> Self {
        let api_key = std::env::var(&cfg.api_key_env).ok();
        Self::new(&cfg.base_url, cfg.timeout_secs, api_key.as_deref())
    }

    // ── Health ───────────────────────────────────────────────────────────

    pub async fn health(&self) -> MirofishStatus {
        let started = Instant::now();
        let result = self
            .request_json(Method::GET, "/health", None, "mirofish.health.http")
            .await;

        let status = match &result {
            Ok(payload) => MirofishStatus {
                reachable: true,
                version: payload
                    .get("service")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .or_else(|| {
                        payload
                            .get("status")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    }),
                base_url: Some(self.base_url.clone()),
            },
            Err(_) => MirofishStatus {
                reachable: false,
                version: None,
                base_url: Some(self.base_url.clone()),
            },
        };

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.base_url".to_string(), json!(self.base_url));
        attrs.insert("mirofish.reachable".to_string(), json!(status.reachable));
        if let Some(version) = &status.version {
            attrs.insert("mirofish.version".to_string(), json!(version));
        }
        self.capture_operation_log("mirofish.health", started, attrs, result.err());

        status
    }

    // ── Knowledge graph ──────────────────────────────────────────────────

    pub async fn build_graph(
        &self,
        project_name: &str,
        simulation_requirement: Option<&str>,
        documents: &[String],
    ) -> Result<GraphBuildResult, String> {
        let started = Instant::now();
        let requirement = simulation_requirement
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(project_name);

        let result = async {
            if documents.is_empty() {
                return Err("MiroFish build_graph requires at least one document".to_string());
            }

            let ontology_payload = self
                .request_multipart(
                    "/api/graph/ontology/generate",
                    build_seed_form(project_name, requirement, documents)?,
                    "mirofish.graph.ontology_generate.http",
                )
                .await?;
            let ontology_data = expect_success_payload(&ontology_payload)?;
            let project_id = required_string(ontology_data, "project_id", "project_id")?;

            let build_payload = self
                .request_json(
                    Method::POST,
                    "/api/graph/build",
                    Some(json!({
                        "project_id": project_id,
                        "graph_name": project_name,
                    })),
                    "mirofish.graph.build_start.http",
                )
                .await?;
            let build_data = expect_success_payload(&build_payload)?;
            let task_id = required_string(build_data, "task_id", "task_id")?;

            let task_payload = self.poll_graph_task(&task_id).await?;
            let graph_id = required_string(&task_payload, "graph_id", "task.result.graph_id")?;
            let entity_count = task_payload
                .get("node_count")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;

            Ok(GraphBuildResult {
                project_id: project_id.to_string(),
                graph_id: graph_id.to_string(),
                entity_count,
                task_id: Some(task_id.to_string()),
            })
        }
        .await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.project_name".to_string(), json!(project_name));
        attrs.insert(
            "mirofish.simulation_requirement".to_string(),
            json!(requirement),
        );
        attrs.insert(
            "mirofish.documents.count".to_string(),
            json!(documents.len() as u64),
        );
        attrs.insert(
            "mirofish.documents.chars".to_string(),
            json!(documents.iter().map(|doc| doc.len() as u64).sum::<u64>()),
        );
        if let Ok(graph) = &result {
            attrs.insert("mirofish.project_id".to_string(), json!(graph.project_id));
            attrs.insert("mirofish.graph_id".to_string(), json!(graph.graph_id));
            attrs.insert(
                "mirofish.graph.entity_count".to_string(),
                json!(graph.entity_count as u64),
            );
            if let Some(task_id) = &graph.task_id {
                attrs.insert("mirofish.task_id".to_string(), json!(task_id));
            }
        }
        self.capture_operation_log(
            "mirofish.graph.build",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    pub async fn get_entities(&self, id: &str) -> Result<Vec<SimAgent>, String> {
        let started = Instant::now();
        let result = async {
            if id.starts_with("sim_") {
                let payload = self
                    .request_json(
                        Method::GET,
                        &format!("/api/simulation/{id}/profiles?platform=reddit"),
                        None,
                        "mirofish.simulation.profiles.http",
                    )
                    .await?;
                let data = expect_success_payload(&payload)?;
                Ok(sim_agents_from_profiles(
                    data.get("profiles")
                        .and_then(Value::as_array)
                        .ok_or("MiroFish profiles payload missing profiles array")?,
                ))
            } else {
                let payload = self
                    .request_json(
                        Method::GET,
                        &format!("/api/simulation/entities/{id}"),
                        None,
                        "mirofish.graph.entities.http",
                    )
                    .await?;
                let data = expect_success_payload(&payload)?;
                Ok(sim_agents_from_graph_entities(
                    data.get("entities")
                        .and_then(Value::as_array)
                        .ok_or("MiroFish graph entities payload missing entities array")?,
                ))
            }
        }
        .await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.lookup_id".to_string(), json!(id));
        attrs.insert(
            "mirofish.lookup_kind".to_string(),
            json!(if id.starts_with("sim_") {
                "simulation"
            } else {
                "graph"
            }),
        );
        if let Ok(agents) = &result {
            attrs.insert(
                "mirofish.agent_count".to_string(),
                json!(agents.len() as u64),
            );
        }
        self.capture_operation_log(
            "mirofish.entities.list",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    // ── Simulation lifecycle ─────────────────────────────────────────────

    pub async fn create_simulation(
        &self,
        graph_id: &str,
        project_id: Option<&str>,
        topic: &str,
        rounds: Option<u32>,
    ) -> Result<SimulationInfo, String> {
        let started = Instant::now();
        let result = async {
            let resolved_project_id =
                match project_id.map(str::trim).filter(|value| !value.is_empty()) {
                    Some(project_id) => project_id.to_string(),
                    None => self.resolve_project_id_for_graph(graph_id).await?,
                };

            let created_payload = self
                .request_json(
                    Method::POST,
                    "/api/simulation/create",
                    Some(json!({
                        "project_id": resolved_project_id,
                        "graph_id": graph_id,
                    })),
                    "mirofish.simulation.create.http",
                )
                .await?;
            let created_data = expect_success_payload(&created_payload)?;
            let simulation_id = required_string(created_data, "simulation_id", "simulation_id")?;

            let _ = self.prepare_simulation_internal(simulation_id).await?;
            let _ = self.run_simulation_internal(simulation_id, rounds).await?;
            self.get_status_internal(simulation_id).await
        }
        .await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.graph_id".to_string(), json!(graph_id));
        if let Some(project_id) = project_id.filter(|value| !value.trim().is_empty()) {
            attrs.insert(
                "mirofish.project_id.requested".to_string(),
                json!(project_id),
            );
        }
        if !topic.trim().is_empty() {
            attrs.insert("mirofish.requested_topic".to_string(), json!(topic));
        }
        if let Some(rounds) = rounds {
            attrs.insert("mirofish.rounds".to_string(), json!(rounds));
        }
        if let Ok(info) = &result {
            attrs.insert("mirofish.simulation_id".to_string(), json!(info.id));
            attrs.insert("mirofish.status".to_string(), json!(info.status));
            if let Some(agent_count) = info.agent_count {
                attrs.insert("mirofish.agent_count".to_string(), json!(agent_count));
            }
            if let Some(project_id) = &info.project_id {
                attrs.insert("mirofish.project_id".to_string(), json!(project_id));
            }
        }
        self.capture_operation_log(
            "mirofish.simulation.create",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    pub async fn prepare_simulation(&self, sim_id: &str) -> Result<SimulationInfo, String> {
        let started = Instant::now();
        let result = self.prepare_simulation_internal(sim_id).await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.simulation_id".to_string(), json!(sim_id));
        if let Ok(info) = &result {
            attrs.insert("mirofish.status".to_string(), json!(info.status));
            if let Some(agent_count) = info.agent_count {
                attrs.insert("mirofish.agent_count".to_string(), json!(agent_count));
            }
        }
        self.capture_operation_log(
            "mirofish.simulation.prepare",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    pub async fn run_simulation(
        &self,
        sim_id: &str,
        rounds: Option<u32>,
    ) -> Result<SimulationInfo, String> {
        let started = Instant::now();
        let result = async {
            let _ = self.run_simulation_internal(sim_id, rounds).await?;
            self.get_status_internal(sim_id).await
        }
        .await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.simulation_id".to_string(), json!(sim_id));
        if let Some(rounds) = rounds {
            attrs.insert("mirofish.rounds".to_string(), json!(rounds));
        }
        if let Ok(info) = &result {
            attrs.insert("mirofish.status".to_string(), json!(info.status));
        }
        self.capture_operation_log(
            "mirofish.simulation.run",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    pub async fn get_status(&self, sim_id: &str) -> Result<SimulationInfo, String> {
        let started = Instant::now();
        let result = self.get_status_internal(sim_id).await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.simulation_id".to_string(), json!(sim_id));
        if let Ok(info) = &result {
            attrs.insert("mirofish.status".to_string(), json!(info.status));
            if let Some(agent_count) = info.agent_count {
                attrs.insert("mirofish.agent_count".to_string(), json!(agent_count));
            }
        }
        self.capture_operation_log(
            "mirofish.simulation.status",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    // ── Interview & reporting ────────────────────────────────────────────

    pub async fn interview(
        &self,
        sim_id: &str,
        agent_name: &str,
        question: &str,
    ) -> Result<InterviewResult, String> {
        let started = Instant::now();
        let result = async {
            let agents = self.get_entities(sim_id).await?;
            let agent = agents
                .iter()
                .find(|agent| {
                    agent.name.eq_ignore_ascii_case(agent_name)
                        || agent
                            .id
                            .as_deref()
                            .is_some_and(|id| id.eq_ignore_ascii_case(agent_name))
                })
                .ok_or_else(|| {
                    format!(
                        "No MiroFish agent named '{agent_name}' in simulation {sim_id}. Call mirofish_list_agents first."
                    )
                })?;
            let agent_id = agent
                .id
                .as_deref()
                .ok_or_else(|| format!("Resolved MiroFish agent '{agent_name}' has no agent id"))?;

            let payload = self
                .request_json(
                    Method::POST,
                    "/api/simulation/interview",
                    Some(json!({
                        "simulation_id": sim_id,
                        "agent_id": parse_agent_id(agent_id)?,
                        "prompt": question,
                    })),
                    "mirofish.simulation.interview.http",
                )
                .await?;
            let data = expect_success_payload(&payload)?;
            let response = extract_interview_response(data)?;

            Ok(InterviewResult {
                agent_name: agent.name.clone(),
                response,
            })
        }
        .await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.simulation_id".to_string(), json!(sim_id));
        attrs.insert("mirofish.agent_name".to_string(), json!(agent_name));
        attrs.insert(
            "mirofish.question_length".to_string(),
            json!(question.chars().count() as u64),
        );
        if let Ok(reply) = &result {
            attrs.insert(
                "mirofish.response_length".to_string(),
                json!(reply.response.chars().count() as u64),
            );
        }
        self.capture_operation_log(
            "mirofish.simulation.interview",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    pub async fn generate_report(
        &self,
        sim_id: &str,
        topic: Option<&str>,
    ) -> Result<ReportResult, String> {
        let started = Instant::now();
        let result = async {
            let generate_payload = self
                .request_json(
                    Method::POST,
                    "/api/report/generate",
                    Some(json!({
                        "simulation_id": sim_id,
                    })),
                    "mirofish.report.generate.http",
                )
                .await?;
            let generate_data = expect_success_payload(&generate_payload)?;

            let report_id = generate_data
                .get("report_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let already_generated = generate_data
                .get("already_generated")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            if !already_generated {
                let task_id = required_string(generate_data, "task_id", "task_id")?;
                let _ = self.poll_report_task(sim_id, task_id).await?;
            }

            let report_payload = self
                .request_json(
                    Method::GET,
                    &format!("/api/report/by-simulation/{sim_id}"),
                    None,
                    "mirofish.report.fetch.http",
                )
                .await?;
            let report_data = expect_success_payload(&report_payload)?;
            let content = required_string(report_data, "markdown_content", "markdown_content")?;

            Ok(ReportResult {
                content: content.to_string(),
                topic: topic.map(ToString::to_string).or_else(|| {
                    report_data
                        .get("simulation_requirement")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                }),
                report_id: report_id.or_else(|| {
                    report_data
                        .get("report_id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                }),
                status: report_data
                    .get("status")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            })
        }
        .await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.simulation_id".to_string(), json!(sim_id));
        if let Some(topic) = topic.filter(|value| !value.trim().is_empty()) {
            attrs.insert("mirofish.report_topic".to_string(), json!(topic));
        }
        if let Ok(report) = &result {
            attrs.insert(
                "mirofish.report_length".to_string(),
                json!(report.content.chars().count() as u64),
            );
            if let Some(report_id) = &report.report_id {
                attrs.insert("mirofish.report_id".to_string(), json!(report_id));
            }
            if let Some(status) = &report.status {
                attrs.insert("mirofish.report_status".to_string(), json!(status));
            }
        }
        self.capture_operation_log(
            "mirofish.report.generate",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    pub async fn chat_report(&self, sim_id: &str, message: &str) -> Result<ReportResult, String> {
        let started = Instant::now();
        let result = async {
            let payload = self
                .request_json(
                    Method::POST,
                    "/api/report/chat",
                    Some(json!({
                        "simulation_id": sim_id,
                        "message": message,
                    })),
                    "mirofish.report.chat.http",
                )
                .await?;
            let data = expect_success_payload(&payload)?;
            let response = required_string(data, "response", "response")?;

            Ok(ReportResult {
                content: response.to_string(),
                topic: None,
                report_id: None,
                status: Some("chat".to_string()),
            })
        }
        .await;

        let mut attrs = BTreeMap::new();
        attrs.insert("mirofish.simulation_id".to_string(), json!(sim_id));
        attrs.insert(
            "mirofish.message_length".to_string(),
            json!(message.chars().count() as u64),
        );
        if let Ok(report) = &result {
            attrs.insert(
                "mirofish.response_length".to_string(),
                json!(report.content.chars().count() as u64),
            );
        }
        self.capture_operation_log(
            "mirofish.report.chat",
            started,
            attrs,
            result.as_ref().err().cloned(),
        );
        result
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    async fn prepare_simulation_internal(&self, sim_id: &str) -> Result<SimulationInfo, String> {
        let payload = self
            .request_json(
                Method::POST,
                "/api/simulation/prepare",
                Some(json!({
                    "simulation_id": sim_id,
                })),
                "mirofish.simulation.prepare_start.http",
            )
            .await?;
        let data = expect_success_payload(&payload)?;
        if data
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status == "ready")
        {
            return self.get_status_internal(sim_id).await;
        }

        let task_id = required_string(data, "task_id", "task_id")?;
        let _ = self.poll_prepare_task(sim_id, task_id).await?;
        self.get_status_internal(sim_id).await
    }

    async fn run_simulation_internal(
        &self,
        sim_id: &str,
        rounds: Option<u32>,
    ) -> Result<Value, String> {
        let mut body = json!({
            "simulation_id": sim_id,
            "platform": "parallel",
        });
        if let Some(rounds) = rounds {
            body["max_rounds"] = json!(rounds);
        }

        let payload = self
            .request_json(
                Method::POST,
                "/api/simulation/start",
                Some(body),
                "mirofish.simulation.start.http",
            )
            .await?;
        Ok(expect_success_payload(&payload)?.clone())
    }

    async fn get_status_internal(&self, sim_id: &str) -> Result<SimulationInfo, String> {
        let payload = self
            .request_json(
                Method::GET,
                &format!("/api/simulation/{sim_id}"),
                None,
                "mirofish.simulation.status.http",
            )
            .await?;
        let data = expect_success_payload(&payload)?;
        Ok(simulation_info_from_value(data))
    }

    async fn poll_graph_task(&self, task_id: &str) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(self.timeout_secs);
        loop {
            let payload = self
                .request_json(
                    Method::GET,
                    &format!("/api/graph/task/{task_id}"),
                    None,
                    "mirofish.graph.task_status.http",
                )
                .await?;
            let data = expect_success_payload(&payload)?;
            match data
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "completed" => {
                    return Ok(data.get("result").cloned().unwrap_or_else(|| json!({})));
                }
                "failed" => {
                    return Err(task_error_message(data, task_id));
                }
                _ => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "Timed out waiting for MiroFish graph task {task_id}"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                }
            }
        }
    }

    async fn poll_prepare_task(&self, sim_id: &str, task_id: &str) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(self.timeout_secs);
        loop {
            let payload = self
                .request_json(
                    Method::POST,
                    "/api/simulation/prepare/status",
                    Some(json!({
                        "simulation_id": sim_id,
                        "task_id": task_id,
                    })),
                    "mirofish.simulation.prepare_status.http",
                )
                .await?;
            let data = expect_success_payload(&payload)?;
            match data
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "ready" | "completed" => return Ok(data.clone()),
                "failed" => return Err(task_error_message(data, task_id)),
                _ => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "Timed out waiting for MiroFish prepare task {task_id}"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                }
            }
        }
    }

    async fn poll_report_task(&self, sim_id: &str, task_id: &str) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(self.timeout_secs);
        loop {
            let payload = self
                .request_json(
                    Method::POST,
                    "/api/report/generate/status",
                    Some(json!({
                        "simulation_id": sim_id,
                        "task_id": task_id,
                    })),
                    "mirofish.report.status.http",
                )
                .await?;
            let data = expect_success_payload(&payload)?;
            match data
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "completed" => return Ok(data.clone()),
                "failed" => return Err(task_error_message(data, task_id)),
                _ => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "Timed out waiting for MiroFish report task {task_id}"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                }
            }
        }
    }

    async fn resolve_project_id_for_graph(&self, graph_id: &str) -> Result<String, String> {
        let payload = self
            .request_json(
                Method::GET,
                "/api/graph/project/list?limit=200",
                None,
                "mirofish.project.list.http",
            )
            .await?;
        let data = expect_success_payload(&payload)?;
        let projects = data
            .as_array()
            .ok_or("MiroFish project list payload missing array data")?;
        projects
            .iter()
            .find(|project| project.get("graph_id").and_then(Value::as_str) == Some(graph_id))
            .and_then(|project| project.get("project_id").and_then(Value::as_str))
            .map(ToString::to_string)
            .ok_or_else(|| format!("No MiroFish project found for graph_id '{graph_id}'"))
    }

    async fn request_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        span_op: &str,
    ) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let span = start_http_span(span_op, method.as_str(), path);
        let mut request = self.http.request(method, &url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|err| {
            finish_http_span(span.as_ref(), None, Some(&err.to_string()));
            format!("MiroFish request failed for {path}: {err}")
        })?;
        decode_json_response(response, span.as_ref(), path).await
    }

    async fn request_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
        span_op: &str,
    ) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let span = start_http_span(span_op, "POST", path);
        let response = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|err| {
                finish_http_span(span.as_ref(), None, Some(&err.to_string()));
                format!("MiroFish multipart request failed for {path}: {err}")
            })?;
        decode_json_response(response, span.as_ref(), path).await
    }

    fn capture_operation_log(
        &self,
        operation: &str,
        started: Instant,
        mut attrs: BTreeMap<String, Value>,
        error: Option<String>,
    ) {
        attrs.insert(
            "event.kind".to_string(),
            json!(if error.is_some() {
                format!("{operation}.failed")
            } else {
                format!("{operation}.completed")
            }),
        );
        attrs.insert("event.category".to_string(), json!("mirofish"));
        attrs.insert(
            "latency_ms".to_string(),
            json!(started.elapsed().as_millis() as u64),
        );
        attrs.insert("mirofish.base_url".to_string(), json!(self.base_url));
        attrs.insert(
            "outcome".to_string(),
            json!(if error.is_some() { "error" } else { "success" }),
        );
        if let Some(error) = error {
            attrs.insert("error.message".to_string(), json!(error));
        }
        capture_operation_event(
            if attrs.contains_key("error.message") {
                Level::Warning
            } else {
                Level::Info
            },
            operation,
            attrs,
        );
    }
}

fn build_seed_form(
    project_name: &str,
    simulation_requirement: &str,
    documents: &[String],
) -> Result<reqwest::multipart::Form, String> {
    let mut form = reqwest::multipart::Form::new()
        .text("project_name", project_name.to_string())
        .text("simulation_requirement", simulation_requirement.to_string());

    for (idx, document) in documents.iter().enumerate() {
        let part = reqwest::multipart::Part::text(document.clone())
            .file_name(format!("seed-{}.txt", idx + 1))
            .mime_str("text/plain; charset=utf-8")
            .map_err(|err| format!("Invalid multipart mime type: {err}"))?;
        form = form.part("files", part);
    }

    Ok(form)
}

fn start_http_span(operation: &str, method: &str, path: &str) -> Option<sentry::TransactionOrSpan> {
    sentry::configure_scope(|scope| {
        scope.get_span().map(|parent| {
            let span = parent.start_child("mirofish.http", operation);
            span.set_data("http.method", method.to_string().into());
            span.set_data("http.route", path.to_string().into());
            span.into()
        })
    })
}

fn finish_http_span(
    span: Option<&sentry::TransactionOrSpan>,
    status_code: Option<reqwest::StatusCode>,
    error: Option<&str>,
) {
    let Some(span) = span else {
        return;
    };

    if let Some(status_code) = status_code {
        span.set_data("http.status_code", status_code.as_u16().into());
        span.set_status(match status_code.as_u16() {
            200..=299 => sentry::protocol::SpanStatus::Ok,
            400 => sentry::protocol::SpanStatus::InvalidArgument,
            401 => sentry::protocol::SpanStatus::Unauthenticated,
            403 => sentry::protocol::SpanStatus::PermissionDenied,
            404 => sentry::protocol::SpanStatus::NotFound,
            429 => sentry::protocol::SpanStatus::ResourceExhausted,
            500..=599 => sentry::protocol::SpanStatus::InternalError,
            _ => sentry::protocol::SpanStatus::UnknownError,
        });
    } else if error.is_some() {
        span.set_status(sentry::protocol::SpanStatus::InternalError);
    }

    if let Some(error) = error {
        span.set_data("error.message", error.to_string().into());
    }
    span.clone().finish();
}

async fn decode_json_response(
    response: reqwest::Response,
    span: Option<&sentry::TransactionOrSpan>,
    path: &str,
) -> Result<Value, String> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        finish_http_span(span, Some(status), Some(&text));
        sentry::add_breadcrumb(sentry::Breadcrumb {
            ty: "http".to_string(),
            category: Some("mirofish".to_string()),
            message: Some(format!("MiroFish {path} failed with {status}")),
            level: sentry::Level::Warning,
            ..Default::default()
        });
        return Err(format!("MiroFish {path} {status}: {text}"));
    }

    let payload = serde_json::from_str::<Value>(&text).map_err(|err| {
        finish_http_span(span, Some(status), Some(&err.to_string()));
        format!("MiroFish parse error for {path}: {err}")
    })?;
    finish_http_span(span, Some(status), None);
    Ok(payload)
}

fn expect_success_payload(payload: &Value) -> Result<&Value, String> {
    let success = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if success {
        Ok(payload.get("data").unwrap_or(payload))
    } else {
        Err(payload
            .get("error")
            .and_then(Value::as_str)
            .or_else(|| payload.get("message").and_then(Value::as_str))
            .unwrap_or("Unknown MiroFish error")
            .to_string())
    }
}

fn required_string<'a>(payload: &'a Value, key: &str, label: &str) -> Result<&'a str, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("MiroFish payload missing {label}"))
}

fn task_error_message(task_payload: &Value, task_id: &str) -> String {
    task_payload
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| task_payload.get("message").and_then(Value::as_str))
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("MiroFish task {task_id} failed"))
}

fn simulation_info_from_value(payload: &Value) -> SimulationInfo {
    SimulationInfo {
        id: payload
            .get("simulation_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: payload
            .get("status")
            .and_then(Value::as_str)
            .or_else(|| payload.get("runner_status").and_then(Value::as_str))
            .unwrap_or("unknown")
            .to_string(),
        agent_count: payload
            .get("profiles_count")
            .and_then(Value::as_u64)
            .or_else(|| payload.get("entities_count").and_then(Value::as_u64))
            .map(|value| value as usize),
        project_id: payload
            .get("project_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        graph_id: payload
            .get("graph_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    }
}

fn sim_agents_from_profiles(profiles: &[Value]) -> Vec<SimAgent> {
    profiles
        .iter()
        .map(|profile| SimAgent {
            id: profile
                .get("user_id")
                .and_then(Value::as_i64)
                .map(|value| value.to_string())
                .or_else(|| {
                    profile
                        .get("user_id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                }),
            name: profile
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| profile.get("user_name").and_then(Value::as_str))
                .or_else(|| profile.get("username").and_then(Value::as_str))
                .unwrap_or("unknown")
                .to_string(),
            agent_type: profile
                .get("source_entity_type")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        })
        .collect()
}

fn sim_agents_from_graph_entities(entities: &[Value]) -> Vec<SimAgent> {
    entities
        .iter()
        .map(|entity| SimAgent {
            id: entity
                .get("uuid")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            name: entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            agent_type: entity
                .get("labels")
                .and_then(Value::as_array)
                .and_then(|labels| {
                    labels
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|label| *label != "Entity" && *label != "Node")
                })
                .map(ToString::to_string),
        })
        .collect()
}

fn extract_interview_response(payload: &Value) -> Result<String, String> {
    if let Some(response) = payload
        .get("result")
        .and_then(|value| value.get("response"))
        .and_then(Value::as_str)
    {
        return Ok(response.to_string());
    }

    if let Some(platforms) = payload
        .get("result")
        .and_then(|value| value.get("platforms"))
        .and_then(Value::as_object)
    {
        let combined = platforms
            .iter()
            .filter_map(|(platform, result)| {
                result
                    .get("response")
                    .and_then(Value::as_str)
                    .map(|response| format!("{platform}: {response}"))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if !combined.is_empty() {
            return Ok(combined);
        }
    }

    Err("MiroFish interview payload missing response text".to_string())
}

fn parse_agent_id(agent_id: &str) -> Result<i64, String> {
    agent_id
        .parse::<i64>()
        .map_err(|_| format!("MiroFish agent id '{agent_id}' is not numeric"))
}

fn capture_operation_event(level: Level, operation: &str, attrs: BTreeMap<String, Value>) {
    let event_kind = attrs
        .get("event.kind")
        .and_then(Value::as_str)
        .unwrap_or(operation);
    let event_kind = event_kind.to_string();
    sentry::with_scope(
        |scope| {
            scope.set_tag("event.category", "mirofish");
            scope.set_tag("event.kind", &event_kind);
            for (key, value) in attrs.clone() {
                scope.set_extra(&key, sentry_value_from_json(value));
            }
        },
        || {
            sentry::capture_message(operation, level);
        },
    );
}

fn sentry_value_from_json(value: Value) -> sentry::protocol::Value {
    match value {
        Value::Null => sentry::protocol::Value::Null,
        Value::Bool(value) => sentry::protocol::Value::Bool(value),
        Value::Number(value) => sentry::protocol::Value::Number(value),
        Value::String(value) => sentry::protocol::Value::String(value),
        Value::Array(values) => {
            sentry::protocol::Value::Array(values.into_iter().map(sentry_value_from_json).collect())
        }
        Value::Object(values) => sentry::protocol::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, sentry_value_from_json(value)))
                .collect(),
        ),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_new_sets_base_url() {
        let c = MirofishClient::new("http://localhost:5001/", 60, None);
        assert_eq!(c.base_url, "http://localhost:5001");
    }

    #[test]
    fn client_from_config_defaults() {
        let cfg = openfang_types::config::MirofishConfig::default();
        let c = MirofishClient::from_config(&cfg);
        assert_eq!(c.base_url, "http://localhost:5001");
    }
}
