use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod prelude {
    pub use super::{Config, Daemon, Notifications, RestoreCleanup, Role, Runner, Secure, Server, Servers, Web};
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum Role {
    /// Server role: can control all processes (local + all agents)
    Server,
    /// Agent role: can only control own local processes
    Agent,
    /// Standalone role: default mode, operates independently
    Standalone,
}

impl Default for Role {
    fn default() -> Self {
        Role::Standalone
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub default: String,
    pub runner: Runner,
    pub daemon: Daemon,
    #[serde(default)]
    pub role: Role,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Runner {
    pub shell: String,
    pub args: Vec<String>,
    pub node: String,
    pub log_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Daemon {
    pub restarts: u64,
    pub interval: u64,
    pub kind: String,
    #[serde(default = "default_web")]
    pub web: Web,
    #[serde(default)]
    pub notifications: Option<Notifications>,
    #[serde(default)]
    pub restore_cleanup: Option<RestoreCleanup>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct RestoreCleanup {
    #[serde(default = "default_true")]
    pub process_logs: bool,
    #[serde(default = "default_true")]
    pub daemon_log: bool,
    #[serde(default = "default_true")]
    pub agent_log: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Web {
    #[serde(default)]
    pub ui: bool,
    #[serde(default)]
    pub api: bool,
    #[serde(default = "default_address")]
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u64,
    pub secure: Option<Secure>,
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Secure {
    pub enabled: bool,
    pub token: String,
}

pub fn default_web() -> Web {
    Web {
        ui: false,
        api: false,
        address: "127.0.0.1".to_string(),
        port: 9876,
        secure: None,
        path: None,
    }
}

fn default_address() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u64 {
    9876
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Servers {
    pub servers: Option<BTreeMap<String, Server>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Server {
    pub address: String,
    pub token: Option<String>,
}

impl Server {
    pub fn get(&self) -> Self {
        Self {
            token: self.token.clone(),
            address: self.address.trim_end_matches('/').to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Notifications {
    #[serde(default)]
    pub enabled: bool,
    pub events: Option<NotificationEvents>,
    pub channels: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NotificationEvents {
    #[serde(default)]
    pub agent_connect: bool,
    #[serde(default)]
    pub agent_disconnect: bool,
    #[serde(default)]
    pub process_start: bool,
    #[serde(default)]
    pub process_stop: bool,
    #[serde(default)]
    pub process_crash: bool,
    #[serde(default)]
    pub process_restart: bool,
}
