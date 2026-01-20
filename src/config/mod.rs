pub mod structs;

use crate::{
    file::{self, Exists},
    helpers,
    process::RemoteConfig,
};

use colored::Colorize;
use macros_rs::{crashln, fmtstr, string};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use structs::prelude::*;

use std::{fs::write, path::Path};

pub fn from(address: &str, token: Option<&str>) -> Result<RemoteConfig, anyhow::Error> {
    let client = Client::new();
    let mut headers = HeaderMap::new();

    if let Some(token) = token {
        headers.insert(
            "token",
            HeaderValue::from_static(Box::leak(Box::from(token))),
        );
    }

    let response = client
        .get(fmtstr!("{address}/daemon/config"))
        .headers(headers)
        .send()?;
    let json = response.json::<RemoteConfig>()?;

    Ok(json)
}

pub fn read() -> Config {
    match home::home_dir() {
        Some(path) => {
            let path = path.display();

            let config_path = format!("{path}/.opm/config.toml");

            if !Exists::check(&config_path).file() {
                // Generate a secure token for API protection
                let secure_token = uuid::Uuid::new_v4().to_string();

                let config = Config {
                    default: string!("local"),
                    runner: Runner {
                        shell: string!("/bin/sh"),
                        args: vec![string!("-c")],
                        node: string!("node"),
                        log_path: format!("{path}/.opm/logs"),
                    },
                    daemon: Daemon {
                        restarts: 10,
                        interval: 1000,
                        kind: string!("default"),
                        web: structs::Web {
                            ui: false,
                            api: false,
                            address: "127.0.0.1".to_string(),
                            port: 9876,
                            secure: Some(structs::Secure {
                                enabled: false,
                                token: secure_token,
                            }),
                            path: None,
                        },
                        notifications: None,
                        restore_cleanup: Some(structs::RestoreCleanup {
                            process_logs: true,
                            daemon_log: true,
                            agent_log: true,
                        }),
                    },
                    role: structs::Role::Standalone,
                };

                let contents = match toml::to_string(&config) {
                    Ok(contents) => contents,
                    Err(err) => crashln!(
                        "{} Cannot parse config.\n{}",
                        *helpers::FAIL,
                        string!(err).white()
                    ),
                };

                if let Err(err) = write(&config_path, contents) {
                    crashln!(
                        "{} Error writing config.\n{}",
                        *helpers::FAIL,
                        string!(err).white()
                    )
                }
                log::info!("created config file with secure API token");
            }

            // Read the config and check if secure token needs to be added
            let mut config: Config = file::read(config_path.clone());
            let mut needs_save = false;

            // If web.secure is None, generate and add a token
            if config.daemon.web.secure.is_none() {
                let secure_token = uuid::Uuid::new_v4().to_string();
                config.daemon.web.secure = Some(structs::Secure {
                    enabled: false,
                    token: secure_token,
                });
                needs_save = true;
                log::info!("added secure API token to existing config");
            }

            // Save config if it was modified
            if needs_save {
                config.save();
            }

            config
        }
        None => crashln!("{} Impossible to get your home directory", *helpers::FAIL),
    }
}

pub fn servers() -> Servers {
    match home::home_dir() {
        Some(path) => {
            let path = path.display();
            let config_path = format!("{path}/.opm/servers.toml");

            if !Exists::check(&config_path).file() {
                if let Err(err) = write(&config_path, "") {
                    crashln!(
                        "{} Error writing servers.\n{}",
                        *helpers::FAIL,
                        string!(err).white()
                    )
                }
            }

            file::read(config_path)
        }
        None => crashln!("{} Impossible to get your home directory", *helpers::FAIL),
    }
}

impl Config {
    pub fn check_shell_absolute(&self) -> bool {
        Path::new(&self.runner.shell).is_absolute()
    }

    pub fn save(&self) {
        match home::home_dir() {
            Some(path) => {
                let path = path.display();
                let config_path = format!("{path}/.opm/config.toml");

                let contents = match toml::to_string(&self) {
                    Ok(contents) => contents,
                    Err(err) => crashln!(
                        "{} Cannot parse config.\n{}",
                        *helpers::FAIL,
                        string!(err).white()
                    ),
                };

                if let Err(err) = write(&config_path, contents) {
                    crashln!(
                        "{} Error writing config.\n{}",
                        *helpers::FAIL,
                        string!(err).white()
                    )
                }
            }
            None => crashln!("{} Impossible to get your home directory", *helpers::FAIL),
        }
    }

    pub fn set_default(mut self, name: String) -> Self {
        self.default = string!(name);
        self
    }

    pub fn get_path(&self) -> String {
        self.daemon.web.path.clone().unwrap_or_else(|| string!("/"))
    }

    pub fn get_address(&self) -> rocket::Config {
        use std::net::{IpAddr, Ipv4Addr};

        let address = self
            .daemon
            .web
            .address
            .parse::<IpAddr>()
            .unwrap_or(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        rocket::Config {
            address,
            port: self.daemon.web.port as u16,
            log_level: rocket::config::LogLevel::Normal,
            ..rocket::Config::default()
        }
    }

    pub fn fmt_address(&self) -> String {
        format!("{}:{}", self.daemon.web.address, self.daemon.web.port)
    }

    /// Check if the current role allows controlling agent processes
    pub fn can_control_agents(&self) -> bool {
        matches!(self.role, structs::Role::Server)
    }

    /// Check if the current role is an agent
    pub fn is_agent(&self) -> bool {
        matches!(self.role, structs::Role::Agent)
    }

    /// Check if the current role is a server
    pub fn is_server(&self) -> bool {
        matches!(self.role, structs::Role::Server)
    }

    /// Get the role as a string
    pub fn get_role_name(&self) -> &str {
        match self.role {
            structs::Role::Server => "server",
            structs::Role::Agent => "agent",
            structs::Role::Standalone => "standalone",
        }
    }
}
