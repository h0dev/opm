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

use std::{
    fs::write,
    path::Path,
};

pub fn from(address: &str, token: Option<&str>) -> Result<RemoteConfig, anyhow::Error> {
    let client = Client::new();
    let mut headers = HeaderMap::new();

    if let Some(token) = token {
        headers.insert("token", HeaderValue::from_static(Box::leak(Box::from(token))));
    }

    let response = client.get(fmtstr!("{address}/daemon/config")).headers(headers).send()?;
    let json = response.json::<RemoteConfig>()?;

    Ok(json)
}

pub fn read() -> Config {
    match home::home_dir() {
        Some(path) => {
            let path = path.display();

            let config_path = format!("{path}/.opm/config.toml");

            if !Exists::check(&config_path).file() {
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
                    },
                };

                let contents = match toml::to_string(&config) {
                    Ok(contents) => contents,
                    Err(err) => crashln!("{} Cannot parse config.\n{}", *helpers::FAIL, string!(err).white()),
                };

                if let Err(err) = write(&config_path, contents) {
                    crashln!("{} Error writing config.\n{}", *helpers::FAIL, string!(err).white())
                }
                log::info!("created config file");
            }

            file::read(config_path)
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
                    crashln!("{} Error writing servers.\n{}", *helpers::FAIL, string!(err).white())
                }
            }

            file::read(config_path)
        }
        None => crashln!("{} Impossible to get your home directory", *helpers::FAIL),
    }
}

impl Config {
    pub fn check_shell_absolute(&self) -> bool { Path::new(&self.runner.shell).is_absolute() }

    pub fn save(&self) {
        match home::home_dir() {
            Some(path) => {
                let path = path.display();
                let config_path = format!("{path}/.opm/config.toml");

                let contents = match toml::to_string(&self) {
                    Ok(contents) => contents,
                    Err(err) => crashln!("{} Cannot parse config.\n{}", *helpers::FAIL, string!(err).white()),
                };

                if let Err(err) = write(&config_path, contents) {
                    crashln!("{} Error writing config.\n{}", *helpers::FAIL, string!(err).white())
                }
            }
            None => crashln!("{} Impossible to get your home directory", *helpers::FAIL),
        }
    }

    pub fn set_default(mut self, name: String) -> Self {
        self.default = string!(name);
        self
    }
}
