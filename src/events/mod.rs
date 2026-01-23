use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    AgentConnect,
    AgentDisconnect,
    ProcessStart,
    ProcessStop,
    ProcessCrash,
    ProcessRestart,
    ProcessDelete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub agent_id: String,
    pub agent_name: String,
    pub process_id: Option<String>,
    pub process_name: Option<String>,
    pub message: String,
}

impl Event {
    pub fn new(
        event_type: EventType,
        agent_id: String,
        agent_name: String,
        process_id: Option<String>,
        process_name: Option<String>,
        message: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type,
            agent_id,
            agent_name,
            process_id,
            process_name,
            message,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventManager {
    events: Arc<RwLock<VecDeque<Event>>>,
    max_events: usize,
}

impl EventManager {
    pub fn new(max_events: usize) -> Self {
        Self {
            events: Arc::new(RwLock::new(VecDeque::with_capacity(max_events))),
            max_events,
        }
    }

    pub async fn add_event(&self, event: Event) {
        let mut events = self.events.write().await;
        
        // Log before moving the event
        log::info!("Event added: {:?} - {}", event.event_type, event.message);
        
        // If we've reached the max, remove the oldest event
        if events.len() >= self.max_events {
            events.pop_front();
        }
        
        events.push_back(event);
    }

    pub async fn get_events(&self, limit: Option<usize>) -> Vec<Event> {
        let events = self.events.read().await;
        let limit = limit.unwrap_or(events.len()).min(events.len());
        
        // Return events in reverse order (newest first)
        events
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub async fn clear_events(&self) {
        let mut events = self.events.write().await;
        events.clear();
        log::info!("All events cleared");
    }
}
