//! Telegram channel integration for OpenFang.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Telegram channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Whether Telegram integration is enabled.
    pub enabled: bool,
    /// Bot token (from BotFather).
    pub bot_token: Option<String>,
    /// Allowed user IDs (Telegram user IDs as strings).
    pub allowed_users: Vec<String>,
    /// Rate limit per minute.
    pub rate_limit_per_minute: u32,
}

/// Telegram command variants.
#[derive(Debug, Clone)]
pub enum TelegramCommand {
    /// Run a task on an agent.
    Run { agent: String, task: String },
    /// List all agents.
    ListAgents,
    /// Get agent status.
    Status { agent_id: String },
    /// Show help message.
    Help,
    /// Unknown command.
    Unknown { text: String },
}

/// Telegram channel for bidirectional communication.
pub struct TelegramChannel {
    config: TelegramConfig,
    command_tx: mpsc::Sender<(String, TelegramCommand)>, // (chat_id, command)
}

impl TelegramChannel {
    /// Create a new Telegram channel.
    pub fn new(
        config: TelegramConfig,
    ) -> (Self, mpsc::Receiver<(String, TelegramCommand)>) {
        let (command_tx, command_rx) = mpsc::channel(100);

        let channel = Self {
            config,
            command_tx,
        };

        (channel, command_rx)
    }

    /// Check if Telegram is enabled and configured.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && self.config.bot_token.is_some()
    }

    /// Parse a text message into a Telegram command.
    pub fn parse_command(text: &str) -> TelegramCommand {
        let text = text.trim();

        if text.starts_with("/run ") {
            let parts: Vec<&str> = text[5..].splitn(2, ' ').collect();
            if parts.len() == 2 {
                return TelegramCommand::Run {
                    agent: parts[0].to_string(),
                    task: parts[1].to_string(),
                };
            }
        }

        if text == "/agents" {
            return TelegramCommand::ListAgents;
        }

        if text.starts_with("/status ") {
            return TelegramCommand::Status {
                agent_id: text[8..].to_string(),
            };
        }

        if text == "/help" {
            return TelegramCommand::Help;
        }

        TelegramCommand::Unknown {
            text: text.to_string(),
        }
    }

    /// Start polling for messages (stub implementation).
    pub async fn start_polling(&self) -> Result<(), String> {
        if !self.is_enabled() {
            return Err("Telegram not enabled or bot token missing".to_string());
        }

        info!("Starting Telegram polling (stub - full implementation next)");

        // TODO: Implement actual teloxide polling in next task
        // For now, just verify config is valid

        Ok(())
    }

    /// Check if a user is allowed to interact with the bot.
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.config.allowed_users.is_empty() {
            return true; // Allow all if no whitelist configured
        }

        self.config.allowed_users.contains(&user_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_run_command() {
        let cmd = TelegramChannel::parse_command("/run researcher analyze Bitcoin");

        match cmd {
            TelegramCommand::Run { agent, task } => {
                assert_eq!(agent, "researcher");
                assert_eq!(task, "analyze Bitcoin");
            }
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_list_agents() {
        let cmd = TelegramChannel::parse_command("/agents");
        assert!(matches!(cmd, TelegramCommand::ListAgents));
    }

    #[test]
    fn test_parse_help() {
        let cmd = TelegramChannel::parse_command("/help");
        assert!(matches!(cmd, TelegramCommand::Help));
    }

    #[test]
    fn test_parse_unknown() {
        let cmd = TelegramChannel::parse_command("Hello bot");

        match cmd {
            TelegramCommand::Unknown { text } => {
                assert_eq!(text, "Hello bot");
            }
            _ => panic!("Expected Unknown command"),
        }
    }

    #[test]
    fn test_user_authorization() {
        let config = TelegramConfig {
            enabled: true,
            bot_token: Some("token".to_string()),
            allowed_users: vec!["12345".to_string()],
            rate_limit_per_minute: 10,
        };

        let (channel, _) = TelegramChannel::new(config);

        assert!(channel.is_user_allowed("12345"));
        assert!(!channel.is_user_allowed("99999"));
    }

    #[test]
    fn test_empty_whitelist_allows_all() {
        let config = TelegramConfig {
            enabled: true,
            bot_token: Some("token".to_string()),
            allowed_users: vec![],
            rate_limit_per_minute: 10,
        };

        let (channel, _) = TelegramChannel::new(config);

        assert!(channel.is_user_allowed("anyone"));
    }
}
