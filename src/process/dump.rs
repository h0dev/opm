//! Process dump management module
//!
//! This module manages process state persistence using a hybrid approach:
//! - **RAM-based cache (MEMORY_CACHE)**: Fast, in-memory storage for transient process state during normal operations
//! - **Permanent file (process.dump)**: Disk-based storage for persistent state across daemon restarts
//!
//! ## Architecture
//!
//! The system maintains two layers of storage:
//!
//! 1. **Memory Cache**: An in-memory cache that stores all process state changes during daemon operation.
//!    This eliminates the need for frequent disk I/O and provides better performance.
//!
//! 2. **Permanent Storage**: A disk-based dump file that persists process state across daemon restarts.
//!    This is only written when explicitly requested (e.g., via `opm save` command or daemon shutdown).
//!
//! ## Key Functions
//!
//! - `read_memory()`: Read current state from RAM cache
//! - `write_memory()`: Write current state to RAM cache  
//! - `clear_memory()`: Clear the RAM cache
//! - `commit_memory()`: Merge RAM cache into permanent storage and clear cache
//! - `read_merged()`: Read combined state from permanent storage + RAM cache
//! - `init_on_startup()`: Initialize daemon state on startup, handling migration from old temp files
//!
//! ## Migration from Temporary Files
//!
//! Previous versions used a temporary file (`process.temp.dump`) for transient state.
//! The new RAM-based approach provides:
//! - **Better performance**: No disk I/O on every operation
//! - **Simplified architecture**: Single in-memory cache instead of file-based temp storage
//! - **Backward compatibility**: `init_on_startup()` migrates old temp files automatically

use crate::{
    file::{self, Exists},
    helpers, log,
    process::{id::Id, Runner},
};

use chrono::Utc;
use colored::Colorize;
use global_placeholders::global;
use macros_rs::{crashln, fmtstr, string};
use once_cell::sync::Lazy;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use std::sync::atomic::Ordering;
use std::{collections::BTreeMap, fs, sync::Mutex};

/// Global in-memory cache for process state (replaces temporary file)
/// This stores the transient process state in RAM instead of writing to disk
static MEMORY_CACHE: Lazy<Mutex<Option<Runner>>> = Lazy::new(|| Mutex::new(None));

/// Helper function to create an empty Runner
fn empty_runner() -> Runner {
    Runner {
        id: Id::new(0),
        list: BTreeMap::new(),
        remote: None,
    }
}

/// Helper function to read permanent dump with fallback to empty runner
fn read_permanent_dump() -> Runner {
    if !Exists::check(&global!("opm.dump")).file() {
        let runner = empty_runner();
        write(&runner);
        log!("created dump file");
        return runner;
    }

    match file::try_read_object(global!("opm.dump")) {
        Ok(runner) => runner,
        Err(err) => {
            log!("[dump] Failed to read permanent dump: {err}");
            let runner = empty_runner();
            write(&runner);
            runner
        }
    }
}

/// Helper function to merge memory cache into permanent dump
fn merge_runners(mut permanent: Runner, memory: Runner) -> Runner {
    use std::sync::atomic::Ordering;
    
    // Merge memory processes into permanent
    for (id, process) in memory.list {
        permanent.list.insert(id, process);
    }
    
    // Use maximum ID counter
    let mem_counter = memory.id.counter.load(Ordering::SeqCst);
    let perm_counter = permanent.id.counter.load(Ordering::SeqCst);
    if mem_counter > perm_counter {
        permanent.id.counter.store(mem_counter, Ordering::SeqCst);
    }
    
    permanent
}

/// Public version of merge_runners for socket server
pub fn merge_runners_public(permanent: Runner, memory: Runner) -> Runner {
    merge_runners(permanent, memory)
}

/// Public version for socket server to avoid recursion
pub fn read_permanent_direct() -> Runner {
    read_permanent_dump()
}

/// Public version for socket server to avoid recursion
pub fn read_memory_direct() -> Runner {
    let cache = MEMORY_CACHE.lock().unwrap();
    match &*cache {
        Some(runner) => runner.clone(),
        None => empty_runner(),
    }
}

