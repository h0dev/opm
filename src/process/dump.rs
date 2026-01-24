//! Process dump management module
//!
//! This module manages process state persistence using a hybrid approach:
//! - **RAM-based cache (MEMORY_CACHE)**: Fast, in-memory storage for transient process state during daemon operations
//! - **Temporary file (process.temp.dump)**: File-based storage for CLI command persistence across invocations
//! - **Permanent file (process.dump)**: Disk-based storage for persistent state across restarts
//!
//! ## Architecture
//!
//! The system maintains three layers of storage:
//!
//! 1. **Memory Cache**: An in-memory cache that stores all process state changes during daemon operation.
//!    This eliminates the need for frequent disk I/O and provides better performance for daemon mode.
//!
//! 2. **Temporary File**: A file-based temporary storage for CLI commands that need persistence across
//!    multiple invocations without committing to permanent storage. This allows users to create processes
//!    temporarily without making them permanent.
//!
//! 3. **Permanent Storage**: A disk-based dump file that persists process state permanently.
//!    This is written when explicitly requested (e.g., via `opm save` command or daemon shutdown).
//!
//! ## Key Functions
//!
//! - `read_memory()`: Read current state from RAM cache (for daemon)
//! - `write_memory()`: Write current state to RAM cache (for daemon)
//! - `read_temp()`: Read current state from temp file (for CLI)
//! - `write_temp()`: Write current state to temp file (for CLI)
//! - `clear_memory()`: Clear the RAM cache
//! - `commit_memory()`: Merge RAM cache into permanent storage and clear cache
//! - `commit_temp()`: Merge temp file into permanent storage and delete temp file
//! - `read_merged()`: Read combined state from permanent storage + temp file + RAM cache
//! - `init_on_startup()`: Initialize daemon state on startup, merging temp files into permanent
//!
//! ## Usage Pattern
//!
//! - **CLI commands** use `write_temp()` for temporary persistence across invocations
//! - **Daemon operations** use `write_memory()` for fast in-memory operations
//! - **Explicit save** (`opm save`) uses `commit_temp()` or `commit_memory()` to make permanent
//! - **Daemon startup** (`opm daemon restore`) merges temp into permanent via `init_on_startup()`

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
use std::{collections::BTreeMap, fs, sync::Mutex};
use once_cell::sync::Lazy;

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

/// Read from memory cache (replaces read_temp)
pub fn read_memory() -> Runner {
    let cache = MEMORY_CACHE.lock().unwrap();
    match &*cache {
        Some(runner) => runner.clone(),
        None => empty_runner(),
    }
}

