use crate::config::structs::Notifications;
use notify_rust::{Notification, Urgency};
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
                let enabled = match event {
                    NotificationEvent::AgentConnect => events.agent_connect,
                    NotificationEvent::AgentDisconnect => events.agent_disconnect,
                    NotificationEvent::ProcessStart => events.process_start,
                    NotificationEvent::ProcessStop => events.process_stop,
                    NotificationEvent::ProcessCrash => events.process_crash,
                    NotificationEvent::ProcessRestart => events.process_restart,
                };

                if !enabled {
                    return;
                }
            }

            // Send desktop notification
            if let Err(e) = self.send_desktop_notification(event, title, message).await {
                log::warn!("Failed to send notification: {}", e);
            }

            // Future: Send to configured channels (Discord, Slack, etc.)
            // This would use the shoutrrr URLs from cfg.channels
        }
    }

    async fn send_desktop_notification(
        &self,
        event: NotificationEvent,
        title: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let urgency = match event {
            NotificationEvent::ProcessCrash => Urgency::Critical,
            NotificationEvent::AgentDisconnect => Urgency::Normal,
            _ => Urgency::Low,
        };

        Notification::new()
            .summary(title)
            .body(message)
            .urgency(urgency)
            .appname("OPM")
            .timeout(5000)
            .show()?;

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
}
