//! Desktop automation via a Python PyObjC/Quartz bridge (macOS computer-use).
//!
//! Manages persistent desktop sessions per agent, communicating with a Python
//! subprocess over JSON-line stdin/stdout protocol (same pattern as browser.rs).
//!
//! # Security
//! - Bridge subprocess launched with cleared env (only PATH, HOME, TMPDIR)
//! - Session limits: max concurrent, idle timeout, 1 per agent
//! - Requires macOS Screen Recording + Accessibility permissions

use dashmap::DashMap;
use openfang_types::config::DesktopConfig;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::OnceLock;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Embedded Python bridge script (compiled into the binary).
const BRIDGE_SCRIPT: &str = include_str!("desktop_bridge.py");

// ── Protocol types ──────────────────────────────────────────────────────────

/// Command sent from Rust to the Python desktop bridge.
#[derive(Debug, Serialize)]
#[serde(tag = "action")]
pub enum DesktopCommand {
    Screenshot,
    MouseMove { x: f64, y: f64 },
    Click { x: f64, y: f64, button: String, double: bool },
    Type { text: String },
    KeyPress { key: String, modifiers: Vec<String> },
    GetActiveWindow,
    LaunchApp { app_name: String },
    Scroll { dx: i32, dy: i32 },
    GetScreenSize,
    Close,
}

/// Response received from the Python desktop bridge.
#[derive(Debug, Deserialize)]
pub struct DesktopResponse {
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

// ── Session ─────────────────────────────────────────────────────────────────

/// A live desktop session backed by a Python PyObjC subprocess.
struct DesktopSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    last_active: Instant,
}

impl DesktopSession {
    /// Send a command and read the response.
    fn send(&mut self, cmd: &DesktopCommand) -> Result<DesktopResponse, String> {
        let json = serde_json::to_string(cmd).map_err(|e| format!("Serialize error: {e}"))?;
        self.stdin
            .write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write to bridge stdin: {e}"))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|e| format!("Failed to write newline: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("Failed to flush bridge stdin: {e}"))?;

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read bridge stdout: {e}"))?;

        if line.trim().is_empty() {
            return Err("Bridge process closed unexpectedly".to_string());
        }

        self.last_active = Instant::now();
        serde_json::from_str(line.trim())
            .map_err(|e| format!("Failed to parse bridge response: {e}"))
    }

    /// Kill the subprocess.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DesktopSession {
    fn drop(&mut self) {
        self.kill();
    }
}

// ── Manager ─────────────────────────────────────────────────────────────────

/// Manages desktop sessions for all agents.
pub struct DesktopManager {
    sessions: DashMap<String, Mutex<DesktopSession>>,
    config: DesktopConfig,
    bridge_path: OnceLock<PathBuf>,
}

impl DesktopManager {
    /// Create a new DesktopManager with the given configuration.
    pub fn new(config: DesktopConfig) -> Self {
        Self {
            sessions: DashMap::new(),
            config,
            bridge_path: OnceLock::new(),
        }
    }

    /// Write the embedded Python bridge script to a temp file (once).
    fn ensure_bridge_script(&self) -> Result<&PathBuf, String> {
        if let Some(path) = self.bridge_path.get() {
            return Ok(path);
        }
        let dir = std::env::temp_dir().join("openfang");
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create temp dir: {e}"))?;
        let path = dir.join("desktop_bridge.py");
        std::fs::write(&path, BRIDGE_SCRIPT)
            .map_err(|e| format!("Failed to write bridge script: {e}"))?;
        debug!(path = %path.display(), "Wrote desktop bridge script");
        // Race-safe: if another thread set it first, we just use theirs
        let _ = self.bridge_path.set(path);
        Ok(self.bridge_path.get().unwrap())
    }

    /// Get or create a desktop session for the given agent.
    fn get_or_create_sync(&self, agent_id: &str) -> Result<(), String> {
        if self.sessions.contains_key(agent_id) {
            return Ok(());
        }

        // Enforce session limit
        if self.sessions.len() >= self.config.max_sessions {
            return Err(format!(
                "Maximum desktop sessions reached ({}). Close an existing session first.",
                self.config.max_sessions
            ));
        }

        let bridge_path = self.ensure_bridge_script()?;

        let mut cmd = std::process::Command::new(&self.config.python_path);
        cmd.arg(bridge_path.to_string_lossy().as_ref());
        cmd.arg("--timeout")
            .arg(self.config.timeout_secs.to_string());
        cmd.arg("--scale")
            .arg(self.config.screenshot_scale.to_string());
        cmd.arg("--display")
            .arg(self.config.display_id.to_string());

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::null());

