use crate::{
    file::{self, Exists},
    helpers, log,
    process::{Runner, id::Id},
};

use chrono;
use colored::Colorize;
use global_placeholders::global;
use macros_rs::{crashln, fmtstr, string};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use std::{collections::BTreeMap, fs};

pub fn from(address: &str, token: Option<&str>) -> Result<Runner, anyhow::Error> {
    let client = Client::new();
    let mut headers = HeaderMap::new();

    if let Some(token) = token {
        headers.insert(
            "token",
            HeaderValue::from_static(Box::leak(Box::from(token))),
        );
    }

    let response = client
        .get(fmtstr!("{address}/daemon/dump"))
        .headers(headers)
        .send()?;
    let bytes = response.bytes()?;

    Ok(file::from_object(&bytes))
}

pub fn read() -> Runner {
    if !Exists::check(&global!("opm.dump")).file() {
        let runner = Runner {
            id: Id::new(0),
            list: BTreeMap::new(),
            remote: None,
        };

        write(&runner);
        log!("created dump file");
        return runner;
    }

    // Try to read the dump file with error recovery
    match file::try_read_object(global!("opm.dump")) {
        Ok(runner) => runner,
        Err(err) => {
            // If parsing fails, the dump file is likely corrupted
            // Log the error and create a fresh dump file
            log!("[dump::read] Corrupted dump file detected: {err}");
            println!(
                "{} Dump file is corrupted. Creating a fresh dump file...",
                *helpers::FAIL
            );
            
            // Backup the corrupted file for debugging
            let backup_path = format!(
                "{}.corrupted.{}",
                global!("opm.dump"),
                chrono::Utc::now().format("%Y%m%d_%H%M%S")
            );
            if let Err(e) = fs::rename(global!("opm.dump"), &backup_path) {
                log!("[dump::read] Failed to backup corrupted file: {e}");
            } else {
                println!("{} Backed up corrupted file to: {}", *helpers::SUCCESS, backup_path);
            }
            
            // Create a fresh runner with empty state
            let runner = Runner {
                id: Id::new(0),
                list: BTreeMap::new(),
                remote: None,
            };
            
            write(&runner);
            log!("created fresh dump file after corruption");
            println!("{} Created fresh dump file", *helpers::SUCCESS);
            
            runner
        }
    }
}

pub fn raw() -> Vec<u8> {
    if !Exists::check(&global!("opm.dump")).file() {
        let runner = Runner {
            id: Id::new(0),
            list: BTreeMap::new(),
            remote: None,
        };

        write(&runner);
        log!("created dump file");
    }

    file::raw(global!("opm.dump"))
}

pub fn write(dump: &Runner) {
    let encoded = match ron::ser::to_string(&dump) {
        Ok(contents) => contents,
        Err(err) => crashln!(
            "{} Cannot encode dump.\n{}",
            *helpers::FAIL,
            string!(err).white()
        ),
    };

    if let Err(err) = fs::write(global!("opm.dump"), encoded) {
        crashln!(
            "{} Error writing dumpfile.\n{}",
            *helpers::FAIL,
            string!(err).white()
        )
    }
}
