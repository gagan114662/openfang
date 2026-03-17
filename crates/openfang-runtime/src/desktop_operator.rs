use crate::sentry_logs::capture_structured_log;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use sentry::protocol::Value as SentryValue;
use sentry::Level;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FILE_REPLY_PREFIX: &str = "__OPENFANG_FILE__:";
const INVARIANT_OPERATOR_VALID_TRANSITION: &str = "operator_valid_transition";
const CLAUDE_EXTENSION_RESET_CHAT_ID: &str = "8444910202";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopOperatorConfig {
    pub enabled: bool,
    pub default_agent_name: String,
    pub exclusive_control: bool,
    pub interrupt_on_local_input: bool,
    pub max_session_hours: u64,
    pub step_timeout_secs: u64,
    pub wait_poll_interval_ms: u64,
    pub auto_verify_after_action: bool,
    pub progress_update_mode: String,
    pub telegram_progress_interval_secs: u64,
    pub helper_script_path: String,
    pub session_store_path: String,
}

impl Default for DesktopOperatorConfig {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let repo_root = home.join("Desktop").join("my projects").join("open_fang");
        Self {
            enabled: true,
            default_agent_name: "mac-operator".to_string(),
            exclusive_control: true,
            interrupt_on_local_input: false,
            max_session_hours: 8,
            step_timeout_secs: 45,
            wait_poll_interval_ms: 750,
            auto_verify_after_action: true,
            progress_update_mode: "milestones".to_string(),
            telegram_progress_interval_secs: 20,
            helper_script_path: repo_root
                .join("scripts")
                .join("desktop_control.py")
                .to_string_lossy()
                .to_string(),
            session_store_path: repo_root
                .join("artifacts")
                .join("operator")
                .join("sessions")
                .to_string_lossy()
                .to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopOperatorState {
    Idle,
    Planning,
    Acting,
    Verifying,
    WaitingForUi,
    WaitingForUser,
    Paused,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DesktopOperatorAction {
    pub action_type: String,
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DesktopOperatorVerify {
    pub verify_type: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DesktopOperatorPlan {
    pub summary: String,
    pub milestone: String,
    pub action: DesktopOperatorAction,
    pub verify: DesktopOperatorVerify,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DesktopOperatorObservation {
    pub frontmost_app: Option<String>,
    pub window_title: Option<String>,
    pub screenshot_path: Option<String>,
    pub browser_url: Option<String>,
    pub browser_page_state: Option<String>,
    pub page_text_excerpt: Option<String>,
    pub claude_response_excerpt: Option<String>,
    pub recent_issue_titles: Vec<String>,
    pub login_required: bool,
    pub failure_phase: Option<String>,
    pub failure_reason: Option<String>,
    pub claude_attached: Option<bool>,
    pub attempt_count: Option<u64>,
    pub response_started: Option<bool>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopOperatorStepResult {
    pub at: DateTime<Utc>,
    pub action: String,
    pub milestone: Option<String>,
    pub success: bool,
    pub details: Option<String>,
    pub observation: Option<DesktopOperatorObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopOperatorInterrupt {
    pub at: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopOperatorMilestone {
    pub at: DateTime<Utc>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopOperatorSession {
    pub chat_id: String,
    pub state: DesktopOperatorState,
    pub current_goal: Option<String>,
    pub current_subtask: Option<String>,
    pub active_app: Option<String>,
    pub window_title: Option<String>,
    pub screenshot_path: Option<String>,
    pub browser_url: Option<String>,
    pub browser_page_state: Option<String>,
    pub page_text_excerpt: Option<String>,
    pub recent_issue_titles: Vec<String>,
    pub login_required: bool,
    pub repo_path: Option<String>,
    pub branch: Option<String>,
    pub blockers: Vec<String>,
    pub milestones: Vec<DesktopOperatorMilestone>,
    pub recent_steps: Vec<DesktopOperatorStepResult>,
    pub summary_rollup: Option<String>,
    pub last_interrupt: Option<DesktopOperatorInterrupt>,
    pub last_updated: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl DesktopOperatorSession {
    fn new(chat_id: &str) -> Self {
        let now = Utc::now();
        Self {
            chat_id: chat_id.to_string(),
            state: DesktopOperatorState::Idle,
            current_goal: None,
            current_subtask: None,
            active_app: None,
            window_title: None,
            screenshot_path: None,
            browser_url: None,
            browser_page_state: None,
            page_text_excerpt: None,
            recent_issue_titles: Vec::new(),
            login_required: false,
            repo_path: None,
            branch: None,
            blockers: Vec::new(),
            milestones: Vec::new(),
            recent_steps: Vec::new(),
            summary_rollup: None,
            last_interrupt: None,
            last_updated: now,
            created_at: now,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OperatorReply {
    pub user_message: String,
}

#[derive(Debug, Clone)]
pub struct OperatorError {
    pub user_message: String,
    pub details: String,
    pub unsupported: bool,
}

impl OperatorError {
    fn unsupported(user_message: impl Into<String>) -> Self {
        let user_message = user_message.into();
        Self {
            details: user_message.clone(),
            user_message,
            unsupported: true,
        }
    }

    fn failed(user_message: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
            details: details.into(),
            unsupported: false,
        }
    }
}

fn state_name(state: &DesktopOperatorState) -> &'static str {
    match state {
        DesktopOperatorState::Idle => "idle",
        DesktopOperatorState::Planning => "planning",
        DesktopOperatorState::Acting => "acting",
        DesktopOperatorState::Verifying => "verifying",
        DesktopOperatorState::WaitingForUi => "waiting_for_ui",
        DesktopOperatorState::WaitingForUser => "waiting_for_user",
        DesktopOperatorState::Paused => "paused",
        DesktopOperatorState::Failed => "failed",
        DesktopOperatorState::Completed => "completed",
    }
}

fn is_allowed_transition(from: &DesktopOperatorState, to: &DesktopOperatorState) -> bool {
    if from == to {
        return true;
    }

    match from {
        DesktopOperatorState::Idle => matches!(
            to,
            DesktopOperatorState::Planning
                | DesktopOperatorState::Acting
                | DesktopOperatorState::Paused
                | DesktopOperatorState::Failed
                | DesktopOperatorState::Completed
        ),
        DesktopOperatorState::Planning => matches!(
            to,
            DesktopOperatorState::Acting
                | DesktopOperatorState::WaitingForUi
                | DesktopOperatorState::WaitingForUser
                | DesktopOperatorState::Paused
                | DesktopOperatorState::Failed
                | DesktopOperatorState::Completed
        ),
        DesktopOperatorState::Acting => matches!(
            to,
            DesktopOperatorState::Verifying
                | DesktopOperatorState::WaitingForUi
                | DesktopOperatorState::WaitingForUser
                | DesktopOperatorState::Paused
                | DesktopOperatorState::Failed
                | DesktopOperatorState::Completed
        ),
        DesktopOperatorState::Verifying => matches!(
            to,
            DesktopOperatorState::Planning
                | DesktopOperatorState::Acting
                | DesktopOperatorState::WaitingForUi
                | DesktopOperatorState::WaitingForUser
                | DesktopOperatorState::Paused
                | DesktopOperatorState::Failed
                | DesktopOperatorState::Completed
        ),
        DesktopOperatorState::WaitingForUi => matches!(
            to,
            DesktopOperatorState::Acting
                | DesktopOperatorState::Verifying
                | DesktopOperatorState::Paused
                | DesktopOperatorState::Failed
                | DesktopOperatorState::Completed
        ),
        DesktopOperatorState::WaitingForUser => matches!(
            to,
            DesktopOperatorState::Planning
                | DesktopOperatorState::Acting
                | DesktopOperatorState::Paused
                | DesktopOperatorState::Failed
                | DesktopOperatorState::Completed
        ),
        DesktopOperatorState::Paused => matches!(
            to,
            DesktopOperatorState::Planning
                | DesktopOperatorState::Failed
                | DesktopOperatorState::Completed
        ),
        DesktopOperatorState::Failed => {
            matches!(
                to,
                DesktopOperatorState::Planning | DesktopOperatorState::Completed
            )
        }
        DesktopOperatorState::Completed => {
            matches!(
                to,
                DesktopOperatorState::Planning | DesktopOperatorState::Completed
            )
        }
    }
}

pub struct DesktopOperatorManager {
    config: DesktopOperatorConfig,
    sessions: DashMap<String, DesktopOperatorSession>,
}

impl DesktopOperatorManager {
    fn emit_transition_invariant(
        &self,
        chat_id: &str,
        before: &DesktopOperatorState,
        after: &DesktopOperatorState,
        allowed: bool,
        session: &DesktopOperatorSession,
        transition_source: &str,
    ) {
        let mut attrs = BTreeMap::new();
        attrs.insert("event.kind".to_string(), json!("invariant.check"));
        attrs.insert(
            "invariant.name".to_string(),
            json!(INVARIANT_OPERATOR_VALID_TRANSITION),
        );
        attrs.insert(
            "invariant.result".to_string(),
            json!(if allowed { "pass" } else { "fail" }),
        );
        attrs.insert("operator.chat_id".to_string(), json!(chat_id));
        attrs.insert(
            "operator.transition_source".to_string(),
            json!(transition_source),
        );
        attrs.insert(
            "operator.state.before".to_string(),
            json!(state_name(before)),
        );
        attrs.insert("operator.state.after".to_string(), json!(state_name(after)));
        attrs.insert("operator.goal".to_string(), json!(session.current_goal));
        attrs.insert(
            "operator.current_subtask".to_string(),
            json!(session.current_subtask),
        );
        attrs.insert("operator.active_app".to_string(), json!(session.active_app));
        attrs.insert("operator.blockers".to_string(), json!(session.blockers));

        sentry::with_scope(
            |scope| {
                scope.set_tag("event.kind", "invariant.check");
                scope.set_tag("invariant.name", INVARIANT_OPERATOR_VALID_TRANSITION);
                scope.set_tag("invariant.result", if allowed { "pass" } else { "fail" });
                scope.set_tag("operator.state.before", state_name(before));
                scope.set_tag("operator.state.after", state_name(after));
                for (key, value) in attrs {
                    scope.set_extra(&key, sentry_value_from_json(value));
                }
            },
            || {
                sentry::capture_message(
                    if allowed {
                        "desktop operator transition checked"
                    } else {
                        "desktop operator transition violated"
                    },
                    if allowed { Level::Info } else { Level::Warning },
                );
            },
        );
    }

    pub fn new(config: DesktopOperatorConfig) -> Self {
        let manager = Self {
            config,
            sessions: DashMap::new(),
        };
        let _ = fs::create_dir_all(manager.store_dir());
        manager
    }

    pub fn config(&self) -> &DesktopOperatorConfig {
        &self.config
    }

    fn one_time_reset_marker_path(&self, chat_id: &str) -> PathBuf {
        self.store_dir()
            .join(format!(".session-reset-{chat_id}.done"))
    }

    fn maybe_reset_one_time_chat_session(&self, chat_id: &str) {
        if chat_id != CLAUDE_EXTENSION_RESET_CHAT_ID {
            return;
        }
        let marker = self.one_time_reset_marker_path(chat_id);
        if marker.exists() {
            return;
        }
        self.sessions.remove(chat_id);
        let _ = fs::remove_file(self.session_path(chat_id));
        let _ = fs::write(&marker, Utc::now().to_rfc3339());
    }

    fn is_claude_extension_action(action_type: &str) -> bool {
        matches!(
            action_type,
            "claude_analyze_sentry" | "claude_prompt" | "open_claude_extension"
        )
    }

    fn parse_failure_detail(details: &str) -> (Option<String>, Option<String>) {
        let mut phase = None;
        let mut reason = None;
        for part in details.split(';').map(str::trim) {
            if let Some(rest) = part.strip_prefix("failure_phase=") {
                phase = Some(rest.trim().to_string());
            } else if let Some(rest) = part.strip_prefix("failure_reason=") {
                reason = Some(rest.trim().to_string());
            }
        }
        (phase, reason)
    }

    fn parse_failure_metadata(details: &str) -> (Option<String>, Option<String>, Option<bool>, Option<u64>) {
        let mut phase = None;
        let mut reason = None;
        let mut attached = None;
        let mut attempt_count = None;
        for part in details.split(';').map(str::trim) {
            if let Some(rest) = part.strip_prefix("failure_phase=") {
                phase = Some(rest.trim().to_string());
            } else if let Some(rest) = part.strip_prefix("failure_reason=") {
                reason = Some(rest.trim().to_string());
            } else if let Some(rest) = part.strip_prefix("claude_attached=") {
                attached = rest.trim().parse::<bool>().ok();
            } else if let Some(rest) = part.strip_prefix("attempt_count=") {
                attempt_count = rest.trim().parse::<u64>().ok();
            }
        }
        (phase, reason, attached, attempt_count)
    }

    fn claude_phase_user_message(phase: Option<&str>, reason: Option<&str>) -> String {
        match phase.unwrap_or("unknown") {
            "preflight" => format!(
                "Blocked before desktop control started: {}",
                reason.unwrap_or("Desktop preflight failed.")
            ),
            "focus_sentry_tab" => "Failed focusing the Sentry issues tab in Chrome.".to_string(),
            "attach_sidepanel" => {
                if let Some(r) = reason {
                    if r.to_lowercase().contains("authorization") || r.to_lowercase().contains("oauth") {
                        return r.to_string();
                    }
                }
                "Failed attaching Claude to the right side panel on the Sentry tab.".to_string()
            }
            "ready_panel" => {
                "Claude side panel is busy and did not return to an idle composer.".to_string()
            }
            "submit" => {
                "Failed submitting the prompt from the attached Claude side panel.".to_string()
            }
            "response_wait" => "Claude did not start responding after submit.".to_string(),
            _ => reason
                .map(ToString::to_string)
                .unwrap_or_else(|| "Failed using Claude extension on this Mac.".to_string()),
        }
    }

    fn emit_claude_extension_lifecycle(
        &self,
        kind: &str,
        level: Level,
        chat_id: &str,
        plan: &DesktopOperatorPlan,
        observation: Option<&DesktopOperatorObservation>,
        details: Option<&str>,
    ) {
        let mut attrs = BTreeMap::new();
        attrs.insert("event.kind".to_string(), json!(kind));
        attrs.insert("operator.chat_id".to_string(), json!(chat_id));
        attrs.insert("operator.goal".to_string(), json!(self.get_or_create(chat_id).current_goal));
        attrs.insert(
            "operator.action".to_string(),
            json!(plan.action.action_type.clone()),
        );
        attrs.insert(
            "sentry.url".to_string(),
            json!(
                plan.action
                    .args
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("https://foolish.sentry.io/issues/?project=-1&statsPeriod=24h")
            ),
        );
        if let Some(obs) = observation {
            attrs.insert(
                "artifact.screenshot_path".to_string(),
                json!(obs.screenshot_path.clone()),
            );
            attrs.insert(
                "claude.attached".to_string(),
                json!(obs.claude_attached.unwrap_or(false)),
            );
            attrs.insert(
                "attempt_count".to_string(),
                json!(obs.attempt_count.unwrap_or(1)),
            );
            if let Some(phase) = obs.failure_phase.clone() {
                attrs.insert("failure_phase".to_string(), json!(phase));
            }
            if let Some(reason) = obs.failure_reason.clone() {
                attrs.insert("failure_reason".to_string(), json!(reason));
            }
            if let Some(error) = obs.error.as_deref() {
                let (phase, reason, attached, attempts) = Self::parse_failure_metadata(error);
                attrs.insert(
                    "failure_phase".to_string(),
                    json!(phase.unwrap_or_else(|| "unknown".to_string())),
                );
                attrs.insert(
                    "failure_reason".to_string(),
                    json!(reason.unwrap_or_else(|| error.to_string())),
                );
                if let Some(attached) = attached {
                    attrs.insert("claude.attached".to_string(), json!(attached));
                }
                if let Some(attempts) = attempts {
                    attrs.insert("attempt_count".to_string(), json!(attempts));
                }
            }
        }
        if let Some(raw) = details {
            let (phase, reason, attached, attempts) = Self::parse_failure_metadata(raw);
            if !attrs.contains_key("failure_phase") {
                attrs.insert(
                    "failure_phase".to_string(),
                    json!(phase.unwrap_or_else(|| "unknown".to_string())),
                );
            }
            if !attrs.contains_key("failure_reason") {
                attrs.insert(
                    "failure_reason".to_string(),
                    json!(reason.unwrap_or_else(|| raw.to_string())),
                );
            }
            if !attrs.contains_key("claude.attached") {
                attrs.insert(
                    "claude.attached".to_string(),
                    json!(attached.unwrap_or(false)),
                );
            }
            if !attrs.contains_key("attempt_count") {
                attrs.insert("attempt_count".to_string(), json!(attempts.unwrap_or(1u64)));
            }
        }
        if !attrs.contains_key("attempt_count") {
            attrs.insert("attempt_count".to_string(), json!(1u64));
        }
        if !attrs.contains_key("claude.attached") {
            attrs.insert("claude.attached".to_string(), json!(false));
        }
        capture_structured_log(level, kind, attrs);
    }

    pub fn get_or_create(&self, chat_id: &str) -> DesktopOperatorSession {
        self.maybe_reset_one_time_chat_session(chat_id);
        if let Some(entry) = self.sessions.get(chat_id) {
            return entry.clone();
        }
        if let Some(session) = self.load(chat_id) {
            self.sessions.insert(chat_id.to_string(), session.clone());
            return session;
        }
        let session = DesktopOperatorSession::new(chat_id);
        self.persist(&session);
        self.sessions.insert(chat_id.to_string(), session.clone());
        session
    }

    pub fn begin_goal(&self, chat_id: &str, goal: &str) -> DesktopOperatorSession {
        self.update_session(chat_id, |session| {
            session.current_goal = Some(goal.to_string());
            session.current_subtask = Some(goal.to_string());
        });
        self.set_state(chat_id, DesktopOperatorState::Planning)
    }

    pub fn set_state(&self, chat_id: &str, state: DesktopOperatorState) -> DesktopOperatorSession {
        let before = self.get_or_create(chat_id).state.clone();
        let allowed = is_allowed_transition(&before, &state);
        let session = self.update_session(chat_id, |session| {
            session.state = state;
        });
        self.emit_transition_invariant(
            chat_id,
            &before,
            &session.state,
            allowed,
            &session,
            "set_state",
        );
        session
    }

    pub fn record_milestone(&self, chat_id: &str, text: &str) -> DesktopOperatorSession {
        self.update_session(chat_id, |session| {
            session.milestones.push(DesktopOperatorMilestone {
                at: Utc::now(),
                text: text.to_string(),
            });
            if session.milestones.len() > 20 {
                let drain_to = session.milestones.len().saturating_sub(20);
                session.milestones.drain(0..drain_to);
            }
            session.current_subtask = Some(text.to_string());
        })
    }

    pub fn record_step(
        &self,
        chat_id: &str,
        action: &str,
        milestone: Option<&str>,
        success: bool,
        details: Option<String>,
        observation: Option<DesktopOperatorObservation>,
    ) -> DesktopOperatorSession {
        let before_state = self.get_or_create(chat_id).state.clone();
        let session = self.update_session(chat_id, |session| {
            if let Some(obs) = &observation {
                if let Some(app) = &obs.frontmost_app {
                    session.active_app = Some(app.clone());
                }
                if let Some(title) = &obs.window_title {
                    session.window_title = Some(title.clone());
                }
                if let Some(path) = &obs.screenshot_path {
                    session.screenshot_path = Some(path.clone());
                }
                if let Some(url) = &obs.browser_url {
                    session.browser_url = Some(url.clone());
                }
                if let Some(state) = &obs.browser_page_state {
                    session.browser_page_state = Some(state.clone());
                }
                if let Some(excerpt) = &obs.page_text_excerpt {
                    session.page_text_excerpt = Some(excerpt.clone());
                }
                if !obs.recent_issue_titles.is_empty() {
                    session.recent_issue_titles = obs.recent_issue_titles.clone();
                }
                session.login_required = obs.login_required;
                if let Some(err) = &obs.error {
                    if !err.is_empty() {
                        session.blockers.push(err.clone());
                    }
                }
            }
            if !success {
                if let Some(msg) = &details {
                    session.blockers.push(msg.clone());
                }
                session.state = DesktopOperatorState::Failed;
            }
            session.recent_steps.push(DesktopOperatorStepResult {
                at: Utc::now(),
                action: action.to_string(),
                milestone: milestone.map(ToString::to_string),
                success,
                details,
                observation,
            });
            if session.recent_steps.len() > 25 {
                let drain_to = session.recent_steps.len().saturating_sub(25);
                session.recent_steps.drain(0..drain_to);
            }
        });
        if !success {
            self.emit_transition_invariant(
                chat_id,
                &before_state,
                &session.state,
                is_allowed_transition(&before_state, &session.state),
                &session,
                "record_step_failure",
            );
        }
        session
    }

    pub fn pause(&self, chat_id: &str, reason: &str) -> DesktopOperatorSession {
        self.update_session(chat_id, |session| {
            session.last_interrupt = Some(DesktopOperatorInterrupt {
                at: Utc::now(),
                reason: reason.to_string(),
            });
        });
        self.set_state(chat_id, DesktopOperatorState::Paused)
    }

    pub fn resume(&self, chat_id: &str) -> DesktopOperatorSession {
        self.set_state(chat_id, DesktopOperatorState::Planning)
    }

    pub fn stop(&self, chat_id: &str) -> DesktopOperatorSession {
        self.update_session(chat_id, |session| {
            session.current_subtask = Some("Stopped".to_string());
        });
        self.set_state(chat_id, DesktopOperatorState::Completed)
    }

    pub fn status_text(&self, chat_id: &str) -> String {
        let session = self.get_or_create(chat_id);
        let goal = session.current_goal.as_deref().unwrap_or("No active goal");
        let subtask = session
            .current_subtask
            .as_deref()
            .unwrap_or("No current step");
        let app = session.active_app.as_deref().unwrap_or("Unknown");
        let mut lines = vec![
            format!("Operator status: {:?}", session.state),
            format!("Goal: {goal}"),
            format!("Current step: {subtask}"),
            format!("Frontmost app: {app}"),
        ];
        if let Some(blocker) = session.blockers.last() {
            lines.push(format!("Blocker: {blocker}"));
        }
        lines.join("\n")
    }

    pub fn current_step_text(&self, chat_id: &str) -> String {
        let session = self.get_or_create(chat_id);
        let subtask = session.current_subtask.as_deref().unwrap_or("Idle");
        let app = session.active_app.as_deref().unwrap_or("Unknown");
        let mut lines = vec![
            format!("Current step: {subtask}"),
            format!("Frontmost app: {app}"),
        ];
        if let Some(url) = session.browser_url.as_deref() {
            lines.push(format!("URL: {url}"));
        }
        if let Some(blocker) = session.blockers.last() {
            lines.push(format!("Blocker: {blocker}"));
        }
        lines.join("\n")
    }

    pub fn latest_screenshot_path(&self, chat_id: &str) -> Option<String> {
        self.get_or_create(chat_id).screenshot_path
    }

    pub fn append_summary_rollup(&self, chat_id: &str, summary: &str) -> DesktopOperatorSession {
        self.update_session(chat_id, |session| {
            session.summary_rollup = Some(summary.to_string());
        })
    }

    pub fn cleanup_expired_sessions(&self) {
        let cutoff = Utc::now() - chrono::Duration::hours(self.config.max_session_hours as i64);
        let stale: Vec<String> = self
            .sessions
            .iter()
            .filter(|entry| entry.last_updated < cutoff)
            .map(|entry| entry.chat_id.clone())
            .collect();
        for chat_id in stale {
            self.sessions.remove(&chat_id);
            let _ = fs::remove_file(self.session_path(&chat_id));
        }
    }

    pub fn generate_next_step(
        &self,
        chat_id: &str,
        user_message: &str,
    ) -> Result<DesktopOperatorPlan, OperatorError> {
        let trimmed = user_message.trim();
        let lower = trimmed.to_lowercase();
        let session = self.get_or_create(chat_id);

        if matches!(lower.as_str(), "hi" | "hello" | "hey" | "yo") {
            self.begin_goal(chat_id, "Ready for the next Mac task");
            return Ok(DesktopOperatorPlan {
                summary: "Ready. Tell me what to do on this Mac.".to_string(),
                milestone: "Ready".to_string(),
                action: DesktopOperatorAction {
                    action_type: "report_ready".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify::default(),
            });
        }

        if lower == "what all can u do on my mac"
            || lower == "what can u do on my mac"
            || lower == "what can you do on my mac"
            || lower == "what all can you do on my mac"
        {
            self.begin_goal(chat_id, "Describe Mac operator capabilities");
            return Ok(DesktopOperatorPlan {
                summary: "I can operate this Mac directly: open and control apps, click, type, scroll, inspect Chrome and Sentry, open the Claude extension, open Codex, take screenshots, record short screen videos, use Terminal, and continue multi-step work across Telegram messages. Give me the task directly and I’ll act on it.".to_string(),
                milestone: "Describing capabilities".to_string(),
                action: DesktopOperatorAction {
                    action_type: "answer_only".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify::default(),
            });
        }

        if lower.contains("codex") && (lower.contains("work with") || lower.contains("access")) {
            self.begin_goal(chat_id, "Describe Codex access");
            return Ok(DesktopOperatorPlan {
                summary: "Yes. I can use the local Codex app and CLI on this Mac, and I can open it live here instead of only describing it.".to_string(),
                milestone: "Describing Codex access".to_string(),
                action: DesktopOperatorAction {
                    action_type: "answer_only".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify::default(),
            });
        }

        if lower.contains("codex")
            && (lower.contains("what can")
                || lower.contains("do for me")
                || lower.contains("away for")
                || lower.contains("while i'm away")
                || lower.contains("while i am away"))
        {
            self.begin_goal(chat_id, "Describe Codex operator capabilities");
            return Ok(DesktopOperatorPlan {
                summary: "With Codex on this Mac, I can inspect the repo, edit files, run builds and tests, open browser workflows, and keep working through Telegram follow-ups while you are away. Give me the task directly and I will act on it here.".to_string(),
                milestone: "Describing Codex operator capabilities".to_string(),
                action: DesktopOperatorAction {
                    action_type: "answer_only".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify::default(),
            });
        }

        if lower.contains("extension")
            && (lower.contains("can u access")
                || lower.contains("can you access")
                || lower.contains("can u work with")
                || lower.contains("can you work with"))
        {
            self.begin_goal(chat_id, "Describe extension access");
            return Ok(DesktopOperatorPlan {
                summary: "Yes. I can open and interact with Chrome extension pages on this Mac, including the installed Claude extension. Say 'open Claude extension' and I will do it live.".to_string(),
                milestone: "Describing extension access".to_string(),
                action: DesktopOperatorAction {
                    action_type: "answer_only".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify::default(),
            });
        }

        if (lower.contains("claude extension") || lower.contains("claude browser extension"))
            && lower.contains("sentry")
            && (lower.contains("analy") || lower.contains("check") || lower.contains("review"))
        {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Opening Sentry and asking Claude to analyze it in the Chrome extension."
                    .to_string(),
                milestone: "Analyzing Sentry with Claude extension".to_string(),
                action: DesktopOperatorAction {
                    action_type: "claude_analyze_sentry".to_string(),
                    args: serde_json::json!({
                        "url": "https://foolish.sentry.io/issues/?project=-1&statsPeriod=24h",
                        "text": "Analyze the current Sentry issues page in this tab. Summarize the visible unresolved issues. If there are no unresolved issues, say exactly: No unresolved issues match the current Sentry filter."
                    }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Google Chrome".to_string()),
                },
            });
        }

        if lower.contains("open claude extension")
            || lower.contains("open the claude extension")
            || lower.contains("open claude browser extension")
            || lower.contains("use claude extension")
        {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Opening the Claude Chrome extension on this Mac.".to_string(),
                milestone: "Opening Claude extension".to_string(),
                action: DesktopOperatorAction {
                    action_type: "open_claude_extension".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Google Chrome".to_string()),
                },
            });
        }

        if lower.starts_with("ask claude ") || lower.starts_with("tell claude ") {
            let prompt = trimmed
                .split_once(' ')
                .map(|(_, rest)| rest.trim())
                .unwrap_or("");
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Sending your prompt to Claude in Chrome on this Mac.".to_string(),
                milestone: "Prompting Claude".to_string(),
                action: DesktopOperatorAction {
                    action_type: "claude_prompt".to_string(),
                    args: serde_json::json!({ "text": prompt }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Google Chrome".to_string()),
                },
            });
        }

        if lower.starts_with("type ")
            && (lower.contains(" in claude") || lower.contains(" into claude"))
        {
            let stripped = trimmed[5..].trim();
            let text = stripped
                .strip_suffix(" in Claude")
                .or_else(|| stripped.strip_suffix(" in claude"))
                .or_else(|| stripped.strip_suffix(" into Claude"))
                .or_else(|| stripped.strip_suffix(" into claude"))
                .unwrap_or(stripped)
                .trim();
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Typing into Claude in Chrome on this Mac.".to_string(),
                milestone: "Typing into Claude".to_string(),
                action: DesktopOperatorAction {
                    action_type: "claude_prompt".to_string(),
                    args: serde_json::json!({ "text": text }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Google Chrome".to_string()),
                },
            });
        }

        if lower == "open codex" || lower == "open codex app" || lower == "open the codex app" {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Opening Codex on this Mac.".to_string(),
                milestone: "Opening Codex".to_string(),
                action: DesktopOperatorAction {
                    action_type: "launch_app".to_string(),
                    args: serde_json::json!({ "app_name": "Codex" }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Codex".to_string()),
                },
            });
        }

        if lower.contains("screen recording")
            || lower.contains("screen record")
            || lower.contains("record my screen")
            || lower.contains("record the screen")
        {
            let seconds = lower
                .split_whitespace()
                .filter_map(|part| part.parse::<u64>().ok())
                .find(|value| (1..=30).contains(value))
                .unwrap_or(8);
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: format!("Recording this Mac screen for {seconds} seconds."),
                milestone: "Recording screen".to_string(),
                action: DesktopOperatorAction {
                    action_type: "screen_record".to_string(),
                    args: serde_json::json!({ "seconds": seconds }),
                },
                verify: DesktopOperatorVerify::default(),
            });
        }

        if lower.contains("extension") && lower.starts_with("open ") {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Opening Chrome extensions.".to_string(),
                milestone: "Opening Chrome extensions".to_string(),
                action: DesktopOperatorAction {
                    action_type: "open_url".to_string(),
                    args: serde_json::json!({ "url": "chrome://extensions/" }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Google Chrome".to_string()),
                },
            });
        }

        if matches!(
            lower.as_str(),
            "status" | "what are you doing" | "what are you doing now"
        ) {
            return Ok(DesktopOperatorPlan {
                summary: self.current_step_text(chat_id),
                milestone: "Status".to_string(),
                action: DesktopOperatorAction {
                    action_type: "report_status".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify::default(),
            });
        }

        if let Some(expr) = lower
            .strip_prefix("open calculator and type ")
            .or_else(|| lower.strip_prefix("open calculator type "))
        {
            let expr = expr.trim();
            if !expr.is_empty() {
                self.begin_goal(chat_id, trimmed);
                return Ok(DesktopOperatorPlan {
                    summary: format!("Opening Calculator and entering {expr}."),
                    milestone: "Operating Calculator".to_string(),
                    action: DesktopOperatorAction {
                        action_type: "calc_input".to_string(),
                        args: serde_json::json!({ "expression": expr }),
                    },
                    verify: DesktopOperatorVerify {
                        verify_type: "frontmost_app_is".to_string(),
                        value: Some("Calculator".to_string()),
                    },
                });
            }
        }

        if lower == "open calculator" {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Opening Calculator.".to_string(),
                milestone: "Opening Calculator".to_string(),
                action: DesktopOperatorAction {
                    action_type: "launch_app".to_string(),
                    args: serde_json::json!({ "app_name": "Calculator" }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Calculator".to_string()),
                },
            });
        }

        if lower == "open chrome" || lower == "open google chrome" {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Opening Chrome.".to_string(),
                milestone: "Opening Chrome".to_string(),
                action: DesktopOperatorAction {
                    action_type: "launch_app".to_string(),
                    args: serde_json::json!({ "app_name": "Google Chrome" }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "frontmost_app_is".to_string(),
                    value: Some("Google Chrome".to_string()),
                },
            });
        }

        if let Some(text_to_type) = lower
            .strip_prefix("type ")
            .or_else(|| lower.strip_prefix("write "))
        {
            let text_to_type = text_to_type.trim();
            if !text_to_type.is_empty() {
                self.begin_goal(chat_id, trimmed);
                return Ok(DesktopOperatorPlan {
                    summary: format!("Typing {text_to_type} into the current app."),
                    milestone: "Typing text".to_string(),
                    action: DesktopOperatorAction {
                        action_type: "type_text".to_string(),
                        args: serde_json::json!({ "text": text_to_type }),
                    },
                    verify: DesktopOperatorVerify::default(),
                });
            }
        }

        if lower == "screenshot" || lower == "/screenshot" {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Taking a screenshot.".to_string(),
                milestone: "Taking screenshot".to_string(),
                action: DesktopOperatorAction {
                    action_type: "screenshot".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify {
                    verify_type: "screenshot_exists".to_string(),
                    value: None,
                },
            });
        }

        if lower.contains("sentry")
            && (lower.contains("log")
                || lower.contains("error")
                || lower.contains("broke")
                || lower.contains("recent")
                || lower == "open sentry"
                || lower == "check sentry")
        {
            self.begin_goal(chat_id, trimmed);
            return Ok(DesktopOperatorPlan {
                summary: "Checking Sentry on this Mac.".to_string(),
                milestone: "Checking Sentry".to_string(),
                action: DesktopOperatorAction {
                    action_type: "open_sentry".to_string(),
                    args: serde_json::json!({
                        "url": "https://foolish.sentry.io/issues/?project=-1&statsPeriod=24h"
                    }),
                },
                verify: DesktopOperatorVerify {
                    verify_type: "chrome_frontmost_with_sentry_url".to_string(),
                    value: Some("https://foolish.sentry.io".to_string()),
                },
            });
        }

        if lower.contains("recent error")
            || lower.contains("recent issues")
            || lower == "what are the recent errors"
            || lower == "what are the recent issues"
            || lower == "what broke"
            || lower == "show me what you found"
            || (lower.contains("error")
                && (session
                    .browser_url
                    .as_deref()
                    .unwrap_or_default()
                    .contains("sentry.io")
                    || session
                        .current_goal
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains("sentry")))
        {
            self.begin_goal(chat_id, "Summarize live Sentry issues");
            return Ok(DesktopOperatorPlan {
                summary: "Summarizing the current visible Sentry issues.".to_string(),
                milestone: "Summarizing Sentry".to_string(),
                action: DesktopOperatorAction {
                    action_type: "summarize_sentry_recent_errors".to_string(),
                    args: Value::Null,
                },
                verify: DesktopOperatorVerify {
                    verify_type: "sentry_page_loaded".to_string(),
                    value: None,
                },
            });
        }

        self.plan_with_claude(chat_id, trimmed)
    }

    pub fn execute_action(
        &self,
        chat_id: &str,
        plan: &DesktopOperatorPlan,
    ) -> Result<DesktopOperatorObservation, OperatorError> {
        match plan.action.action_type.as_str() {
            "report_ready" | "report_status" => Ok(DesktopOperatorObservation {
                success: true,
                ..Default::default()
            }),
            "answer_only" => Ok(DesktopOperatorObservation {
                success: true,
                ..Default::default()
            }),
            "launch_app" => {
                let app_name = plan
                    .action
                    .args
                    .get("app_name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed opening app.", "Missing app_name in plan")
                    })?;
                self.run_helper(&["launch".to_string(), app_name.to_string()])
                    .map_err(|err| {
                        OperatorError::failed(format!("Failed opening {app_name}."), err)
                    })
                    .and_then(|_| self.current_window_observation())
            }
            "calc_input" => {
                let expr = plan
                    .action
                    .args
                    .get("expression")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OperatorError::failed(
                            "Failed typing into Calculator.",
                            "Missing expression in plan",
                        )
                    })?;
                self.run_helper(&["calc-input".to_string(), expr.to_string()])
                    .map_err(|err| OperatorError::failed("Failed typing into Calculator.", err))?;
                self.current_window_observation()
            }
            "screenshot" => {
                let value = self
                    .run_helper(&["screenshot".to_string()])
                    .map_err(|err| OperatorError::failed("Failed taking screenshot.", err))?;
                Ok(DesktopOperatorObservation {
                    screenshot_path: value
                        .get("path")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    success: value
                        .get("success")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    error: None,
                    ..Default::default()
                })
            }
            "open_sentry" => {
                let value = self
                    .run_helper(&["chrome-open-sentry".to_string()])
                    .map_err(|err| OperatorError::failed("Failed opening Sentry.", err))?;
                Ok(self.observation_from_browser_payload(&value))
            }
            "open_claude_extension" => {
                let value = self
                    .run_helper(&["chrome-open-claude-extension".to_string()])
                    .map_err(|err| {
                        OperatorError::failed("Failed opening the Claude extension.", err)
                    })?;
                Ok(self.observation_from_browser_payload(&value))
            }
            "claude_prompt" => {
                let text = plan
                    .action
                    .args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OperatorError::failed(
                            "Failed sending prompt to Claude.",
                            "Missing text in plan",
                        )
                    })?;
                let value = self
                    .run_helper(&["claude-prompt".to_string(), text.to_string()])
                    .map_err(|err| {
                        OperatorError::failed("Failed sending prompt to Claude.", err)
                    })?;
                Ok(self.observation_from_browser_payload(&value))
            }
            "claude_analyze_sentry" => {
                let url = plan
                    .action
                    .args
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("https://foolish.sentry.io/issues/?project=-1&statsPeriod=24h");
                let text = plan
                    .action
                    .args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OperatorError::failed(
                            "Failed analyzing Sentry with Claude.",
                            "Missing text in plan",
                        )
                    })?;
                self.run_helper(&["open-url".to_string(), url.to_string()])
                    .map_err(|err| OperatorError::failed("Failed opening Sentry.", err))?;
                let value = self
                    .run_helper(&["claude-prompt".to_string(), text.to_string()])
                    .map_err(|err| {
                        OperatorError::failed(
                            "Failed sending Sentry analysis prompt to Claude.",
                            err,
                        )
                    })?;
                Ok(self.observation_from_browser_payload(&value))
            }
            "open_codex" => self
                .run_helper(&["launch".to_string(), "Codex".to_string()])
                .map_err(|err| OperatorError::failed("Failed opening Codex.", err))
                .and_then(|_| self.current_window_observation()),
            "screen_record" => {
                let seconds = plan
                    .action
                    .args
                    .get("seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(8);
                let value = self
                    .run_helper(&["screen-record".to_string(), seconds.to_string()])
                    .map_err(|err| OperatorError::failed("Failed recording the screen.", err))?;
                Ok(DesktopOperatorObservation {
                    screenshot_path: value
                        .get("path")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    success: value
                        .get("success")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    error: value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    ..Default::default()
                })
            }
            "browser_state_refresh" => {
                let value = self
                    .run_helper(&["browser-state".to_string()])
                    .map_err(|err| OperatorError::failed("Failed reading browser state.", err))?;
                Ok(self.observation_from_browser_payload(&value))
            }
            "summarize_sentry_recent_errors" => {
                let session = self.get_or_create(chat_id);
                let value = self
                    .run_helper(&["browser-state".to_string()])
                    .map_err(|err| OperatorError::failed("Failed reading Sentry page.", err))?;
                let mut observation = self.observation_from_browser_payload(&value);
                let on_sentry = observation
                    .browser_url
                    .as_deref()
                    .unwrap_or_default()
                    .contains("sentry.io")
                    || observation
                        .window_title
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains("sentry");
                let stable_page_state = matches!(
                    observation.browser_page_state.as_deref(),
                    Some("issues_page") | Some("empty_issues_page") | Some("login_required")
                );
                let needs_refresh = !on_sentry
                    || (session
                        .current_goal
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains("sentry")
                        && observation.recent_issue_titles.is_empty()
                        && !observation.login_required
                        && !stable_page_state);
                if needs_refresh {
                    let refreshed = self
                        .run_helper(&["chrome-open-sentry".to_string()])
                        .map_err(|err| OperatorError::failed("Failed opening Sentry.", err))?;
                    observation = self.observation_from_browser_payload(&refreshed);
                }
                Ok(observation)
            }
            "open_url" => {
                let url = plan
                    .action
                    .args
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed opening URL.", "Missing URL in plan")
                    })?;
                self.run_helper(&["open-url".to_string(), url.to_string()])
                    .map_err(|err| OperatorError::failed("Failed opening URL.", err))?;
                let mut observation = self.current_window_observation()?;
                if let Ok(value) = self.run_helper(&["screenshot".to_string()]) {
                    observation.screenshot_path = value
                        .get("path")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                }
                Ok(observation)
            }
            "type_text" => {
                let text = plan
                    .action
                    .args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed typing text.", "Missing text in plan")
                    })?;
                self.run_helper(&["type".to_string(), text.to_string()])
                    .map_err(|err| OperatorError::failed("Failed typing text.", err))?;
                self.current_window_observation()
            }
            "keypress" => {
                let key = plan
                    .action
                    .args
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed pressing key.", "Missing key in plan")
                    })?;
                let mut args = vec!["keypress".to_string(), key.to_string()];
                if let Some(modifiers) = plan.action.args.get("modifiers").and_then(Value::as_array)
                {
                    for modifier in modifiers {
                        if let Some(value) = modifier.as_str() {
                            args.push(value.to_string());
                        }
                    }
                }
                self.run_helper(&args)
                    .map_err(|err| OperatorError::failed("Failed pressing key.", err))?;
                self.current_window_observation()
            }
            "move_mouse" => {
                let x = plan
                    .action
                    .args
                    .get("x")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed moving mouse.", "Missing x in plan")
                    })?;
                let y = plan
                    .action
                    .args
                    .get("y")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed moving mouse.", "Missing y in plan")
                    })?;
                self.run_helper(&["move".to_string(), x.to_string(), y.to_string()])
                    .map_err(|err| OperatorError::failed("Failed moving mouse.", err))?;
                self.current_window_observation()
            }
            "click" => {
                let x = plan
                    .action
                    .args
                    .get("x")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed clicking.", "Missing x in plan")
                    })?;
                let y = plan
                    .action
                    .args
                    .get("y")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed clicking.", "Missing y in plan")
                    })?;
                let mode = plan
                    .action
                    .args
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("left");
                self.run_helper(&[
                    "click".to_string(),
                    x.to_string(),
                    y.to_string(),
                    mode.to_string(),
                ])
                .map_err(|err| OperatorError::failed("Failed clicking.", err))?;
                self.current_window_observation()
            }
            "scroll" => {
                let dx = plan
                    .action
                    .args
                    .get("dx")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed scrolling.", "Missing dx in plan")
                    })?;
                let dy = plan
                    .action
                    .args
                    .get("dy")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed scrolling.", "Missing dy in plan")
                    })?;
                self.run_helper(&["scroll".to_string(), dx.to_string(), dy.to_string()])
                    .map_err(|err| OperatorError::failed("Failed scrolling.", err))?;
                self.current_window_observation()
            }
            "wait" => {
                let seconds = plan
                    .action
                    .args
                    .get("seconds")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        OperatorError::failed("Failed waiting.", "Missing seconds in plan")
                    })?;
                self.run_helper(&["wait".to_string(), seconds.to_string()])
                    .map_err(|err| OperatorError::failed("Failed waiting.", err))?;
                self.current_window_observation()
            }
            other => Err(OperatorError::unsupported(format!(
                "No action executor for {other}"
            ))),
        }
    }

    pub fn verify_action(
        &self,
        plan: &DesktopOperatorPlan,
        observation: &DesktopOperatorObservation,
    ) -> Result<bool, OperatorError> {
        match plan.verify.verify_type.as_str() {
            "" => Ok(true),
            "frontmost_app_is" => {
                let expected = plan.verify.value.as_deref().unwrap_or_default();
                Ok(observation.frontmost_app.as_deref() == Some(expected))
            }
            "window_title_contains" => {
                let expected = plan
                    .verify
                    .value
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase();
                let title = observation
                    .window_title
                    .clone()
                    .unwrap_or_default()
                    .to_lowercase();
                Ok(title.contains(&expected))
            }
            "active_window_contains" => {
                let expected = plan
                    .verify
                    .value
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase();
                let title = observation
                    .window_title
                    .clone()
                    .unwrap_or_default()
                    .to_lowercase();
                Ok(title.contains(&expected))
            }
            "screenshot_exists" => Ok(observation.screenshot_path.is_some()),
            "chrome_frontmost_with_sentry_url" => Ok(observation.frontmost_app.as_deref()
                == Some("Google Chrome")
                && observation
                    .browser_url
                    .as_deref()
                    .unwrap_or_default()
                    .contains(plan.verify.value.as_deref().unwrap_or("sentry.io"))),
            "sentry_page_loaded" => Ok(observation.frontmost_app.as_deref()
                == Some("Google Chrome")
                && (observation
                    .browser_url
                    .as_deref()
                    .unwrap_or_default()
                    .contains("sentry.io")
                    || observation
                        .window_title
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains("sentry"))),
            "sentry_issue_list_visible" => Ok(!observation.recent_issue_titles.is_empty()),
            "sentry_login_required" => Ok(observation.login_required),
            other => Err(OperatorError::failed(
                "Verification failed.",
                format!("Unknown verification type: {other}"),
            )),
        }
    }

    pub fn execute_operator_turn(
        &self,
        chat_id: &str,
        user_message: &str,
    ) -> Result<OperatorReply, OperatorError> {
        let plan = self.generate_next_step(chat_id, user_message)?;
        self.record_milestone(chat_id, &plan.milestone);
        self.set_state(chat_id, DesktopOperatorState::Acting);
        let is_claude_flow = Self::is_claude_extension_action(&plan.action.action_type);
        if is_claude_flow {
            self.emit_claude_extension_lifecycle(
                "operator.claude_extension.started",
                Level::Info,
                chat_id,
                &plan,
                None,
                None,
            );
        }

        let observation = match self.execute_action(chat_id, &plan) {
            Ok(observation) => observation,
            Err(err) => {
                let transformed = if is_claude_flow {
                    let (phase, reason) = Self::parse_failure_detail(&err.details);
                    OperatorError::failed(
                        Self::claude_phase_user_message(phase.as_deref(), reason.as_deref()),
                        err.details,
                    )
                } else {
                    err
                };
                if is_claude_flow {
                    self.emit_claude_extension_lifecycle(
                        "operator.claude_extension.failed",
                        Level::Warning,
                        chat_id,
                        &plan,
                        None,
                        Some(&transformed.details),
                    );
                }
                return Err(transformed);
            }
        };
        self.set_state(chat_id, DesktopOperatorState::Verifying);
        let verified = self.verify_action(&plan, &observation)?;

        if !verified {
            let reason = match plan.action.action_type.as_str() {
                "calc_input" => "Failed resetting Calculator display.".to_string(),
                "launch_app" => format!(
                    "Failed opening {}.",
                    plan.action
                        .args
                        .get("app_name")
                        .and_then(Value::as_str)
                        .unwrap_or("app")
                ),
                "open_sentry" => "Sentry opened but browser verification failed.".to_string(),
                "open_claude_extension" => "Failed opening the Claude extension.".to_string(),
                "claude_analyze_sentry" => {
                    let detail = observation.error.clone().unwrap_or_default();
                    let (phase, parsed_reason) = Self::parse_failure_detail(&detail);
                    Self::claude_phase_user_message(phase.as_deref(), parsed_reason.as_deref())
                }
                "open_codex" => "Failed opening Codex.".to_string(),
                "screen_record" => "Failed recording the screen.".to_string(),
                "summarize_sentry_recent_errors" => {
                    "Could not interpret the current Sentry page.".to_string()
                }
                _ => "Operator verification failed.".to_string(),
            };
            self.record_step(
                chat_id,
                &plan.action.action_type,
                Some(&plan.milestone),
                false,
                Some(reason.clone()),
                Some(observation.clone()),
            );
            if is_claude_flow {
                self.emit_claude_extension_lifecycle(
                    "operator.claude_extension.failed",
                    Level::Warning,
                    chat_id,
                    &plan,
                    Some(&observation),
                    Some(&reason),
                );
            }
            return Err(OperatorError::failed(reason.clone(), reason));
        }

        let reply = self.format_success_reply(&plan, &observation);
        self.record_step(
            chat_id,
            &plan.action.action_type,
            Some(&plan.milestone),
            true,
            Some(reply.clone()),
            Some(observation.clone()),
        );
        self.set_state(chat_id, DesktopOperatorState::Completed);
        if is_claude_flow {
            let latest = self.get_or_create(chat_id);
            let latest_obs = latest.recent_steps.last().and_then(|s| s.observation.as_ref());
            self.emit_claude_extension_lifecycle(
                "operator.claude_extension.completed",
                Level::Info,
                chat_id,
                &plan,
                latest_obs,
                None,
            );
        }
        Ok(OperatorReply {
            user_message: reply,
        })
    }

    fn format_success_reply(
        &self,
        plan: &DesktopOperatorPlan,
        observation: &DesktopOperatorObservation,
    ) -> String {
        match plan.action.action_type.as_str() {
            "report_ready" => plan.summary.clone(),
            "report_status" => plan.summary.clone(),
            "answer_only" => {
                if plan.summary.trim().is_empty() {
                    "Done.".to_string()
                } else {
                    plan.summary.clone()
                }
            }
            "launch_app" => {
                let app_name = plan
                    .action
                    .args
                    .get("app_name")
                    .and_then(Value::as_str)
                    .unwrap_or("app");
                format!("Opened {app_name}.")
            }
            "calc_input" => {
                let expr = plan
                    .action
                    .args
                    .get("expression")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                format!("Done. Calculator is open with {expr} entered.")
            }
            "screenshot" => {
                let path = observation.screenshot_path.as_deref().unwrap_or("unknown");
                format!("Saved screenshot: {path}")
            }
            "open_sentry" => self.format_sentry_reply(observation, true),
            "open_claude_extension" => match observation.window_title.as_deref() {
                Some(title) if !title.is_empty() => {
                    format!("Opened the Claude Chrome extension on this Mac ({title}).")
                }
                _ => "Opened the Claude Chrome extension on this Mac.".to_string(),
            },
            "claude_prompt" => {
                "Sent your prompt through the Claude Chrome extension on this Mac.".to_string()
            }
            "claude_analyze_sentry" => {
                if let Some(reply) = observation.claude_response_excerpt.as_deref() {
                    reply.to_string()
                } else {
                    "Opened Sentry and asked Claude to analyze it in the Chrome extension."
                        .to_string()
                }
            }
            "open_codex" => "Opened Codex on this Mac.".to_string(),
            "screen_record" => {
                let path = observation.screenshot_path.as_deref().unwrap_or("unknown");
                let filename = std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("screen-recording.mp4");
                self.file_reply(
                    path,
                    filename,
                    Some("Sent the screen recording.".to_string()),
                )
            }
            "summarize_sentry_recent_errors" => self.format_sentry_reply(observation, false),
            "open_url" => {
                let url = plan
                    .action
                    .args
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("url");
                let app = observation.frontmost_app.as_deref().unwrap_or("unknown");
                format!("Opened {url} in {app}.")
            }
            "type_text" | "keypress" | "move_mouse" | "click" | "scroll" | "wait" => {
                plan.summary.clone()
            }
            _ => "Done.".to_string(),
        }
    }

    fn current_window_observation(&self) -> Result<DesktopOperatorObservation, OperatorError> {
        let active = self
            .run_helper(&["active-window".to_string()])
            .map_err(|err| OperatorError::failed("Failed reading active window.", err))?;
        Ok(DesktopOperatorObservation {
            frontmost_app: active
                .get("data")
                .and_then(|d| d.get("app_name"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            window_title: active
                .get("data")
                .and_then(|d| d.get("window_title"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            success: active
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            error: None,
            screenshot_path: None,
            browser_url: None,
            browser_page_state: None,
            page_text_excerpt: None,
            claude_response_excerpt: None,
            recent_issue_titles: Vec::new(),
            login_required: false,
            failure_phase: None,
            failure_reason: None,
            claude_attached: None,
            attempt_count: None,
            response_started: None,
        })
    }

    fn observation_from_browser_payload(&self, value: &Value) -> DesktopOperatorObservation {
        let data = value.get("data").unwrap_or(value);
        let probe = data.get("probe").and_then(Value::as_object);
        let page_state = data
            .get("page_hint")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let browser_url = data
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                probe
                    .and_then(|p| p.get("url"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            });
        let recent_issue_titles = data
            .get("recent_issue_titles")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let page_looks_live = !recent_issue_titles.is_empty()
            || page_state.as_deref() == Some("issues_page")
            || page_state.as_deref() == Some("empty_issues_page")
            || page_state.as_deref() == Some("claude_extension")
            || browser_url
                .as_deref()
                .unwrap_or_default()
                .contains("/issues");
        DesktopOperatorObservation {
            frontmost_app: data
                .get("app_name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    data.get("active_window")
                        .and_then(|v| v.get("app_name"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                }),
            window_title: data
                .get("window_title")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    data.get("active_window")
                        .and_then(|v| v.get("window_title"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .or_else(|| {
                    probe
                        .and_then(|p| p.get("title"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                }),
            screenshot_path: data
                .get("screenshot_path")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            browser_url,
            browser_page_state: page_state,
            page_text_excerpt: data
                .get("page_text_excerpt")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .filter(|text| !text.trim().is_empty())
                .or_else(|| {
                    probe
                        .and_then(|p| p.get("content_excerpt"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                        .filter(|text| !text.trim().is_empty())
                }),
            claude_response_excerpt: data
                .get("claude_response_excerpt")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .filter(|text| !text.trim().is_empty()),
            recent_issue_titles,
            login_required: data
                .get("login_required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || (!page_looks_live
                    && probe
                        .and_then(|p| p.get("content_excerpt"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains("login with google")),
            failure_phase: value
                .get("failure_phase")
                .or_else(|| data.get("failure_phase"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            failure_reason: value
                .get("failure_reason")
                .or_else(|| data.get("failure_reason"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            claude_attached: data
                .get("claude.attached")
                .or_else(|| data.get("claude_attached"))
                .and_then(Value::as_bool),
            attempt_count: value
                .get("attempt_count")
                .or_else(|| data.get("attempt_count"))
                .and_then(Value::as_u64),
            response_started: value
                .get("response_started")
                .or_else(|| data.get("response_started"))
                .and_then(Value::as_bool),
            success: value
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        }
    }

    fn format_sentry_reply(
        &self,
        observation: &DesktopOperatorObservation,
        include_intro: bool,
    ) -> String {
        let prefix = if include_intro {
            format!(
                "Opened Sentry in {}.",
                observation
                    .frontmost_app
                    .as_deref()
                    .unwrap_or("Google Chrome")
            )
        } else {
            "Current Sentry page:".to_string()
        };

        if observation.login_required {
            return format!("{prefix} The page requires login on this Mac.");
        }

        if observation.browser_page_state.as_deref() == Some("empty_issues_page")
            || observation
                .page_text_excerpt
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains("no issues match your search")
        {
            return format!("{prefix} No unresolved issues match the current Sentry filter.");
        }

        if !observation.recent_issue_titles.is_empty() {
            let issues = observation
                .recent_issue_titles
                .iter()
                .take(5)
                .map(|title| format!("- {title}"))
                .collect::<Vec<_>>()
                .join("\n");
            return format!("{prefix} Visible recent issues:\n{issues}");
        }

        if let Some(excerpt) = observation.page_text_excerpt.as_deref() {
            let lines = excerpt
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .filter(|line| {
                    let lower = line.to_lowercase();
                    !lower.contains("sentry.io/")
                        && !matches!(lower.as_str(), "issues" | "feed")
                        && (lower.contains("error")
                            || lower.contains("warning")
                            || lower.contains("failed")
                            || lower.contains("timed out")
                            || lower.contains("timeout")
                            || lower.contains("validation")
                            || lower.contains("api error")
                            || lower.contains("codex cli"))
                })
                .take(4)
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                return format!("{prefix} Top visible lines:\n- {}", lines.join("\n- "));
            }
        }

        if observation
            .browser_url
            .as_deref()
            .unwrap_or_default()
            .contains("sentry.io")
        {
            return format!(
                "{prefix} Sentry page opened but the issue list is not clearly visible."
            );
        }

        format!("{prefix} Could not verify the current Sentry page.")
    }

    fn run_helper(&self, args: &[String]) -> Result<Value, String> {
        let output = Command::new("python3")
            .arg(&self.config.helper_script_path)
            .args(args)
            .output()
            .map_err(|e| format!("desktop helper failed to start: {e}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() {
            if !stdout.is_empty() {
                if let Ok(value) = serde_json::from_str::<Value>(&stdout) {
                    let phase = value
                        .get("failure_phase")
                        .or_else(|| value.get("data").and_then(|d| d.get("failure_phase")))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let reason = value
                        .get("failure_reason")
                        .or_else(|| value.get("data").and_then(|d| d.get("failure_reason")))
                        .or_else(|| value.get("error"))
                        .and_then(Value::as_str)
                        .unwrap_or("desktop helper failed");
                    let attached = value
                        .get("data")
                        .and_then(|d| d.get("claude.attached"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let attempt_count = value
                        .get("attempt_count")
                        .or_else(|| value.get("data").and_then(|d| d.get("attempt_count")))
                        .and_then(Value::as_u64)
                        .unwrap_or(1);
                    return Err(format!(
                        "failure_phase={phase}; failure_reason={reason}; claude_attached={attached}; attempt_count={attempt_count}"
                    ));
                }
            }
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                return Err(format!(
                    "desktop helper exited with status {}",
                    output.status
                ));
            }
            return Err(stderr);
        }
        serde_json::from_str(&stdout)
            .map_err(|e| format!("invalid desktop helper output: {e}"))
    }

    fn file_reply(&self, path: &str, filename: &str, text: Option<String>) -> String {
        format!(
            "{}{}",
            FILE_REPLY_PREFIX,
            serde_json::json!({
                "path": path,
                "filename": filename,
                "text": text
            })
        )
    }

    fn plan_with_claude(
        &self,
        chat_id: &str,
        user_message: &str,
    ) -> Result<DesktopOperatorPlan, OperatorError> {
        let session = self.get_or_create(chat_id);
        let observation = self
            .current_window_observation()
            .unwrap_or_else(|_| DesktopOperatorObservation::default());
        let schema = r#"{
  "type":"object",
  "properties":{
    "summary":{"type":"string"},
    "milestone":{"type":"string"},
    "action":{
      "type":"object",
      "properties":{
        "action_type":{
          "type":"string",
          "enum":[
            "answer_only",
            "launch_app",
            "open_url",
            "type_text",
            "keypress",
            "move_mouse",
            "click",
            "scroll",
            "wait",
            "screenshot",
            "open_sentry",
            "open_claude_extension",
            "claude_analyze_sentry",
            "open_codex",
            "summarize_sentry_recent_errors",
            "browser_state_refresh"
          ]
        },
        "args":{"type":"object"}
      },
      "required":["action_type","args"]
    },
    "verify":{
      "type":"object",
      "properties":{
        "verify_type":{
          "type":"string",
          "enum":[
            "",
            "frontmost_app_is",
            "window_title_contains",
            "active_window_contains",
            "screenshot_exists",
            "chrome_frontmost_with_sentry_url",
            "sentry_page_loaded",
            "sentry_issue_list_visible",
            "sentry_login_required"
          ]
        },
        "value":{"type":["string","null"]}
      },
      "required":["verify_type","value"]
    }
  },
  "required":["summary","milestone","action","verify"]
}"#;
        let system_prompt = "You are the Mac-local Telegram operator planner. Return exactly one next-step JSON object. Never ask intake questions. Prefer visible desktop actions on this Mac. If the request is a simple conversational answer, use action_type=answer_only and put the final user-facing answer in summary. Do not produce prose outside the JSON schema.";
        let prompt = format!(
            "Session state:\n- state: {:?}\n- goal: {}\n- current_subtask: {}\n- frontmost_app: {}\n- window_title: {}\n- summary_rollup: {}\n\nUser message:\n{}\n",
            session.state,
            session.current_goal.as_deref().unwrap_or("none"),
            session.current_subtask.as_deref().unwrap_or("none"),
            observation.frontmost_app.as_deref().unwrap_or("unknown"),
            observation.window_title.as_deref().unwrap_or("unknown"),
            session.summary_rollup.as_deref().unwrap_or("none"),
            user_message
        );
        let output = Command::new("/opt/homebrew/bin/claude")
            .args([
                "-p",
                "--output-format",
                "json",
                "--json-schema",
                schema,
                "--system-prompt",
                system_prompt,
                "--tools",
                "",
                "--permission-mode",
                "dontAsk",
                &prompt,
            ])
            .output()
            .map_err(|e| {
                OperatorError::failed(
                    "Operator plan step invalid.",
                    format!("claude failed to start: {e}"),
                )
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(OperatorError::failed(
                "Operator plan step invalid.",
                if stderr.is_empty() {
                    format!("claude exited with status {}", output.status)
                } else {
                    stderr
                },
            ));
        }
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
            OperatorError::failed(
                "Operator plan step invalid.",
                format!("invalid claude json output: {e}"),
            )
        })?;
        let structured = value.get("structured_output").cloned().ok_or_else(|| {
            OperatorError::failed(
                "Operator plan step invalid.",
                "claude response missing structured_output",
            )
        })?;
        serde_json::from_value(structured).map_err(|e| {
            OperatorError::failed(
                "Operator plan step invalid.",
                format!("invalid operator plan payload: {e}"),
            )
        })
    }

    fn update_session<F>(&self, chat_id: &str, mutator: F) -> DesktopOperatorSession
    where
        F: FnOnce(&mut DesktopOperatorSession),
    {
        let mut session = self.get_or_create(chat_id);
        mutator(&mut session);
        session.last_updated = Utc::now();
        self.persist(&session);
        self.sessions.insert(chat_id.to_string(), session.clone());
        session
    }

    fn load(&self, chat_id: &str) -> Option<DesktopOperatorSession> {
        let path = self.session_path(chat_id);
        let body = fs::read_to_string(path).ok()?;
        serde_json::from_str(&body).ok()
    }

    fn persist(&self, session: &DesktopOperatorSession) {
        let path = self.session_path(&session.chat_id);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(body) = serde_json::to_string_pretty(session) {
            let _ = fs::write(path, body);
        }
    }

    fn store_dir(&self) -> &Path {
        Path::new(&self.config.session_store_path)
    }

    fn session_path(&self, chat_id: &str) -> PathBuf {
        let safe = chat_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>();
        self.store_dir().join(format!("{safe}.json"))
    }
}

fn sentry_value_from_json(value: Value) -> SentryValue {
    match value {
        Value::Null => SentryValue::Null,
        Value::Bool(v) => SentryValue::Bool(v),
        Value::Number(v) => SentryValue::Number(v),
        Value::String(v) => SentryValue::String(v),
        Value::Array(values) => {
            SentryValue::Array(values.into_iter().map(sentry_value_from_json).collect())
        }
        Value::Object(values) => SentryValue::Object(
            values
                .into_iter()
                .map(|(k, v)| (k, sentry_value_from_json(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn manager() -> DesktopOperatorManager {
        let dir = tempdir().unwrap();
        let cfg = DesktopOperatorConfig {
            session_store_path: dir.path().to_string_lossy().to_string(),
            helper_script_path: "/bin/false".to_string(),
            ..Default::default()
        };
        DesktopOperatorManager::new(cfg)
    }

    #[test]
    fn creates_and_persists_session() {
        let mgr = manager();
        let session = mgr.begin_goal("chat-1", "Open Calculator");
        assert_eq!(session.current_goal.as_deref(), Some("Open Calculator"));
        assert!(matches!(session.state, DesktopOperatorState::Planning));
    }

    #[test]
    fn pause_and_resume_round_trip() {
        let mgr = manager();
        mgr.begin_goal("chat-2", "Check Sentry");
        let paused = mgr.pause("chat-2", "manual");
        assert!(matches!(paused.state, DesktopOperatorState::Paused));
        let resumed = mgr.resume("chat-2");
        assert!(matches!(resumed.state, DesktopOperatorState::Planning));
    }

    #[test]
    fn calculator_plan_parses() {
        let mgr = manager();
        let plan = mgr
            .generate_next_step("chat-3", "open calculator and type 123")
            .unwrap();
        assert_eq!(plan.action.action_type, "calc_input");
        assert_eq!(
            plan.action.args.get("expression").and_then(Value::as_str),
            Some("123")
        );
    }

    #[test]
    fn sentry_empty_page_reply_is_explicit() {
        let mgr = manager();
        let reply = mgr.format_sentry_reply(
            &DesktopOperatorObservation {
                frontmost_app: Some("Google Chrome".to_string()),
                browser_page_state: Some("empty_issues_page".to_string()),
                page_text_excerpt: Some("No issues match your search".to_string()),
                success: true,
                ..Default::default()
            },
            true,
        );
        assert!(reply.contains("No unresolved issues match the current Sentry filter."));
    }

    #[test]
    fn transition_matrix_rejects_skipping_verification_from_idle() {
        assert!(!is_allowed_transition(
            &DesktopOperatorState::Idle,
            &DesktopOperatorState::Verifying
        ));
        assert!(is_allowed_transition(
            &DesktopOperatorState::Planning,
            &DesktopOperatorState::Acting
        ));
    }
}
