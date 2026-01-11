use super::types::AgentInfo;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Registry for managing connected agents on the server side
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentInfo>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(&self, agent: AgentInfo) {
        let mut agents = self.agents.write().unwrap();
        agents.insert(agent.id.clone(), agent);
    }

    pub fn unregister(&self, id: &str) {
        let mut agents = self.agents.write().unwrap();
        agents.remove(id);
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
