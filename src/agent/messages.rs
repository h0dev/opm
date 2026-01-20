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
    /// System information update message
    SystemInfoUpdate {
        id: String,
        system_info: super::types::SystemInfo,
    },
    /// Process list update message
    ProcessUpdate {
        id: String,
        processes: Vec<serde_json::Value>,
    },
    /// Action request from server to agent
    ActionRequest {
        request_id: String,
        process_id: usize,
        method: String,
    },
    /// Action response from agent to server
    ActionResponse {
        request_id: String,
        success: bool,
        message: String,
    },
    /// Response message
    Response { success: bool, message: String },
    /// Ping message from server to agent
    Ping,
    /// Pong response from agent
    Pong,
}