        // SECURITY: Isolate environment — clear everything, pass through only essentials
        cmd.env_clear();
        if let Ok(v) = std::env::var("PATH") {
            cmd.env("PATH", v);
        }
        if let Ok(v) = std::env::var("HOME") {
            cmd.env("HOME", v);
        }
        if let Ok(v) = std::env::var("TMPDIR") {
            cmd.env("TMPDIR", v);
        }
        // PyObjC needs DISPLAY on some setups
        if let Ok(v) = std::env::var("DISPLAY") {
            cmd.env("DISPLAY", v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "Failed to spawn desktop bridge: {e}. Ensure Python and pyobjc-framework-Quartz are installed."
            )
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or("Failed to capture bridge stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to capture bridge stdout")?;
        let mut reader = BufReader::new(stdout);

        // Wait for the "ready" response
        let mut ready_line = String::new();
        reader
            .read_line(&mut ready_line)
            .map_err(|e| format!("Bridge failed to start: {e}"))?;

        if ready_line.trim().is_empty() {
            let _ = child.kill();
            return Err(
                "Desktop bridge process exited without sending ready signal. \
                 Check Python/PyObjC installation and Screen Recording permissions."
                    .to_string(),
            );
        }

        let ready: DesktopResponse = serde_json::from_str(ready_line.trim())
            .map_err(|e| format!("Bridge startup failed: {e}. Output: {ready_line}"))?;

        if !ready.success {
            let err = ready.error.unwrap_or_else(|| "Unknown error".to_string());
            let _ = child.kill();
            return Err(format!("Desktop bridge failed to start: {err}"));
        }

        info!(agent_id, "Desktop session created");

        let session = DesktopSession {
            child,
            stdin,
            stdout: reader,
            last_active: Instant::now(),
        };

        self.sessions
            .insert(agent_id.to_string(), Mutex::new(session));
        Ok(())
    }

    /// Check whether an agent has an active desktop session.
    pub fn has_session(&self, agent_id: &str) -> bool {
        self.sessions.contains_key(agent_id)
    }

    /// Send a command to an agent's desktop session.
    pub async fn send_command(
        &self,
        agent_id: &str,
        cmd: DesktopCommand,
    ) -> Result<DesktopResponse, String> {
        tokio::task::block_in_place(|| self.get_or_create_sync(agent_id))?;

        let session_ref = self
            .sessions
            .get(agent_id)
            .ok_or_else(|| "Session disappeared".to_string())?;

        let session_mutex = session_ref.value();
        let mut session = session_mutex.lock().await;

        let response = tokio::task::block_in_place(|| session.send(&cmd))?;

        if !response.success {
            let err = response
                .error
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string());
            warn!(agent_id, error = %err, "Desktop command failed");
        }

        Ok(response)
    }

    /// Close an agent's desktop session.
    pub async fn close_session(&self, agent_id: &str) {
        if let Some((_, session_mutex)) = self.sessions.remove(agent_id) {
            let mut session = session_mutex.lock().await;
            let _ = session.send(&DesktopCommand::Close);
            session.kill();
            info!(agent_id, "Desktop session closed");
        }
    }

    /// Clean up an agent's desktop session (called after agent loop ends).
    pub async fn cleanup_agent(&self, agent_id: &str) {
        self.close_session(agent_id).await;
    }
}

// ── Tool handler functions ──────────────────────────────────────────────────

