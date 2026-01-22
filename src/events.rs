/// Event logging system for tracking process and system events
///
/// This module provides functionality to log and retrieve events that occur
/// in the system, such as process lifecycle events (start, stop, crash, restart)
/// and agent connection events (connect, disconnect).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use utoipa::ToSchema;

/// Maximum number of events to keep in memory
const MAX_EVENTS: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Event {
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    ProcessStart,
    ProcessStop,
    ProcessCrash,
    ProcessRestart,
    AgentConnect,
    AgentDisconnect,
}

impl EventType {
    pub fn from_notification_event(event: crate::notifications::NotificationEvent) -> Self {
        match event {
            crate::notifications::NotificationEvent::ProcessStart => EventType::ProcessStart,
            crate::notifications::NotificationEvent::ProcessStop => EventType::ProcessStop,
            crate::notifications::NotificationEvent::ProcessCrash => EventType::ProcessCrash,
            crate::notifications::NotificationEvent::ProcessRestart => EventType::ProcessRestart,
            crate::notifications::NotificationEvent::AgentConnect => EventType::AgentConnect,
            crate::notifications::NotificationEvent::AgentDisconnect => EventType::AgentDisconnect,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventLog {
    events: Arc<RwLock<Vec<Event>>>,
    next_id: Arc<RwLock<u64>>,
}

impl EventLog {
    pub fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            next_id: Arc::new(RwLock::new(1)),
        }
    }

    /// Log a new event
    pub async fn log(&self, event_type: EventType, title: String, message: String) {
        let mut events = self.events.write().await;
        let mut next_id = self.next_id.write().await;

        let event = Event {
            id: *next_id,
            timestamp: Utc::now(),
            event_type,
            title,
            message,
        };

        events.push(event);

        // Keep only the last MAX_EVENTS events
        if events.len() > MAX_EVENTS {
            let drain_count = events.len() - MAX_EVENTS;
            events.drain(0..drain_count);
        }

        *next_id += 1;
    }

    /// Get all events
    pub async fn get_all(&self) -> Vec<Event> {
        let events = self.events.read().await;
        events.clone()
    }

    /// Get events filtered by type
    pub async fn get_by_type(&self, event_type: EventType) -> Vec<Event> {
        let events = self.events.read().await;
        events
            .iter()
            .filter(|e| e.event_type == event_type)
            .cloned()
            .collect()
    }

    /// Get the last N events
    pub async fn get_last(&self, count: usize) -> Vec<Event> {
        let events = self.events.read().await;
        let start = if events.len() > count {
            events.len() - count
        } else {
            0
        };
        events[start..].to_vec()
    }

    /// Clear all events
    pub async fn clear(&self) {
        let mut events = self.events.write().await;
        events.clear();
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}
