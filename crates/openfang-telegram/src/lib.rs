//! Telegram channel integration for OpenFang.

use serde::{Deserialize, Serialize};
use teloxide::prelude::*;
use tracing::info;

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

impl TelegramCommand {
    /// Parse a text message into a command.
    pub fn parse_command(text: &str) -> Self {
        let text = text.trim();

        if let Some(args) = text.strip_prefix("/run ") {
            let parts: Vec<&str> = args.splitn(2, ' ').collect();
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

        if let Some(agent_id) = text.strip_prefix("/status ") {
            return TelegramCommand::Status {
                agent_id: agent_id.to_string(),
            };
        }

        if text == "/help" {
            return TelegramCommand::Help;
        }

        TelegramCommand::Unknown {
            text: text.to_string(),
        }
    }
}

/// Telegram bot for bidirectional communication.
#[derive(Clone)]
pub struct TelegramBot {
    bot: Bot,
    config: TelegramConfig,
}

impl TelegramBot {
    /// Create a new Telegram bot.
    pub fn new(config: TelegramConfig) -> Result<Self, String> {
        let bot_token = config
            .bot_token
            .as_ref()
            .ok_or_else(|| "Bot token not configured".to_string())?;

        let bot = Bot::new(bot_token);

        Ok(Self { bot, config })
    }

    /// Check if bot is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && self.config.bot_token.is_some()
    }

    /// Send a text message to a chat.
    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<(), String> {
        let chat_id_i64: i64 = chat_id
            .parse()
            .map_err(|_| format!("Invalid chat_id: {}", chat_id))?;

        self.bot
            .send_message(ChatId(chat_id_i64), text)
            .await
            .map_err(|e| format!("Failed to send message: {}", e))?;

        Ok(())
    }

    /// Start polling for messages and forward commands.
    pub async fn start_polling(
        self,
        command_tx: tokio::sync::mpsc::Sender<(String, TelegramCommand)>,
    ) -> Result<(), String> {
        if !self.is_enabled() {
            return Err("Bot not enabled or token missing".to_string());
        }

        info!("Starting Telegram bot polling");

        let allowed_users = self.config.allowed_users.clone();

        teloxide::repl(self.bot.clone(), move |bot: Bot, msg: Message| {
            let tx = command_tx.clone();
            let allowed = allowed_users.clone();

            async move {
                let chat_id = msg.chat.id.0.to_string();
                let user_id = msg.from.as_ref().map(|u| u.id.0.to_string());

                // Authorization check
                if !allowed.is_empty() {
                    if let Some(uid) = &user_id {
                        if !allowed.contains(uid) {
                            // Silent ignore for unauthorized users
                            return Ok(());
                        }
                    } else {
                        return Ok(());
                    }
                }

                if let Some(text) = msg.text() {
                    let command = TelegramCommand::parse_command(text);

                    // Send command to kernel
                    let _ = tx.send((chat_id.clone(), command.clone())).await;

                    // Send acknowledgment for known commands
                    match command {
                        TelegramCommand::Help => {
                            bot.send_message(
                                msg.chat.id,
                                "Available commands:\n\
                                /run <agent> <task> - Run a task on an agent\n\
                                /agents - List all agents\n\
                                /status <agent_id> - Get agent status\n\
                                /help - Show this help",
                            )
                            .await?;
                        }
                        TelegramCommand::Unknown { .. } => {
                            // Don't respond to unknown commands
                        }
                        _ => {
                            // Commands like /run, /agents, /status will be handled by kernel
                            bot.send_message(msg.chat.id, "Processing...").await?;
                        }
                    }
                }

                Ok(())
            }
        })
        .await;

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
        let cmd = TelegramCommand::parse_command("/run researcher analyze Bitcoin");

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
        let cmd = TelegramCommand::parse_command("/agents");
        assert!(matches!(cmd, TelegramCommand::ListAgents));
    }

    #[test]
    fn test_parse_help() {
        let cmd = TelegramCommand::parse_command("/help");
        assert!(matches!(cmd, TelegramCommand::Help));
    }

    #[test]
    fn test_parse_unknown() {
        let cmd = TelegramCommand::parse_command("Hello bot");

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

        let bot = TelegramBot::new(config).expect("Failed to create bot");

        assert!(bot.is_user_allowed("12345"));
        assert!(!bot.is_user_allowed("99999"));
    }

    #[test]
    fn test_empty_whitelist_allows_all() {
        let config = TelegramConfig {
            enabled: true,
            bot_token: Some("token".to_string()),
            allowed_users: vec![],
            rate_limit_per_minute: 10,
        };

        let bot = TelegramBot::new(config).expect("Failed to create bot");

        assert!(bot.is_user_allowed("anyone"));
    }
}
