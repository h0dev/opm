use super::types::AgentInfo;
use crate::process::ProcessItem;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Registry for managing connected agents on the server side
#[derive(Clone)]
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentInfo>>>,
    agent_processes: Arc<RwLock<HashMap<String, Vec<ProcessItem>>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            agent_processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(&self, agent: AgentInfo) {
        let mut agents = self.agents.write().unwrap();
        agents.insert(agent.id.clone(), agent);
    }

    pub fn unregister(&self, id: &str) {
        let mut agents = self.agents.write().unwrap();
        agents.remove(id);
        // Also remove process data
        let mut processes = self.agent_processes.write().unwrap();
        processes.remove(id);
    }

    pub fn get(&self, id: &str) -> Option<AgentInfo> {
        let agents = self.agents.read().unwrap();
        agents.get(id).cloned()
    }

    pub fn list(&self) -> Vec<AgentInfo> {
        let agents = self.agents.read().unwrap();
        agents.values().cloned().collect()
    }

    pub fn update_heartbeat(&self, id: &str) -> bool {
        let mut agents = self.agents.write().unwrap();
        if let Some(agent) = agents.get_mut(id) {
            agent.last_seen = std::time::SystemTime::now();
            true
        } else {
            false
        }
    }

    pub fn update_processes(&self, id: &str, processes: Vec<ProcessItem>) {
        let mut agent_processes = self.agent_processes.write().unwrap();
        agent_processes.insert(id.to_string(), processes);
    }

    pub fn get_processes(&self, id: &str) -> Option<Vec<ProcessItem>> {
        let agent_processes = self.agent_processes.read().unwrap();
        agent_processes.get(id).map(|v| {
            // Manually clone by serializing and deserializing
            v.iter()
                .filter_map(|p| {
                    serde_json::to_value(p)
                        .ok()
                        .and_then(|v| serde_json::from_value(v).ok())
                })
                .collect()
        })
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
