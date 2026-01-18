use serde::{Deserialize, Serialize};

/// Message protocol for agent-server WebSocket communication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentMessage {
    /// Agent registration message
    Register {
        id: String,
        name: String,
        hostname: Option<String>,
        api_endpoint: Option<String>,
    },
    /// Heartbeat/ping message
    Heartbeat { id: String },
    /// Response message
    Response { success: bool, message: String },
    /// Ping message from server to agent
    Ping,
    /// Pong response from agent
    Pong,
}