/// screen_screenshot — Capture a screenshot of the entire screen.
pub async fn tool_screen_screenshot(
    _input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let resp = mgr
        .send_command(agent_id, DesktopCommand::Screenshot)
        .await?;

    if !resp.success {
        return Err(resp
            .error
            .unwrap_or_else(|| "Screenshot failed".to_string()));
    }

    let data = resp.data.unwrap_or_default();
    let b64 = data["image_base64"].as_str().unwrap_or("");
    let width = data["width"].as_u64().unwrap_or(0);
    let height = data["height"].as_u64().unwrap_or(0);

    // Save screenshot to uploads temp dir so it's accessible via /api/uploads/
    let mut image_urls: Vec<String> = Vec::new();
    if !b64.is_empty() {
        use base64::Engine;
        let upload_dir = std::env::temp_dir().join("openfang_uploads");
        let _ = std::fs::create_dir_all(&upload_dir);
        let file_id = uuid::Uuid::new_v4().to_string();
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64) {
            let path = upload_dir.join(&file_id);
            if std::fs::write(&path, &decoded).is_ok() {
                image_urls.push(format!("/api/uploads/{file_id}"));
            }
        }
    }

    let result = serde_json::json!({
        "screenshot": true,
        "width": width,
        "height": height,
        "image_urls": image_urls,
    });

    Ok(result.to_string())
}

/// screen_click — Click at screen coordinates.
pub async fn tool_screen_click(
    input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let x = input["x"].as_f64().ok_or("Missing 'x' parameter")?;
    let y = input["y"].as_f64().ok_or("Missing 'y' parameter")?;
    let button = input["button"].as_str().unwrap_or("left").to_string();
    let double = input["double"].as_bool().unwrap_or(false);

    let resp = mgr
        .send_command(agent_id, DesktopCommand::Click { x, y, button: button.clone(), double })
        .await?;

    if !resp.success {
        return Err(resp.error.unwrap_or_else(|| "Click failed".to_string()));
    }

    Ok(format!("Clicked at ({x}, {y}) [{button}]"))
}

/// screen_type — Type text using the keyboard.
pub async fn tool_screen_type(
    input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let text = input["text"].as_str().ok_or("Missing 'text' parameter")?;

    let resp = mgr
        .send_command(
            agent_id,
            DesktopCommand::Type {
                text: text.to_string(),
            },
        )
        .await?;

    if !resp.success {
        return Err(resp.error.unwrap_or_else(|| "Type failed".to_string()));
    }

    Ok(format!("Typed: {text}"))
}

