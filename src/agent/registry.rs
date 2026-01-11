use super::types::AgentInfo;
use crate::notifications::{NotificationManager, NotificationEvent};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Registry for managing connected agents on the server side
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentInfo>>>,
    notifier: Option<Arc<NotificationManager>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            notifier: None,
        }
    }

    pub fn with_notifier(notifier: Arc<NotificationManager>) -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            notifier: Some(notifier),
        }
    }

    pub fn register(&self, agent: AgentInfo) {
        let agent_name = agent.name.clone();
        let agent_id = agent.id.clone();
        
        let mut agents = self.agents.write().unwrap();
        agents.insert(agent.id.clone(), agent);
        drop(agents);

        // Send notification
        if let Some(ref notifier) = self.notifier {
            let notifier: Arc<NotificationManager> = Arc::clone(notifier);
            tokio::spawn(async move {
                notifier
                    .send(
                        NotificationEvent::AgentConnect,
                        "Agent Connected",
                        &format!("Agent '{}' (ID: {}) has connected", agent_name, agent_id),
                    )
                    .await;
            });
        }
    }

    pub fn unregister(&self, id: &str) {
        let mut agents = self.agents.write().unwrap();
        let agent = agents.remove(id);
        drop(agents);

        // Send notification
        if let (Some(notifier), Some(agent)) = (&self.notifier, agent) {
            let notifier: Arc<NotificationManager> = Arc::clone(notifier);
            let agent_name = agent.name.clone();
            let agent_id = agent.id.clone();
            tokio::spawn(async move {
                notifier
                    .send(
                        NotificationEvent::AgentDisconnect,
                        "Agent Disconnected",
                        &format!("Agent '{}' (ID: {}) has disconnected", agent_name, agent_id),
                    )
                    .await;
            });
        }
    }

    pub fn get(&self, id: &str) -> Option<AgentInfo> {
        let agents = self.agents.read().unwrap();
        agents.get(id).cloned()
    }

    pub fn list(&self) -> Vec<AgentInfo> {
        let agents = self.agents.read().unwrap();
        agents.values().cloned().collect()
    }

    pub fn update_heartbeat(&self, id: &str) {
        let mut agents = self.agents.write().unwrap();
        if let Some(agent) = agents.get_mut(id) {
            agent.last_seen = std::time::SystemTime::now();
        }
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
