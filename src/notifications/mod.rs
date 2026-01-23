use crate::config::structs::Notifications;
use notify_rust::{Notification, Urgency};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct NotificationManager {
    config: Arc<RwLock<Option<Notifications>>>,
}

impl NotificationManager {
    pub fn new(config: Option<Notifications>) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
        }
    }

    pub async fn update_config(&self, config: Option<Notifications>) {
        let mut cfg = self.config.write().await;
        *cfg = config;
    }

    pub async fn send(&self, event: NotificationEvent, title: &str, message: &str) {
        let config = self.config.read().await;

        if let Some(cfg) = config.as_ref() {
            if !cfg.enabled {
                return;
            }

            // Check if this event is enabled
            if let Some(events) = &cfg.events {
                if !event.is_enabled(events) {
                    return;
                }
            }

            // Send desktop notification (may fail in headless environments, which is OK)
            if let Err(e) = self.send_desktop_notification(event, title, message).await {
                log::debug!("Desktop notification not available: {}", e);
            }

            // Send to configured external channels
            if let Some(channels) = &cfg.channels {
                if !channels.is_empty() {
                    if let Err(e) = self
                        .send_channel_notifications(title, message, channels)
                        .await
                    {
                        log::warn!("Failed to send channel notifications: {}", e);
                    }
                }
            }
        }
    }

    async fn send_desktop_notification(
        &self,
        event: NotificationEvent,
        title: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Notification::new()
            .summary(title)
            .body(message)
            .urgency(event.urgency())
            .appname("OPM")
            .timeout(5000)
            .show()?;

        Ok(())
    }

    async fn send_channel_notifications(
        &self,
        title: &str,
        message: &str,
        channels: &[String],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use reqwest::Client;

        let client = Client::new();
        let mut errors = Vec::new();
        let mut success_count = 0;

        for channel_url in channels {
            // Parse the shoutrrr URL to determine the service type
            if let Some((service, rest)) = channel_url.split_once("://") {
                let result = match service {
                    "discord" => {
                        self.send_discord_webhook(&client, rest, title, message)
                            .await
                    }
                    "slack" => self.send_slack_webhook(&client, rest, title, message).await,
                    "telegram" => {
                        self.send_telegram_message(&client, rest, title, message)
                            .await
                    }
                    _ => {
                        log::warn!("Unsupported notification service: {}", service);
                        errors.push(format!("Unsupported service: {}", service));
                        continue;
                    }
                };

                match result {
                    Ok(_) => success_count += 1,
                    Err(e) => {
                        log::warn!("Failed to send to {}: {}", service, e);
                        errors.push(format!("{}: {}", service, e));
                    }
                }
            } else {
                log::warn!("Invalid channel URL format: {}", channel_url);
                errors.push(format!("Invalid URL format: {}", channel_url));
            }
        }

        if success_count > 0 {
            Ok(())
        } else if !errors.is_empty() {
            Err(errors.join("; ").into())
        } else {
            Err("No valid notification channels configured".into())
        }
    }

    async fn send_discord_webhook(
        &self,
        client: &reqwest::Client,
        webhook_data: &str,
        title: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Discord webhook URL format: token@id or full webhook URL
        // NOTE: The webhook token will appear in server access logs when using URL path construction.
        // For production use, consider using Discord's webhook API with proper authentication headers.
        let webhook_url = if webhook_data.starts_with("http") {
            webhook_data.to_string()
        } else {
            // Parse token@id format (shoutrrr: discord://token@id)
            // Discord API expects: https://discord.com/api/webhooks/{id}/{token}
            if let Some((token, id)) = webhook_data.split_once('@') {
                format!("https://discord.com/api/webhooks/{}/{}", id, token)
            } else {
                return Err(
                    "Invalid Discord webhook format: expected 'token@id' or full webhook URL"
                        .into(),
                );
            }
        };

        let mut payload = HashMap::new();
        payload.insert("content", format!("**{}**\n{}", title, message));

        let response = client.post(&webhook_url).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = if status.is_client_error() || status.is_server_error() {
                response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unable to read response body".to_string())
            } else {
                "Non-success status but no error details available".to_string()
            };
            return Err(format!(
                "Discord webhook failed with status: {} - Response: {}",
                status, body
            )
            .into());
        }

        Ok(())
    }

    async fn send_slack_webhook(
        &self,
        client: &reqwest::Client,
        webhook_data: &str,
        title: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Slack webhook URL format: full webhook URL is required
        let webhook_url = if webhook_data.starts_with("http") {
            webhook_data.to_string()
        } else {
            return Err("Slack webhooks require full URL format (e.g., https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXX)".into());
        };

        let mut payload = HashMap::new();
        payload.insert("text", format!("*{}*\n{}", title, message));

        let response = client.post(&webhook_url).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = if status.is_client_error() || status.is_server_error() {
                response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unable to read response body".to_string())
            } else {
                "Non-success status but no error details available".to_string()
            };
            return Err(format!(
                "Slack webhook failed with status: {} - Response: {}",
                status, body
            )
            .into());
        }

        Ok(())
    }

    async fn send_telegram_message(
        &self,
        client: &reqwest::Client,
        webhook_data: &str,
        title: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Telegram format: token@telegram?chats=@chat_id
        // Extract token and chat ID
        let (token, rest) = webhook_data
            .split_once('@')
            .ok_or("Invalid Telegram format: expected 'token@telegram?chats=@chat_id'")?;

        let chat_id = if let Some(query) = rest.strip_prefix("telegram?chats=") {
            query
        } else {
            return Err("Invalid Telegram format: expected 'token@telegram?chats=@chat_id'".into());
        };

        let api_url = format!("https://api.telegram.org/bot{}/sendMessage", token);
        let text = format!("<b>{}</b>\n{}", title, message);

        let mut payload = HashMap::new();
        payload.insert("chat_id", chat_id);
        payload.insert("text", &text);
        payload.insert("parse_mode", "HTML");

        let response = client.post(&api_url).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = if status.is_client_error() || status.is_server_error() {
                response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unable to read response body".to_string())
            } else {
                "Non-success status but no error details available".to_string()
            };
            return Err(format!(
                "Telegram API failed with status: {} - Response: {}",
                status, body
            )
            .into());
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NotificationEvent {
    AgentConnect,
    AgentDisconnect,
    ProcessStart,
    ProcessStop,
    ProcessCrash,
    ProcessRestart,
    ProcessDelete,
}

impl NotificationEvent {
    /// Check if this event is enabled in the configuration
    pub fn is_enabled(&self, events: &crate::config::structs::NotificationEvents) -> bool {
        match self {
            NotificationEvent::AgentConnect => events.agent_connect,
            NotificationEvent::AgentDisconnect => events.agent_disconnect,
            NotificationEvent::ProcessStart => events.process_start,
            NotificationEvent::ProcessStop => events.process_stop,
            NotificationEvent::ProcessCrash => events.process_crash,
            NotificationEvent::ProcessRestart => events.process_restart,
            NotificationEvent::ProcessDelete => events.process_delete,
        }
    }

    /// Get the urgency level for this event
    pub fn urgency(&self) -> Urgency {
        match self {
            NotificationEvent::ProcessCrash => Urgency::Critical,
            NotificationEvent::AgentDisconnect => Urgency::Normal,
            _ => Urgency::Low,
        }
    }
}
