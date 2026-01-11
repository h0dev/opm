use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod prelude {
    pub use super::{Config, Daemon, Runner, Server, Servers, Secure, Web};
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub default: String,
    pub runner: Runner,
    pub daemon: Daemon,
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