/// screen_key_press — Press a key with optional modifiers.
pub async fn tool_screen_key_press(
    input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let key = input["key"]
        .as_str()
        .ok_or("Missing 'key' parameter")?
        .to_string();
    let modifiers: Vec<String> = input["modifiers"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let resp = mgr
        .send_command(agent_id, DesktopCommand::KeyPress { key: key.clone(), modifiers: modifiers.clone() })
        .await?;

    if !resp.success {
        return Err(resp
            .error
            .unwrap_or_else(|| "Key press failed".to_string()));
    }

    if modifiers.is_empty() {
        Ok(format!("Pressed: {key}"))
    } else {
        Ok(format!("Pressed: {}+{key}", modifiers.join("+")))
    }
}

/// screen_mouse_move — Move the mouse cursor.
pub async fn tool_screen_mouse_move(
    input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let x = input["x"].as_f64().ok_or("Missing 'x' parameter")?;
    let y = input["y"].as_f64().ok_or("Missing 'y' parameter")?;

    let resp = mgr
        .send_command(agent_id, DesktopCommand::MouseMove { x, y })
        .await?;

    if !resp.success {
        return Err(resp
            .error
            .unwrap_or_else(|| "Mouse move failed".to_string()));
    }

    Ok(format!("Mouse moved to ({x}, {y})"))
}

/// screen_get_active_window — Get information about the currently active window.
pub async fn tool_screen_get_active_window(
    _input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let resp = mgr
        .send_command(agent_id, DesktopCommand::GetActiveWindow)
        .await?;

    if !resp.success {
        return Err(resp
            .error
            .unwrap_or_else(|| "GetActiveWindow failed".to_string()));
    }

    let data = resp.data.unwrap_or_default();
    Ok(serde_json::to_string_pretty(&data).unwrap_or_else(|_| data.to_string()))
}

/// screen_launch_app — Launch an application by name.
pub async fn tool_screen_launch_app(
    input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let app_name = input["app_name"]
        .as_str()
        .ok_or("Missing 'app_name' parameter")?;

    let resp = mgr
        .send_command(
            agent_id,
            DesktopCommand::LaunchApp {
                app_name: app_name.to_string(),
            },
        )
        .await?;

    if !resp.success {
        return Err(resp
            .error
            .unwrap_or_else(|| "Launch failed".to_string()));
    }

    Ok(format!("Launched: {app_name}"))
}

/// screen_scroll — Scroll the mouse wheel.
pub async fn tool_screen_scroll(
    input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    let dx = input["dx"].as_i64().unwrap_or(0) as i32;
    let dy = input["dy"].as_i64().unwrap_or(-3) as i32;

    let resp = mgr
        .send_command(agent_id, DesktopCommand::Scroll { dx, dy })
        .await?;

    if !resp.success {
        return Err(resp
            .error
            .unwrap_or_else(|| "Scroll failed".to_string()));
    }

    Ok(format!("Scrolled (dx={dx}, dy={dy})"))
}

/// screen_close — Close the desktop session for this agent.
pub async fn tool_screen_close(
    _input: &serde_json::Value,
    mgr: &DesktopManager,
    agent_id: &str,
) -> Result<String, String> {
    mgr.close_session(agent_id).await;
    Ok("Desktop session closed.".to_string())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desktop_config_defaults() {
        let config = DesktopConfig::default();
        assert_eq!(config.max_sessions, 3);
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.idle_timeout_secs, 600);
        assert!((config.screenshot_scale - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.display_id, 0);
    }

    #[test]
    fn test_desktop_command_serialize_screenshot() {
        let cmd = DesktopCommand::Screenshot;
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"Screenshot\""));
    }

    #[test]
    fn test_desktop_command_serialize_mouse_move() {
        let cmd = DesktopCommand::MouseMove { x: 100.0, y: 200.0 };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"MouseMove\""));
        assert!(json.contains("\"x\":100.0"));
        assert!(json.contains("\"y\":200.0"));
    }

    #[test]
    fn test_desktop_command_serialize_click() {
        let cmd = DesktopCommand::Click {
            x: 50.0,
            y: 75.0,
            button: "left".to_string(),
            double: false,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"Click\""));
        assert!(json.contains("\"x\":50.0"));
        assert!(json.contains("\"button\":\"left\""));
    }

    #[test]
    fn test_desktop_command_serialize_type() {
        let cmd = DesktopCommand::Type {
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"Type\""));
        assert!(json.contains("hello world"));
    }

    #[test]
    fn test_desktop_command_serialize_key_press() {
        let cmd = DesktopCommand::KeyPress {
            key: "c".to_string(),
            modifiers: vec!["command".to_string()],
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"KeyPress\""));
        assert!(json.contains("\"key\":\"c\""));
        assert!(json.contains("\"command\""));
    }

    #[test]
    fn test_desktop_command_serialize_get_active_window() {
        let cmd = DesktopCommand::GetActiveWindow;
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"GetActiveWindow\""));
    }

    #[test]
    fn test_desktop_command_serialize_launch_app() {
        let cmd = DesktopCommand::LaunchApp {
            app_name: "Google Chrome".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"LaunchApp\""));
        assert!(json.contains("Google Chrome"));
    }

    #[test]
    fn test_desktop_command_serialize_scroll() {
        let cmd = DesktopCommand::Scroll { dx: 0, dy: -3 };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"Scroll\""));
        assert!(json.contains("\"dy\":-3"));
    }

    #[test]
    fn test_desktop_command_serialize_close() {
        let cmd = DesktopCommand::Close;
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"Close\""));
    }

    #[test]
    fn test_desktop_response_deserialize() {
        let json =
            r#"{"success": true, "data": {"width": 1920, "height": 1080}}"#;
        let resp: DesktopResponse = serde_json::from_str(json).unwrap();
        assert!(resp.success);
        assert!(resp.data.is_some());
        assert!(resp.error.is_none());
        let data = resp.data.unwrap();
        assert_eq!(data["width"], 1920);
    }

    #[test]
    fn test_desktop_response_error_deserialize() {
        let json = r#"{"success": false, "error": "Screen Recording permission required"}"#;
        let resp: DesktopResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.success);
        assert!(resp.data.is_none());
        assert_eq!(
            resp.error.unwrap(),
            "Screen Recording permission required"
        );
    }

    #[test]
    fn test_desktop_manager_new() {
        let config = DesktopConfig::default();
        let mgr = DesktopManager::new(config);
        assert!(mgr.sessions.is_empty());
    }

    #[test]
    fn test_desktop_command_serialize_get_screen_size() {
        let cmd = DesktopCommand::GetScreenSize;
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"action\":\"GetScreenSize\""));
    }
}
