use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::fs;
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
    storage_path: String,
}

impl EventManager {
    pub fn new(max_events: usize) -> Self {
        let storage_path = Self::get_storage_path();
        let events = Self::load_from_file(&storage_path, max_events);
        
        Self {
            events: Arc::new(RwLock::new(events)),
            max_events,
            storage_path,
        }
    }

    fn get_storage_path() -> String {
        // Use the same base directory as process dumps
        let base_path = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        format!("{}/.opm/events.dump", base_path)
    }

    fn load_from_file(path: &str, max_events: usize) -> VecDeque<Event> {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(events) = ron::de::from_str::<VecDeque<Event>>(&contents) {
                log::info!("Loaded {} events from {}", events.len(), path);
                // Trim to max_events if needed
                if events.len() > max_events {
                    let skip_count = events.len() - max_events;
                    events.into_iter().skip(skip_count).collect()
                } else {
                    events
                }
            } else {
                log::warn!("Failed to parse events file, starting fresh");
                VecDeque::with_capacity(max_events)
            }
        } else {
            log::info!("No existing events file found, starting fresh");
            VecDeque::with_capacity(max_events)
        }
    }

    fn save_to_file(&self, events: &VecDeque<Event>) {
        if let Ok(encoded) = ron::ser::to_string_pretty(events, ron::ser::PrettyConfig::default()) {
            if let Err(err) = fs::write(&self.storage_path, encoded) {
                log::error!("Failed to save events to {}: {}", self.storage_path, err);
            }
        } else {
            log::error!("Failed to encode events");
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
        
        // Save to file after adding
        self.save_to_file(&events);
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
        
        // Save empty state to file
        self.save_to_file(&events);
    }
}