/// Public version for socket server to avoid recursion
pub fn write_memory_direct(dump: &Runner) {
    let mut cache = MEMORY_CACHE.lock().unwrap();
    *cache = Some(dump.clone());
    log!("[dump::write_memory_direct] Updated in-memory process cache");
}

/// Public version for socket server to avoid recursion
pub fn commit_memory_direct() {
    // Read permanent dump directly
    let permanent = read_permanent_dump();
    let memory = read_memory_direct();

    // Merge memory processes into permanent
    let merged = merge_runners(permanent, memory);

    // Write merged state to permanent
    write(&merged);

    // Clear memory cache
    clear_memory();
    log!("[dump::commit_memory_direct] Committed memory cache to permanent storage");
}

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
        let runner = empty_runner();
        write(&runner);
        log!("created dump file");
    }

    file::raw(global!("opm.dump"))
}

pub fn write(dump: &Runner) {
    let dump_path = global!("opm.dump");
    
    // Create backup of existing dump file before writing new one
    if Exists::check(&dump_path).file() {
        let backup_path = format!("{}.bak", dump_path);
        if let Err(e) = fs::copy(&dump_path, &backup_path) {
            log!("[dump::write] Failed to create backup: {}", e);
        } else {
            log!("[dump::write] Created backup at {}", backup_path);
        }
    }
    
    let encoded = match ron::ser::to_string(&dump) {
        Ok(contents) => contents,
        Err(err) => crashln!(
            "{} Cannot encode dump.\n{}",
            *helpers::FAIL,
            string!(err).white()
        ),
    };

    if let Err(err) = fs::write(&dump_path, encoded) {
        crashln!(
            "{} Error writing dumpfile.\n{}",
            *helpers::FAIL,
            string!(err).white()
        )
    }
}

/// Read from memory cache (replaces read_temp)
pub fn read_memory() -> Runner {
    let cache = MEMORY_CACHE.lock().unwrap();
    match &*cache {
        Some(runner) => runner.clone(),
        None => empty_runner(),
    }
}

/// Write to memory cache (replaces write_temp)
/// If daemon is running, sends state via socket. Otherwise, writes to memory cache.
pub fn write_memory(dump: &Runner) {
    use global_placeholders::global;

    // Try to send to daemon via socket first
    let socket_path = global!("opm.socket");
    match crate::socket::send_request(
        &socket_path,
        crate::socket::SocketRequest::SetState(dump.clone()),
    ) {
        Ok(crate::socket::SocketResponse::Success) => {
            log!("[dump::write_memory] Updated state in daemon via socket");
            return;
        }
        Ok(_) => {
            log!("[dump::write_memory] Unexpected response from daemon socket, falling back to memory");
        }
        Err(_) => {
            // Daemon not running, fall back to in-memory cache
            log!("[dump::write_memory] Daemon not running, using in-memory cache");
        }
    }

    // Fallback: Write to local memory cache
    let mut cache = MEMORY_CACHE.lock().unwrap();
    *cache = Some(dump.clone());
    log!("[dump::write_memory] Updated in-memory process cache");
}

/// Clear memory cache
pub fn clear_memory() {
    let mut cache = MEMORY_CACHE.lock().unwrap();
    *cache = None;
    log!("[dump::clear_memory] Cleared in-memory process cache");
}

/// Merge memory cache into permanent and clear memory (replaces commit_temp)
/// If daemon is running, sends SavePermanent command via socket. Otherwise, commits locally.
pub fn commit_memory() {
    use global_placeholders::global;

    // Try to commit via daemon socket first
    let socket_path = global!("opm.socket");
    match crate::socket::send_request(&socket_path, crate::socket::SocketRequest::SavePermanent) {
        Ok(crate::socket::SocketResponse::Success) => {
            log!("[dump::commit_memory] Committed memory to permanent storage via daemon");
            return;
        }
        Ok(_) => {
            log!("[dump::commit_memory] Unexpected response from daemon socket, falling back to local commit");
        }
        Err(_) => {
            // Daemon not running, fall back to local commit
            log!("[dump::commit_memory] Daemon not running, committing locally");
        }
    }

    // Fallback: Local commit
    let permanent = read_permanent_dump();
    let memory = read_memory();

    // Merge memory processes into permanent
    let merged = merge_runners(permanent, memory);

    // Write merged state to permanent
    write(&merged);

    // Clear memory cache
    clear_memory();
    log!("[dump::commit_memory] Committed memory cache to permanent storage");
}