/// Write to memory cache (replaces write_temp)
pub fn write_memory(dump: &Runner) {
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

/// Read from temporary file (for CLI commands that don't have access to daemon's memory)
pub fn read_temp() -> Runner {
    let temp_path = global!("opm.dump.temp");
    if !Exists::check(&temp_path).file() {
        return empty_runner();
    }
    
    match file::try_read_object(temp_path) {
        Ok(runner) => runner,
        Err(err) => {
            log!("[dump::read_temp] Failed to read temp dump: {err}");
            empty_runner()
        }
    }
}

/// Write to temporary file (for CLI commands that need persistence but not permanent storage)
pub fn write_temp(dump: &Runner) {
    let temp_path = global!("opm.dump.temp");
    let encoded = match ron::ser::to_string(&dump) {
        Ok(contents) => contents,
        Err(err) => {
            log!("[dump::write_temp] Cannot encode temp dump: {err}");
            return;
        }
    };
    
    if let Err(err) = fs::write(&temp_path, encoded) {
        log!("[dump::write_temp] Error writing temp dump file: {err}");
    } else {
        log!("[dump::write_temp] Wrote process state to temporary file");
    }
}

/// Merge memory cache into permanent and clear memory (replaces commit_temp)
pub fn commit_memory() {
    // Read permanent dump directly
    let mut permanent = read_permanent_dump();
    let memory = read_memory();
    
    // Merge memory processes into permanent
    for (id, process) in memory.list {
        permanent.list.insert(id, process);
    }
    
    // Update ID counter to maximum
    use std::sync::atomic::Ordering;
    let mem_counter = memory.id.counter.load(Ordering::SeqCst);
    let perm_counter = permanent.id.counter.load(Ordering::SeqCst);
    if mem_counter > perm_counter {
        permanent.id.counter.store(mem_counter, Ordering::SeqCst);
    }
    
    // Write merged state to permanent
    write(&permanent);
    
    // Clear memory cache
    clear_memory();
    log!("[dump::commit_memory] Committed memory cache to permanent storage");
}

/// Merge temp file into permanent and clear temp file
pub fn commit_temp() {
    // Read permanent dump directly
    let mut permanent = read_permanent_dump();
    let temp = read_temp();
    
    // Merge temp processes into permanent
    for (id, process) in temp.list {
        permanent.list.insert(id, process);
    }
    
    // Update ID counter to maximum
    use std::sync::atomic::Ordering;
    let temp_counter = temp.id.counter.load(Ordering::SeqCst);
    let perm_counter = permanent.id.counter.load(Ordering::SeqCst);
    if temp_counter > perm_counter {
        permanent.id.counter.store(temp_counter, Ordering::SeqCst);
    }
    
    // Write merged state to permanent
    write(&permanent);
    
    // Clear temp file
    let temp_path = global!("opm.dump.temp");
    if Exists::check(&temp_path).file() {
        let _ = fs::remove_file(&temp_path);
        log!("[dump::commit_temp] Committed temp file to permanent storage and removed temp file");
    }
}

/// Read merged state (permanent + memory + temp file) - for CLI commands
pub fn read_merged() -> Runner {
    // Read permanent dump directly without triggering recursive operations
    let mut permanent = read_permanent_dump();
    
    // Read temp file if it exists (for CLI command persistence)
    let temp = read_temp();
    
    // Merge temp file processes into permanent
    for (id, process) in temp.list {
        permanent.list.insert(id, process);
    }
    
    // Read memory cache if it exists (for daemon operations)
    let memory = read_memory();
    
    // Merge memory processes into permanent (memory takes precedence over temp)
    for (id, process) in memory.list {
        permanent.list.insert(id, process);
    }
    
    // Use maximum ID counter from all sources
    use std::sync::atomic::Ordering;
    let temp_counter = temp.id.counter.load(Ordering::SeqCst);
    let mem_counter = memory.id.counter.load(Ordering::SeqCst);
    let perm_counter = permanent.id.counter.load(Ordering::SeqCst);
    let max_counter = temp_counter.max(mem_counter).max(perm_counter);
    if max_counter > perm_counter {
        permanent.id.counter.store(max_counter, Ordering::SeqCst);
    }
    
    permanent
}

/// Initialize on daemon startup: merge temp file into daemon memory (do NOT delete temp file or commit to permanent)
pub fn init_on_startup() -> Runner {
    // Read permanent dump
    let mut permanent = read_permanent_dump();
    
    // Check if temp dump file exists (from CLI commands)
    let temp_dump_path = global!("opm.dump.temp");
    if Exists::check(&temp_dump_path).file() {
        log!("[dump::init_on_startup] Found temp dump file, loading into daemon memory...");
        
        // Read temp file
        match file::try_read_object::<Runner>(temp_dump_path.clone()) {
            Ok(temporary) => {
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
                
                log!("[dump::init_on_startup] Loaded temp processes into daemon memory");
            }
            Err(err) => {
                log!("[dump::init_on_startup] Failed to read temp dump: {err}");
            }
        }
        
        // DO NOT delete temp file here - leave it for CLI commands to read
        // Temp file will only be deleted when user explicitly runs `opm save`
        log!("[dump::init_on_startup] Kept temp file for CLI persistence");
    }

    // Clear memory cache to start fresh (daemon will use write_memory for its operations)
    clear_memory();
    log!("[dump::init_on_startup] Cleared memory cache for fresh daemon start");

    // Note: We preserve crash.crashed flag so restore command can identify crashed processes
    // The daemon will mark crashed processes as stopped (running=false) when it detects they're dead
    // but we keep crash.crashed=true so users can restore them with 'opm restore'

    permanent
}

