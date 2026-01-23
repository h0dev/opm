use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use utoipa::ToSchema;
use uuid::Uuid;

// Default API port for agents (different from server default 9876)
pub const AGENT_DEFAULT_API_PORT: u16 = 9877;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    pub server_url: String,
    pub token: Option<String>,
    pub reconnect_interval: u64, // seconds
    pub heartbeat_interval: u64, // seconds
    pub api_address: String,     // Address where agent API is listening
    pub api_port: u16,
}

impl AgentConfig {
    pub fn new(server_url: String, name: Option<String>, token: Option<String>) -> Self {
        let id = Uuid::new_v4().to_string();
        let name = name.unwrap_or_else(|| {
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| format!("agent-{}", &id[..8]))
        });

        Self {
            id,
            name,
            server_url,
            token,
            reconnect_interval: 5,  // 5 seconds default
            heartbeat_interval: 30, // 30 seconds default
            api_address: "0.0.0.0".to_string(),
            api_port: AGENT_DEFAULT_API_PORT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub enum AgentStatus {
    Online,
    Offline,
    Connecting,
    Reconnecting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub enum ConnectionType {
    In,  // Inbound connection (agent connects to server)
    Out, // Outbound connection (server connects to agent)
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub hostname: Option<String>,
    pub status: AgentStatus,
    pub connection_type: ConnectionType,
    #[serde(with = "time_serializer")]
    pub last_seen: SystemTime,
    #[serde(with = "time_serializer")]
    pub connected_at: SystemTime,
    /// API endpoint where agent can be reached (e.g., "http://192.168.1.100:9877")
    pub api_endpoint: Option<String>,
    /// System information for this agent
    pub system_info: Option<SystemInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SystemInfo {
    pub os_name: String,
    pub os_version: String,
    pub arch: String,
    pub cpu_count: Option<usize>,
    pub total_memory: Option<u64>,
    pub resource_usage: Option<ResourceUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResourceUsage {
    /// CPU usage as percentage (0-100 per core, can exceed 100 for multi-core)
    pub cpu_usage: Option<f64>,
    /// Memory usage in KB
    pub memory_used: Option<u64>,
    /// Memory available in KB
    pub memory_available: Option<u64>,
    /// Memory usage as percentage (0-100)
    pub memory_percent: Option<f64>,
    /// Total disk space in KB
    pub disk_total: Option<u64>,
    /// Free disk space in KB
    pub disk_free: Option<u64>,
    /// Disk usage as percentage (0-100)
    pub disk_percent: Option<f64>,
    /// Load average (1 minute)
    pub load_avg_1: Option<f64>,
    /// Load average (5 minutes)
    pub load_avg_5: Option<f64>,
    /// Load average (15 minutes)
    pub load_avg_15: Option<f64>,
}

// Custom serializer for SystemTime to make it compatible with JSON
mod time_serializer {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let duration = time.duration_since(UNIX_EPOCH).unwrap();
        duration.as_secs().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + std::time::Duration::from_secs(secs))
    }
}

impl AgentInfo {
    pub fn new(id: String, name: String, connection_type: ConnectionType) -> Self {
        Self {
            id,
            name,
            hostname: None,
            status: AgentStatus::Connecting,
            connection_type,
            last_seen: SystemTime::now(),
            connected_at: SystemTime::now(),
            api_endpoint: None,
            system_info: None,
        }
    }
}
