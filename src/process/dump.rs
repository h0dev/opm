use crate::{
    file::{self, Exists},
    helpers, log,
    process::{Runner, id::Id},
};

use chrono::Utc;
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

            // Backup the corrupted file for debugging
            let backup_path = format!(
                "{}.corrupted.{}",
                global!("opm.dump"),
                Utc::now().format("%Y%m%d_%H%M%S")
            );

            // Try rename first (fast for same filesystem), fall back to copy+remove for cross-filesystem
            let backup_result = fs::rename(global!("opm.dump"), &backup_path).or_else(|_| {
                fs::copy(global!("opm.dump"), &backup_path)
                    .and_then(|_| fs::remove_file(global!("opm.dump")))
            });

            if let Err(e) = backup_result {
                log!("[dump::read] Failed to backup corrupted file: {e}");
            } else {
                log!("[dump::read] Backed up corrupted file to: {backup_path}");
            }

            // Create a fresh runner with empty state
            let runner = Runner {
                id: Id::new(0),
                list: BTreeMap::new(),
                remote: None,
            };

            write(&runner);
            log!("[dump::read] Created fresh dump file after corruption");

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

/// Read from temporary dump file
pub fn read_temp() -> Runner {
    if !Exists::check(&global!("opm.dump.temp")).file() {
        return Runner {
            id: Id::new(0),
            list: BTreeMap::new(),
            remote: None,
        };
    }

    match file::try_read_object(global!("opm.dump.temp")) {
        Ok(runner) => runner,
        Err(err) => {
            log!("[dump::read_temp] Failed to read temp dump: {err}");
            // Return empty runner on error
            Runner {
                id: Id::new(0),
                list: BTreeMap::new(),
                remote: None,
            }
        }
    }
}

/// Write to temporary dump file
pub fn write_temp(dump: &Runner) {
    let encoded = match ron::ser::to_string(&dump) {
        Ok(contents) => contents,
        Err(err) => {
            log!("[dump::write_temp] Cannot encode temp dump: {err}");
            return;
        }
    };

    if let Err(err) = fs::write(global!("opm.dump.temp"), encoded) {
        log!("[dump::write_temp] Error writing temp dumpfile: {err}");
    }
}

/// Merge temporary dump into permanent and clear temporary
pub fn commit_temp() {
    let mut permanent = read();
    let temporary = read_temp();
    
    // Merge temporary processes into permanent
    for (id, process) in temporary.list {
        permanent.list.insert(id, process);
    }
    
    // Update ID counter to maximum
    use std::sync::atomic::Ordering;
    let temp_counter = temporary.id.counter.load(Ordering::SeqCst);
    let perm_counter = permanent.id.counter.load(Ordering::SeqCst);
    if temp_counter > perm_counter {
        permanent.id.counter.store(temp_counter, Ordering::SeqCst);
    }
    
    // Write merged state to permanent
    write(&permanent);
    
    // Clear temporary dump
    let _ = fs::remove_file(global!("opm.dump.temp"));
    log!("[dump::commit_temp] Committed temporary processes to permanent storage");
}

/// Read merged state (permanent + temporary)
pub fn read_merged() -> Runner {
    let mut permanent = read();
    let temporary = read_temp();
    
    // Merge temporary processes into permanent
    for (id, process) in temporary.list {
        permanent.list.insert(id, process);
    }
    
    // Use maximum ID counter
    use std::sync::atomic::Ordering;
    let temp_counter = temporary.id.counter.load(Ordering::SeqCst);
    let perm_counter = permanent.id.counter.load(Ordering::SeqCst);
    if temp_counter > perm_counter {
        permanent.id.counter.store(temp_counter, Ordering::SeqCst);
    }
    
    permanent
}

