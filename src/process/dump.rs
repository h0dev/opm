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
use std::path::PathBuf;
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
            // Deserialization failed - likely due to structure changes after upgrade
            // Create a timestamped backup to prevent data loss
            let dump_path = global!("opm.dump");
            let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
            let corrupted_backup = format!("{}.corrupted.{}", dump_path, timestamp);

            if let Err(e) = fs::copy(&dump_path, &corrupted_backup) {
                log!("[dump] ERROR: Failed to backup corrupted dump file: {}", e);
            } else {
                log!(
                    "[dump] Corrupted dump file backed up to: {}",
                    corrupted_backup
                );
                println!("{}", format!("\n⚠️  Warning: OPM dump file could not be read (likely due to structure changes after upgrade)").yellow());
                println!(
                    "{}",
                    format!("   Backup created at: {}", corrupted_backup).yellow()
                );
                println!(
                    "{}",
                    format!("   Your old process data is preserved in the backup file.").yellow()
                );
                println!(
                    "{}",
                    format!("   Starting with empty process list. Use 'opm restore' if needed.\n")
                        .yellow()
                );
            }

            log!("[dump] Failed to read permanent dump: {err}");

            // Return empty runner WITHOUT writing to disk
            // This prevents overwriting the backup we just created
            empty_runner()
        }
    }
}

/// Helper function to merge memory cache into permanent dump
/// If memory cache exists (Some), it represents the complete current state and replaces permanent
/// If memory cache is None, permanent state is preserved
fn merge_runners(mut permanent: Runner, memory: Option<Runner>) -> Runner {
    use std::sync::atomic::Ordering;

    // If memory cache exists, it represents the complete current state
    // Replace permanent's list with memory's list to reflect deletions
    if let Some(memory) = memory {
        // IMPORTANT: Memory cache represents the complete authoritative state.
        // When the daemon or CLI writes to memory cache via SetState, it sends
        // the complete runner with all processes. The memory cache is the single
        // source of truth for the current process list during runtime.
        //
        // We do a complete replacement (not a merge) because:
        // 1. Process deletions: If a process is in permanent but not in memory,
        //    it means it was explicitly removed, so we must not keep it.
        // 2. State authority: Memory cache is updated by daemon's monitoring loop
        //    and reflects the current runtime state (PIDs, crash counters, etc.)
        // 3. Consistency: A proper merge would require complex conflict resolution
        //    and could lead to inconsistencies (e.g., keeping a deleted process).
        //
        // This replacement is safe because:
        // - SetState handler (socket.rs) already merges incoming state with current memory
        // - So memory cache always contains the complete merged state
        // - GetState simply returns this authoritative state
        permanent.list = memory.list;
        // When memory has state, also update the counter to match memory's counter
        // This ensures deletions properly decrease the counter
        let mem_counter = memory.id.counter.load(Ordering::SeqCst);
        permanent.id.counter.store(mem_counter, Ordering::SeqCst);

        // If both lists are empty, reset counter to 0
        if permanent.list.is_empty() {
            permanent.id.counter.store(0, Ordering::SeqCst);
        }
    }

    permanent
}

