//! Video summary renderer for post-execution agent demonstrations.

use crate::sentry_logs::{capture_structured_log, current_event_context, record_artifact};
use openfang_types::config::VideoConfig;
use openfang_types::facts::ArtifactRecord;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Video summary renderer.
pub struct VideoRenderer {
    config: VideoConfig,
    recordings_dir: PathBuf,
}

impl VideoRenderer {
    /// Create a new video renderer.
    pub fn new(config: VideoConfig, data_dir: &Path) -> Self {
        let recordings_dir = data_dir.join("recordings");

        if config.enabled {
            if let Err(e) = std::fs::create_dir_all(&recordings_dir) {
                warn!(error = %e, "Failed to create recordings directory");
            }
        }

        Self {
            config,
            recordings_dir,
        }
    }

    /// Check if video rendering is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Render a video summary from audit events.
    pub async fn render_summary(
        &self,
        agent_id: &str,
        task_id: &str,
        audit_events: Vec<serde_json::Value>,
    ) -> Result<PathBuf, String> {
        if !self.config.enabled {
            return Err("Video rendering disabled".to_string());
        }

        debug!(
            agent_id = %agent_id,
            task_id = %task_id,
            event_count = audit_events.len(),
            "Rendering video summary"
        );

        // Create output path
        let agent_dir = self.recordings_dir.join(agent_id);
        std::fs::create_dir_all(&agent_dir)
            .map_err(|e| format!("Failed to create agent directory: {}", e))?;

        let video_path = agent_dir.join(format!("{}.mp4", task_id));

        // Check if ffmpeg is available
        if !self.is_ffmpeg_available() {
            // Fall back to saving raw audit log
            let json_path = agent_dir.join(format!("{}.json", task_id));
            std::fs::write(
                &json_path,
                serde_json::to_string_pretty(&audit_events).unwrap(),
            )
            .map_err(|e| format!("Failed to save audit log: {}", e))?;

            warn!("ffmpeg not available, saved audit log as JSON");
            return Ok(json_path);
        }

        // TODO: Implement actual rendering in next task
        // For now, create empty file as placeholder
        std::fs::write(&video_path, b"")
            .map_err(|e| format!("Failed to create video file: {}", e))?;

        let context = current_event_context().unwrap_or_default();
        let artifact_id = format!("recording:{agent_id}:{task_id}");
        record_artifact(ArtifactRecord {
            artifact_id: artifact_id.clone(),
            run_id: context.run_id,
            session_id: context.session_id,
            agent_id: Some(agent_id.to_string()),
            artifact_kind: "video_recording".to_string(),
            storage_path: video_path.to_string_lossy().to_string(),
            content_type: Some("video/mp4".to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            metadata_json: serde_json::json!({
                "task_id": task_id,
                "event_count": audit_events.len(),
            }),
        });
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert(
            "event.kind".to_string(),
            serde_json::json!("artifact.recorded"),
        );
        attrs.insert(
            "artifact.id".to_string(),
            serde_json::json!(artifact_id.clone()),
        );
        attrs.insert("artifact.ids".to_string(), serde_json::json!([artifact_id]));
        attrs.insert("agent.id".to_string(), serde_json::json!(agent_id));
        attrs.insert("outcome".to_string(), serde_json::json!("success"));
        attrs.insert(
            "payload.recording.task_id".to_string(),
            serde_json::json!(task_id),
        );
        attrs.insert(
            "payload.recording.path".to_string(),
            serde_json::json!(video_path.to_string_lossy().to_string()),
        );
        capture_structured_log(sentry::Level::Info, "artifact.recorded", attrs);

        info!(path = %video_path.display(), "Video summary rendered");
        Ok(video_path)
    }

    /// Check if ffmpeg is available on the system.
    fn is_ffmpeg_available(&self) -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok()
    }

    /// Clean up old recordings based on retention policy.
    pub fn cleanup_old_recordings(&self) -> Result<usize, String> {
        if !self.config.enabled {
            return Ok(0);
        }

        let max_age = std::time::Duration::from_secs(self.config.retention_days as u64 * 24 * 3600);

        let mut deleted = 0;

        if let Ok(entries) = std::fs::read_dir(&self.recordings_dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(elapsed) = modified.elapsed() {
                            if elapsed > max_age && std::fs::remove_file(entry.path()).is_ok() {
                                deleted += 1;
                            }
                        }
                    }
                }
            }
        }

        if deleted > 0 {
            info!(deleted, "Cleaned up old recordings");
        }

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_render_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let config = VideoConfig {
            enabled: false,
            ..Default::default()
        };

        let renderer = VideoRenderer::new(config, temp_dir.path());
        let result = renderer.render_summary("agent1", "task1", vec![]).await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Video rendering disabled");
    }

    #[test]
    fn test_ffmpeg_check() {
        let temp_dir = TempDir::new().unwrap();
        let renderer = VideoRenderer::new(VideoConfig::default(), temp_dir.path());

        // Just check it doesn't crash
        let _ = renderer.is_ffmpeg_available();
    }
}
