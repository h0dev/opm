pub mod channels;

use crate::config::structs::Notifications;
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

    pub async fn send(&self, _event: NotificationEvent, _title: &str, _message: &str) {
        // Notification system has been removed
        // Events are now logged via the EventLog system instead
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