/// Read merged state (permanent + memory) - replaces read_merged
/// If daemon is running, queries state via socket. Otherwise, reads from disk.
pub fn read_merged() -> Runner {
    use global_placeholders::global;

    // Try to read from daemon via socket first
    let socket_path = global!("opm.socket");
    match crate::socket::send_request(&socket_path, crate::socket::SocketRequest::GetState) {
        Ok(crate::socket::SocketResponse::State(runner)) => {
            log!("[dump::read_merged] Retrieved state from daemon via socket");
            return runner;
        }
        Ok(_) => {
            log!(
                "[dump::read_merged] Unexpected response from daemon socket, falling back to file"
            );
        }
        Err(_) => {
            // Daemon not running, fall back to file-based read
            log!("[dump::read_merged] Daemon not running, reading from disk");
        }
    }

    // Fallback: Read permanent dump directly without triggering recursive operations
    let permanent = read_permanent_dump();

    // Read memory cache if it exists
    let memory = read_memory();

    // Merge and return
    merge_runners(permanent, memory)
}

/// Initialize on daemon startup: merge any old temp file into permanent, clean temp, clear memory
pub fn init_on_startup() -> Runner {
    // Read permanent dump
    let mut permanent = read_permanent_dump();

    // Check if old temp dump file exists from previous version (for migration)
    let temp_dump_path = global!("opm.dump.temp");
    if Exists::check(&temp_dump_path).file() {
        log!(
            "[dump::init_on_startup] Found old temp dump file from previous version, migrating..."
        );

        // Read old temp file
        match file::try_read_object::<Runner>(temp_dump_path.clone()) {
            Ok(temporary) => {
                // Merge temporary processes into permanent
                for (id, process) in temporary.list {
                    permanent.list.insert(id, process);
                }

                // Update ID counter to maximum
                let temp_counter = temporary.id.counter.load(Ordering::SeqCst);
                let perm_counter = permanent.id.counter.load(Ordering::SeqCst);
                if temp_counter > perm_counter {
                    permanent.id.counter.store(temp_counter, Ordering::SeqCst);
                }

                log!("[dump::init_on_startup] Merged old temp dump file");
            }
            Err(err) => {
                log!("[dump::init_on_startup] Failed to read old temp dump: {err}");
            }
        }

        // Delete old temp file after migration
        let _ = fs::remove_file(&temp_dump_path);
        log!("[dump::init_on_startup] Cleaned up old temp dump file");
    }

    // Clear memory cache to start fresh
    clear_memory();
    log!("[dump::init_on_startup] Cleared memory cache for fresh daemon start");

    // Note: We preserve crash.crashed flag so restore command can identify crashed processes
    // The daemon will mark crashed processes as stopped (running=false) when it detects they're dead
    // but we keep crash.crashed=true so users can restore them with 'opm restore'

    permanent
}

/// Restore dump from backup file
pub fn restore_from_backup() -> Result<(), String> {
    let dump_path = global!("opm.dump");
    let backup_path = format!("{}.bak", dump_path);
    
    // Check if backup exists
    if !Exists::check(&backup_path).file() {
        return Err("No backup file found. Create a backup first by saving processes.".to_string());
    }
    
    // Try to read the backup file to validate it
    match file::try_read_object::<Runner>(backup_path.clone()) {
        Ok(backup_runner) => {
            // Backup is valid, restore it
            write(&backup_runner);
            log!("[dump::restore_from_backup] Restored from backup: {}", backup_path);
            Ok(())
        }
        Err(e) => {
            Err(format!("Backup file is corrupted or invalid: {}", e))
        }
    }
}

/// Check if backup file exists
pub fn has_backup() -> bool {
    let backup_path = format!("{}.bak", global!("opm.dump"));
    Exists::check(&backup_path).file()
}