/// Public version of merge_runners for socket server
pub fn merge_runners_public(permanent: Runner, memory: Option<Runner>) -> Runner {
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

/// Public version for socket server that returns Option<Runner>
/// Used by merge logic to distinguish between no cache vs empty cache
pub fn read_memory_direct_option() -> Option<Runner> {
    let cache = MEMORY_CACHE.lock().unwrap();
    cache.clone()
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
    let memory = read_memory_direct_option();

    // Merge memory processes into permanent
    let merged = merge_runners(permanent, memory);

    // Write merged state to permanent
    write(&merged);

    // Clear memory cache
    clear_memory();
    log!("[dump::commit_memory_direct] Committed memory cache to permanent storage");
}

/// Read merged state directly without using socket (for use by daemon's own code)
/// This is needed to avoid recursion when the daemon needs to read its own state
/// during the monitoring loop. Unlike read_merged(), this never tries to use the socket.
///
/// ## Behavior
/// - If memory cache exists, returns it (most up-to-date state)
/// - Otherwise, reads permanent dump from disk (for daemon startup or after memory clear)
/// - Creates empty runner if permanent dump is missing or corrupted
/// - All operations use fallback defaults (empty runner) on error to ensure daemon stability
pub fn read_merged_direct() -> Runner {
    // Prefer memory cache if it exists (contains most recent state)
    if let Some(memory) = read_memory_direct_option() {
        return memory;
    }

    // Fallback to permanent dump if memory cache is empty
    // This is critical for daemon startup and restore operations
    read_permanent_dump()
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

    // Atomic write: write to temp file first, then rename
    // This prevents corruption if the write is interrupted (power loss, kill -9, etc.)
    // Use PathBuf for proper cross-platform path handling
    let temp_path = PathBuf::from(&dump_path).with_extension("tmp");

    // Write to temporary file
    if let Err(err) = fs::write(&temp_path, &encoded) {
        crashln!(
            "{} Error writing temporary dumpfile.\n{}",
            *helpers::FAIL,
            string!(err).white()
        )
    }

    // Verify temp file size is non-zero before renaming
    match fs::metadata(&temp_path) {
        Ok(metadata) => {
            if metadata.len() == 0 {
                if let Err(e) = fs::remove_file(&temp_path) {
                    log!("[dump::write] Failed to cleanup empty temp file: {}", e);
                }
                crashln!(
                    "{} Temporary dump file is empty (0 bytes), aborting write to prevent data loss",
                    *helpers::FAIL
                );
            }
        }
        Err(err) => {
            if let Err(e) = fs::remove_file(&temp_path) {
                log!(
                    "[dump::write] Failed to cleanup temp file after metadata error: {}",
                    e
                );
            }
            crashln!(
                "{} Cannot verify temporary dump file.\n{}",
                *helpers::FAIL,
                string!(err).white()
            );
        }
    }

    // Atomically rename temp file to actual dump file
    // On Unix, this is atomic and will never leave the dump file in a partial state
    if let Err(err) = fs::rename(&temp_path, &dump_path) {
        if let Err(e) = fs::remove_file(&temp_path) {
            log!(
                "[dump::write] Failed to cleanup temp file after rename error: {}",
                e
            );
        }
        crashln!(
            "{} Error renaming temporary dumpfile to final location.\n{}",
            *helpers::FAIL,
            string!(err).white()
        )
    }

    log!("[dump::write] Successfully wrote dump file atomically");
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

pub fn load_permanent_into_memory() -> Runner {
    let mut runner = read_permanent_dump();

    // Explicitly reset restart counters for all processes during restore
    // This gives processes a fresh start after system restore/reboot
    // Note: Counters ARE now persisted to dump file, but we reset them during
    // restore to give processes a clean slate after reboot/restore operations
    reset_all_restart_counters(&mut runner);

    log!("[dump::load_permanent_into_memory] Loaded permanent dump, restart counters explicitly reset to 0");

    write_memory_direct(&runner);
    runner
}

/// Reset restart counters for all processes in the runner
/// This is called during restore operations to give processes a fresh start
/// Note: Only resets the restart counter - preserves crashed and errored states
fn reset_all_restart_counters(runner: &mut Runner) {
    for (id, process) in runner.list.iter_mut() {
        process.restarts = 0;
        log!("[dump::reset_all_restart_counters] Reset restart counter for process id={} name={} (preserving state: crashed={}, errored={})", 
             id, &process.name, process.crash.crashed, process.errored);
    }
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
    let memory = read_memory_direct_option();

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

    // Fallback: Read from permanent dump file if memory cache is empty
    // This ensures CLI commands can read process state even when daemon is not running
    read_memory_direct_option().unwrap_or_else(|| read_permanent_dump())
}

/// Read state from daemon only (no disk fallback)
/// This should only be used when daemon is guaranteed to be running (e.g., during restore)
/// Returns error if daemon is not accessible
pub fn read_from_daemon_only() -> Result<Runner, String> {
    use global_placeholders::global;
    use std::thread;
    use std::time::Duration;

    let socket_path = global!("opm.socket");

    // Use more aggressive retry strategy for restore operations
    // The daemon might be busy processing existing processes, so we give it more time
    // We use send_request_once directly to avoid nested retries
    const MAX_RETRIES: u32 = 10;
    const INITIAL_BACKOFF_MS: u64 = 100;

    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        match crate::socket::send_request_once(
            &socket_path,
            &crate::socket::SocketRequest::GetState,
        ) {
            Ok(crate::socket::SocketResponse::State(runner)) => {
                if attempt > 0 {
                    log!(
                        "[dump::read_from_daemon_only] Retrieved state from daemon after {} retries",
                        attempt
                    );
                } else {
                    log!("[dump::read_from_daemon_only] Retrieved state from daemon via socket");
                }
                return Ok(runner);
            }
            Ok(_) => {
                let err = "Unexpected response from daemon socket";
                log!("[dump::read_from_daemon_only] {}", err);
                return Err(err.to_string());
            }
            Err(e) => {
                last_error = Some(e);

                // Don't retry on the last attempt
                if attempt < MAX_RETRIES - 1 {
                    // Exponential backoff with cap: 100ms, 200ms, 400ms, 800ms, then capped at 1000ms
                    // For attempt 0 (first failure): 100 * 2^0 = 100ms
                    // For attempt 1 (second failure): 100 * 2^1 = 200ms
                    // For attempt 2 (third failure): 100 * 2^2 = 400ms, etc.
                    let backoff_ms = (INITIAL_BACKOFF_MS * 2u64.pow(attempt)).min(1000);
                    thread::sleep(Duration::from_millis(backoff_ms));
                    log!(
                        "[dump::read_from_daemon_only] Retry {}/{} after {}ms: {}",
                        attempt + 1,
                        MAX_RETRIES,
                        backoff_ms,
                        last_error.as_ref().unwrap()
                    );
                }
            }
        }
    }

    // All retries exhausted
    let err = format!(
        "Failed to read from daemon after {} retries: {}",
        MAX_RETRIES,
        last_error.expect("last_error should be set after all retries fail")
    );
    log!("[dump::read_from_daemon_only] {}", err);
    Err(err)
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

    // Note: We preserve both the crash.crashed flag and running state
    // so restore command can properly handle processes across daemon restarts.
    // The restart counter is now persisted (serde serialization enabled)
    // to maintain accurate counts across daemon restarts and display correctly in CLI.

    // Populate memory cache with loaded state to keep processes in RAM
    // This ensures the daemon has the process state immediately available in memory
    // and prevents losing temporary state after startup
    // Note: We clone permanent here to satisfy both cache storage and return value.
    // This is a one-time startup operation, so the clone overhead is negligible.
    let mut cache = MEMORY_CACHE.lock().unwrap();
    *cache = Some(permanent.clone());
    log!("[dump::init_on_startup] Populated memory cache with loaded state (restart counters maintained from dump)");

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
            log!(
                "[dump::restore_from_backup] Restored from backup: {}",
                backup_path
            );
            Ok(())
        }
        Err(e) => Err(format!("Backup file is corrupted or invalid: {}", e)),
    }
}

/// Check if backup file exists
pub fn has_backup() -> bool {
    let backup_path = format!("{}.bak", global!("opm.dump"));
    Exists::check(&backup_path).file()
}
