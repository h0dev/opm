#[macro_use]
mod log;
mod api;
mod fork;

use api::{
    DAEMON_CPU_PERCENTAGE, DAEMON_MEM_USAGE, DAEMON_START_TIME, GLOBAL_EVENT_MANAGER,
    GLOBAL_NOTIFICATION_MANAGER,
};
use chrono::{DateTime, Utc};
use colored::Colorize;
use fork::{daemon, Fork};
use global_placeholders::global;
use home;
use macros_rs::{crashln, string, ternary};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use opm::process::{unix::NativeProcess as Process, MemoryInfo};
use serde::Serialize;
use serde_json::json;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{process, thread::sleep, time::Duration};

use opm::{
    config,
    helpers::{self, ColoredString},
    process::{
        dump, get_process_cpu_usage_with_children_from_process, hash, Runner,
        COOLDOWN_LOG_INTERVAL_SECS, PROCESS_CLEANUP_WAIT_MS,
        RESTART_COOLDOWN_SECS,
    },
};

use tabled::{
    settings::{
        object::Columns,
        style::{BorderColor, Style},
        themes::Colorization,
        Color, Rotate,
    },
    Table, Tabled,
};

// Grace period for crash detection is now configurable via daemon.crash_grace_period in config.toml
// Default is 2 seconds to prevent false crash detection when processes are initializing

static ENABLE_API: AtomicBool = AtomicBool::new(false);
static ENABLE_WEBUI: AtomicBool = AtomicBool::new(false);
// Flag to prevent daemon from auto-starting processes during restore operation
// This prevents race condition where daemon restarts processes that restore is already handling
static RESTORE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_termination_signal(_: libc::c_int) {
    // SAFETY: Signal handlers should be kept simple and avoid complex operations.
    // Don't save process state on daemon shutdown - users should explicitly use 'opm save'
    // if they want to persist process state across daemon restarts.
    // This prevents unexpected process.dump writes when daemon is stopped/restarted.

    // Clean up lock file before exiting
    if let Some(home_dir) = home::home_dir() {
        let lock_path = format!("{}/.opm/daemon.lock", home_dir.display());
        let _ = std::fs::remove_file(&lock_path);
    }

    pid::remove();
    log!("[daemon] killed", "pid" => process::id());
    unsafe { libc::_exit(0) }
}

extern "C" fn handle_sigpipe(_: libc::c_int) {
    // Ignore SIGPIPE - this prevents the daemon from crashing when writing to closed stdout/stderr
    // This can happen when the daemon tries to use println!() after being daemonized
}

// Helper function to emit crash event and notification
async fn emit_crash_event_and_notification(id: usize, name: String) {
    // Emit event if EventManager is available
    if let Some(event_manager) = GLOBAL_EVENT_MANAGER.get() {
        let event = opm::events::Event::new(
            opm::events::EventType::ProcessCrash,
            "local".to_string(),
            "Local".to_string(),
            Some(id.to_string()),
            Some(name.clone()),
            format!("Process '{}' crashed", name),
        );
        event_manager.add_event(event).await;
    }

    // Send notification if NotificationManager is available
    if let Some(notification_manager) = GLOBAL_NOTIFICATION_MANAGER.get() {
        notification_manager
            .send(
                opm::notifications::NotificationEvent::ProcessCrash,
                "Process Crashed",
                &format!("Process '{}' has crashed", name),
            )
            .await;
    }
}

/// Reap zombie child processes by calling try_wait() on all process handles
/// This function should be called periodically (e.g., at the start of each monitoring cycle)
/// to prevent zombie process accumulation
fn reap_zombie_processes() {
    let handles_snapshot: Vec<(i64, Arc<Mutex<std::process::Child>>)> =
        opm::process::PROCESS_HANDLES
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();

    for (pid, handle_ref) in handles_snapshot {
        if let Ok(mut child) = handle_ref.try_lock() {
            // Non-blocking check if the child has exited
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Child has exited - remove from handles before dropping the lock
                    // This eliminates any race condition where another thread could access the handle
                    opm::process::PROCESS_HANDLES.remove(&pid);
                    log!("[daemon] reaped zombie/exited process", "pid" => pid, "success" => status.success());
                }
                Ok(None) => {
                    // Child is still running, nothing to do
                }
                Err(e) => {
                    // Error calling try_wait - log but don't remove
                    ::log::warn!("[daemon] error calling try_wait on pid {}: {}", pid, e);
                }
            }
        }
        // If we can't acquire the lock, skip this handle - it might be in use elsewhere
    }
}

/// Extract a search pattern from a command for process adoption
/// Looks for distinctive parts like JAR files, script names, executables
fn extract_search_pattern(command: &str) -> String {
    // Look for patterns that uniquely identify the process
    // Priority: JAR files, then .py/.js/.sh files, then quoted strings, then first word

    // Check for JAR files (e.g., "java -jar Stirling-PDF.jar")
    if let Some(jar_pos) = command.find(".jar") {
        // Find the start of the filename (after last space or slash)
        let before_jar = &command[..jar_pos];
        if let Some(start) = before_jar.rfind(|c: char| c == ' ' || c == '/') {
            let end = (jar_pos + 4).min(command.len());
            if start + 1 < end {
                let jar_name = &command[start + 1..end];
                return jar_name.trim().to_string();
            }
        }
    }

    // Check for common script extensions
    for ext in &[".py", ".js", ".sh", ".rb", ".pl"] {
        if let Some(ext_pos) = command.find(ext) {
            let before_ext = &command[..ext_pos];
            if let Some(start) = before_ext.rfind(|c: char| c == ' ' || c == '/') {
                let end = (ext_pos + ext.len()).min(command.len());
                if start + 1 < end {
                    let script_name = &command[start + 1..end];
                    return script_name.trim().to_string();
                }
            }
        }
    }

    // Fall back to the first word if it looks like an executable
    let first_word = command.split_whitespace().next().unwrap_or("");
    if !first_word.is_empty() && !first_word.starts_with('-') {
        // Skip common shells
        if !matches!(first_word, "sh" | "bash" | "zsh" | "fish" | "dash") {
            return first_word.to_string();
        }
    }

    // If all else fails, return empty (no adoption will occur)
    String::new()
}

fn restart_process() {
    log!("[DAEMON_V2_CHECK] Monitoring cycle initiated", "fingerprint" => "v2_fix");

    // Reap zombie processes at the start of each monitoring cycle
    // This prevents zombie accumulation by calling try_wait() on all child process handles
    reap_zombie_processes();

    // Load daemon config once at the start to avoid repeated I/O operations
    let daemon_config = config::read().daemon;

    // Use a single Runner instance to avoid state synchronization issues
    // Use new_direct() instead of new() to read from memory cache directly
    // without trying to use the socket (which would cause recursion/deadlock)
    let mut runner = Runner::new_direct();
    // Collect IDs first to avoid borrowing issues during iteration
    // Use process_ids() instead of items().keys() to avoid cloning all processes
    let process_ids: Vec<usize> = runner.process_ids().collect();

    for id in process_ids {
        let item = match runner.info(id) {
            Some(item) => item.clone(),
            None => continue, // Process was removed, skip it
        };

        // Check if PID info is missing/incomplete - log error and skip crash detection
        if opm::process::is_pid_info_missing(item.pid, &item.children) {
            ::log::error!("[daemon] process {} ({}) has missing/incomplete PID info (pid={}, children={:?}) - cannot determine crash status",
                item.name, id, item.pid, item.children);
            // DO NOT mark as crashed when PID info is missing
            continue;
        }

        // Treat PID=0 as dead (it's not a valid process PID for managed processes)
        // PID 0 is reserved for the kernel scheduler and should never be assigned to user processes
        let has_valid_pid = item.pid > 0;

        // Check if any descendant is alive (root PID + tracked children)
        let shell_alive = item
            .shell_pid
            .map_or(false, |pid| opm::process::is_pid_alive(pid));

        // Check if session is alive (more robust than individual PID checks)
        // This handles process forking where the main PID exits but children continue running
        let session_alive = item
            .session_id
            .map_or(false, |sid| opm::process::is_session_alive(sid));

        // PM2-STYLE VALIDATION: Check for PID reuse and command mismatch
        // This is the "single source of truth" validation that prevents ghost processes
        // For shell-wrapped processes, validate the shell_pid (the wrapper process)
        // For direct processes, validate the main pid
        let mut validation_failed = false;
        if has_valid_pid {
            let search_pattern = extract_search_pattern(&item.script);
            let expected_pattern = if !search_pattern.is_empty() {
                Some(search_pattern.as_str())
            } else {
                None
            };

            // Validate the shell wrapper PID if present, otherwise validate main PID
            let pid_to_validate = item.shell_pid.unwrap_or(item.pid);
            let (is_valid, current_start_time) = opm::process::validate_process_with_sysinfo(
                pid_to_validate,
                expected_pattern,
                item.process_start_time,
            );

            if !is_valid && item.running {
                validation_failed = true;
                // PID has been reused or command mismatch detected
                ::log::warn!(
                    "[daemon] PID {} validation failed for process {} ({}). PID may have been reused or command mismatch.",
                    pid_to_validate, item.name, id
                );

                // Update start time if process exists but with different start time
                if let Some(new_start_time) = current_start_time {
                    if runner.exists(id) {
                        runner.process(id).process_start_time = Some(new_start_time);
                    }
                }
            }
        }

        // Use enhanced sysinfo-based detection for more robust process tree checking
        // This handles cases where shell wrapper exits but children are still running
        // If validation failed, treat process as dead
        let any_descendant_alive = if validation_failed {
            false
        } else {
            has_valid_pid
                && (opm::process::is_process_or_children_alive_sysinfo(item.pid, &item.children)
                    || shell_alive
                    || session_alive)
        };

        // Check if the main process (PID or shell_pid) is alive
        // This is the primary indicator of process health
        let main_process_alive = if validation_failed {
            false
        } else {
            has_valid_pid && (opm::process::is_pid_alive(item.pid) || shell_alive || session_alive)
        };

        // Even if a PID is alive, check if all tracked children are zombies
        // This handles cases where the wrong PID was adopted but the actual children crashed
        // However, if the main process itself is alive, we should treat it as online
        let all_children_are_zombies = !item.children.is_empty()
            && item.children.iter().all(|&child_pid| {
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    opm::process::unix::is_process_zombie(child_pid as i32)
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    false
                }
            });

        if all_children_are_zombies && !main_process_alive {
            // All tracked children are zombies AND main process is not alive - treat as crashed
            log!("[daemon] all tracked children are zombies and main process not alive, treating as crashed", 
                "name" => &item.name, "id" => id, "children" => format!("{:?}", item.children));
            // Fall through to crash detection logic below (process is treated as dead)
        } else if any_descendant_alive {
            // --- PROCESS IS ALIVE ---
            // Update children list for the next cycle.
            let current_children = opm::process::process_find_children(item.pid);
            // Merge new children with existing ones (keep union of both sets)
            let mut all_children: std::collections::HashSet<i64> =
                item.children.iter().copied().collect();
            for child in &current_children {
                all_children.insert(*child);
            }
            let merged_children: Vec<i64> = all_children.into_iter().collect();

            if merged_children != item.children {
                log!("[daemon] updating children list", "name" => &item.name, "id" => id, "children" => format!("{:?}", merged_children));
                runner.set_children(id, merged_children);
                runner.save_direct();
            }

            // Perform other checks for living processes (memory, watch).
            if item.running && item.max_memory > 0 {
                let pid_for_monitoring = item.shell_pid.unwrap_or(item.pid);
                if let Some(memory_info) =
                    opm::process::get_process_memory_with_children(pid_for_monitoring)
                {
                    if memory_info.rss > item.max_memory {
                        log!("[daemon] memory limit exceeded", "name" => &item.name, "id" => id, "memory" => memory_info.rss, "limit" => item.max_memory);
                        runner.stop(id);
                        continue;
                    }
                }
            }

            if item.running && item.watch.enabled {
                let path = item.path.join(item.watch.path.clone());
                if hash::create(path) != item.watch.hash {
                    log!("[daemon] watch triggered reload", "name" => &item.name, "id" => id);
                    runner.restart(id, false, true);
                    continue;
                }
            }

            // If process is stable, clear the crashed flag.
            if item.running && item.crash.crashed {
                let uptime_secs = (Utc::now() - item.started).num_seconds();
                if uptime_secs >= daemon_config.crash_grace_period as i64 {
                    if runner.exists(id) {
                        runner.process(id).crash.crashed = false;
                        runner.save_direct();
                        log!("[daemon] process stabilized, cleared crashed flag", "name" => &item.name, "id" => id);
                    }
                }
            }
        } else {
            // --- PROCESS IS DEAD (no root, no children alive) ---

            // DISABLED: Process Adoption logic has been removed to fix restart bug
            // The adoption logic was searching ALL system processes and adopting unrelated processes,
            // causing the restart mechanism to attach to wrong PIDs and show incorrect metrics.
            // 
            // Original issue: When a process crashes and needs restart, the daemon would search for
            // any process matching the command pattern and adopt it, even if it was:
            // - Started by a different user
            // - Started by a different process manager
            // - Not related to OPM at all
            // 
            // This caused the bug where instead of restarting the crashed process, OPM would
            // attach to another process and display its uptime, RAM, CPU, and PID.
            //
            // The proper behavior is to always restart crashed processes, not try to adopt
            // unrelated processes. Process forking scenarios are already handled by:
            // 1. Session-based tracking (process.session_id)
            // 2. Process tree monitoring (process.children)
            // 3. Shell wrapper child adoption (in the crash detection section below)
            //
            // If process adoption is needed in the future, it must include strict validation:
            // - Session ID must match (process was spawned by OPM)
            // - Parent process must be OPM or daemon
            // - Start time must be recent (within seconds of expected start)
            // - Command line must match exactly, not just contain pattern

            // Check per-process 5s delay after last action (start/restart/reload/restore)
            // This delay prevents immediate crash detection for newly started processes
            // but should NOT prevent auto-restart of already-crashed processes
            let seconds_since_action = (Utc::now() - item.last_action_at).num_seconds();
            let within_action_delay = seconds_since_action < 5;

            // Child PID adoption logic has been removed due to bugs in PR #306
            // The adoption logic had two critical issues:
            // 1. Could adopt unrelated PIDs that happened to be in the same process group
            // 2. Prevented stopped processes from staying stopped (would restart them instead)
            // When a process dies, we no longer try to adopt its children

            if daemon_config.crash_detection {
                // --- CRASH DETECTION LOGIC ---
                // Only run crash detection if not within action delay
                // This prevents false crash detection for processes that are still starting up
                if !within_action_delay {
                    // Detect new crashes: process has PID but is now dead
                    let grace_period = daemon_config.crash_grace_period as i64;
                    let just_started = (Utc::now() - item.started).num_seconds() < grace_period;
                    let is_new_crash = item.pid > 0;

                    if is_new_crash && !just_started {
                        // Check if this is a manual stop (user-initiated via 'opm stop')
                        // Re-read the latest process state to check the manual_stop flag
                        // (item is a snapshot from the start of the loop, might be stale)
                        let is_manual_stop = runner.exists(id) && runner.process(id).manual_stop;

                        if is_manual_stop {
                            if runner.exists(id) {
                                let process = runner.process(id);
                                process.running = false;
                                process.pid = 0;
                                process.shell_pid = None;
                                process.crash.crashed = false;
                                // Reset manual_stop flag after handling
                                process.manual_stop = false;
                                runner.save_direct();
                                log!("[daemon] process stopped manually (not a crash)", "name" => &item.name, "id" => id);
                            }
                            continue;
                        }

                        let process_handle_pid = item.shell_pid.unwrap_or(item.pid);
                        let mut exited_successfully = false;
                        let mut handle_found = false;

                        if let Some((_, handle_ref)) =
                            opm::process::PROCESS_HANDLES.remove(&process_handle_pid)
                        {
                            handle_found = true;
                            if let Ok(mut child) = handle_ref.lock() {
                                if let Ok(Some(status)) = child.try_wait() {
                                    exited_successfully = status.success();
                                    log!("[daemon] reaped exited process", "name" => &item.name, "id" => id, "success" => exited_successfully);
                                }
                            }
                        }

                        if handle_found && exited_successfully {
                            // Safe PID adoption for shell wrapper scenarios (Stirling-PDF fix)
                            // If the shell wrapper exits cleanly but has exactly ONE child process,
                            // adopt that child as the new primary PID instead of marking as stopped
                            // This handles cases where `sh -c` or `bash -c` creates a transient parent

                            let current_children = opm::process::process_find_children(item.pid);

                            // Only adopt if:
                            // 1. Exactly one child exists (prevents adopting wrong PID)
                            // 2. The child is actually alive
                            // 3. Process is supposed to be running (prevents restarting stopped processes)
                            if current_children.len() == 1 && item.running {
                                let child_pid = current_children[0];
                                if opm::process::is_pid_alive(child_pid) {
                                    // Verify we can access the child process (basic safety check)
                                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                                    {
                                        use opm::process::unix::NativeProcess;
                                        // Try to create process handle - if successful, we can safely adopt
                                        if let Ok(_child_process) =
                                            NativeProcess::new(child_pid as u32)
                                        {
                                            // Validate and capture start time
                                            let (_is_valid, start_time) =
                                                opm::process::validate_process_with_sysinfo(
                                                    child_pid, None, None,
                                                );

                                            // Adopt the child PID
                                            if runner.exists(id) {
                                                let process = runner.process(id);
                                                let old_pid = process.pid;
                                                process.pid = child_pid;
                                                process.shell_pid = None; // Child is now the primary PID
                                                process.process_start_time = start_time; // Store start time
                                                process.is_process_tree = false; // No longer a wrapper
                                                                                 // Add to tracked children for monitoring
                                                if !process.children.contains(&child_pid) {
                                                    process.children.push(child_pid);
                                                }
                                                // Reset failed restart attempts on successful adoption
                                                process.failed_restart_attempts = 0;
                                                runner.save_direct();
                                                log!("[daemon] adopted child PID after shell wrapper exit", 
                                                "name" => &item.name, 
                                                "id" => id, 
                                                "old_pid" => old_pid,
                                                "new_pid" => child_pid);
                                                continue;
                                            }
                                        }
                                    }

                                    // If we're on an unsupported OS, still try to adopt
                                    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                                    {
                                        if runner.exists(id) {
                                            let process = runner.process(id);
                                            let old_pid = process.pid;
                                            process.pid = child_pid;
                                            process.shell_pid = None;
                                            process.is_process_tree = false; // No longer a wrapper
                                            if !process.children.contains(&child_pid) {
                                                process.children.push(child_pid);
                                            }
                                            process.failed_restart_attempts = 0;
                                            runner.save_direct();
                                            log!("[daemon] adopted child PID after shell wrapper exit", 
                                            "name" => &item.name, 
                                            "id" => id, 
                                            "old_pid" => old_pid,
                                            "new_pid" => child_pid);
                                            continue;
                                        }
                                    }
                                }
                            }

                            // No suitable child to adopt - process exited cleanly, mark as stopped
                            if runner.exists(id) {
                                let process = runner.process(id);
                                process.running = false;
                                process.pid = 0;
                                process.shell_pid = None;
                                process.crash.crashed = false;
                                runner.save_direct();
                                log!("[daemon] process stopped cleanly", "name" => &item.name, "id" => id);
                            }
                            continue;
                        }

                        // If handle not found, or it exited with an error, it's a crash.
                        if runner.exists(id) {
                            let process = runner.process(id);
                            process.pid = 0;
                            process.shell_pid = None;
                            process.crash.crashed = true;

                            if item.running {
                                let process_name = item.name.clone();
                                if let Some(handle) = tokio::runtime::Handle::try_current().ok() {
                                    handle
                                        .spawn(emit_crash_event_and_notification(id, process_name));
                                }

                                // Check restart limit using restarts counter
                                // This is the single source of truth displayed in `opm info`
                                if item.restarts >= daemon_config.restarts {
                                    process.running = false;
                                    process.errored = true;
                                    log!("[daemon] process reached max restart limit, setting errored state", "name" => &item.name, "id" => id, "restarts" => item.restarts, "limit" => daemon_config.restarts);
                                } else {
                                    log!("[daemon] process crashed", "name" => &item.name, "id" => id, "restarts" => item.restarts);
                                }
                            }
                            runner.save_direct();
                        }
                    }
                } // End of !within_action_delay check for crash detection

                // --- AUTO-RESTART LOGIC ---
                // Attempt to restart any process that is supposed to be running but is dead.
                // This handles both:
                // 1. Newly detected crashes (from the crash detection logic above)
                // 2. Previously crashed processes that are still dead (pid=0, crashed=true)
                //
                // IMPORTANT: This logic runs even during the action delay period to ensure
                // that already-crashed processes can be restarted without waiting for the delay
                //
                // By placing this logic outside the is_new_crash check, we ensure that
                // processes that failed to restart (e.g., due to bad working directory)
                // will continue to be retried on subsequent daemon cycles.
                log!("[daemon] checking if process needs restart", "id" => id, "name" => &item.name);
                if runner.exists(id) {
                    log!("[daemon] process exists in runner", "id" => id);
                    let updated_process = runner.info(id).cloned();
                    log!("[daemon] got updated process", "id" => id, "has_proc" => updated_process.is_some());
                    if let Some(proc) = updated_process {
                        log!("[daemon] checking if process is running", "id" => id, "running" => proc.running, "restarts" => proc.restarts);
                        if proc.running {
                            // Check restart limit BEFORE attempting restart
                            if proc.restarts >= daemon_config.restarts {
                                // Limit reached - stop the process permanently and set errored state
                                if let Some(process) = runner.list.get_mut(&id) {
                                    process.running = false;
                                    process.errored = true;
                                }
                                runner.save_direct();
                                log!("[daemon] process reached max restart limit, stopping permanently with errored state", 
                                    "name" => &proc.name, "id" => id, "restarts" => proc.restarts, "limit" => daemon_config.restarts);
                            } else {
                                // Simplified restart cooldown mechanism with fixed 2s delay
                                // As per requirement #4, no complex exponential backoff
                                let seconds_since_last_attempt = proc
                                    .last_restart_attempt
                                    .map(|t| (Utc::now() - t).num_seconds())
                                    .unwrap_or(i64::MAX); // Never attempted - allow restart immediately

                                // Fixed 2-second delay for all restart attempts
                                let base_delay = RESTART_COOLDOWN_SECS;

                                // Check if process has exceeded maximum restart attempts
                                const MAX_RESTART_ATTEMPTS: u32 = 5;
                                if proc.failed_restart_attempts >= MAX_RESTART_ATTEMPTS {
                                    // Set to FATAL_ERROR state - stop auto-restart until manual intervention
                                    if let Some(process) = runner.list.get_mut(&id) {
                                        process.running = false;
                                        process.errored = true;
                                    }
                                    runner.save_direct();
                                    log!("[daemon] process reached maximum restart failure limit - FATAL_ERROR state",
                                        "name" => &proc.name,
                                        "id" => id,
                                        "failed_attempts" => proc.failed_restart_attempts,
                                        "max_attempts" => MAX_RESTART_ATTEMPTS);
                                    continue; // Skip restart - requires manual intervention
                                }

                                let within_cooldown =
                                    seconds_since_last_attempt < base_delay as i64;

                                if within_cooldown {
                                    // Process is in cooldown period - skip restart
                                    // Log periodically to reduce noise (but not at 0 to avoid race with restart log)
                                    if seconds_since_last_attempt > 0
                                        && seconds_since_last_attempt % COOLDOWN_LOG_INTERVAL_SECS
                                            == 0
                                    {
                                        log!("[daemon] process in restart cooldown", 
                                            "name" => &proc.name, 
                                            "id" => id, 
                                            "wait_secs" => base_delay - seconds_since_last_attempt as u64);
                                    }
                                } else if runner.is_frozen(id) {
                                    log!("[daemon] process is frozen, skipping restart", "name" => &proc.name, "id" => id);
                                } else {
                                    // Ready to attempt restart
                                    // Only increment the counter if the process has a valid PID (was actually running)
                                    // This prevents double-counting when restart fails and we retry on the next cycle
                                    // - pid > 0: Process was running and crashed, this is a new restart attempt
                                    // - pid = 0: Process failed to start/restart, this is a retry of the same attempt
                                    let is_new_crash = proc.pid > 0;

                                    if is_new_crash {
                                        let new_restart_count = proc.restarts + 1;
                                        if let Some(process) = runner.list.get_mut(&id) {
                                            process.restarts = new_restart_count;
                                        }
                                        log!("[daemon] restarting crashed process", 
                                            "name" => &proc.name, 
                                            "id" => id, 
                                            "restarts" => new_restart_count, 
                                            "cooldown_secs" => base_delay);
                                    } else {
                                        log!("[daemon] retrying failed restart", 
                                            "name" => &proc.name, 
                                            "id" => id, 
                                            "restarts" => proc.restarts, 
                                            "failed_attempts" => proc.failed_restart_attempts,
                                            "cooldown_secs" => base_delay);
                                    }

                                    // Record restart attempt timestamp BEFORE attempting restart
                                    if let Some(process) = runner.list.get_mut(&id) {
                                        process.last_restart_attempt = Some(Utc::now());
                                    }

                                    // Attempt restart
                                    runner.restart(id, true, true);

                                    // Update failed restart counter based on result
                                    // Verify process is actually running by checking PID > 0 AND process is alive
                                    // Also check if this was a quick crash (within 10 seconds) to detect flapping
                                    if let Some(process) = runner.list.get_mut(&id) {
                                        if process.pid > 0
                                            && opm::process::is_pid_alive(process.pid)
                                        {
                                            // Restart succeeded - process has valid PID and is alive
                                            // Check if previous process crashed quickly (within 10s of start)
                                            // This indicates a flapping process that needs exponential backoff
                                            if is_new_crash {
                                                // Use the process's started time which was updated by restart()
                                                // Calculate how long the PREVIOUS instance ran before crashing
                                                let previous_started = item.started;
                                                let crash_time =
                                                    (Utc::now() - previous_started).num_seconds();
                                                if crash_time < 10 {
                                                    // Quick crash detected - this counts as a failed restart
                                                    process.failed_restart_attempts += 1;
                                                    log!("[daemon] quick crash detected ({}s), incrementing failure counter",
                                                        "name" => &proc.name,
                                                        "id" => id,
                                                        "crash_time_secs" => crash_time,
                                                        "failed_attempts" => process.failed_restart_attempts);
                                                } else {
                                                    // Process ran long enough - reset failure counter
                                                    process.failed_restart_attempts = 0;
                                                }
                                            } else {
                                                // Retry of failed restart succeeded - reset counter
                                                process.failed_restart_attempts = 0;
                                            }
                                        } else {
                                            // Restart failed - no valid PID or process is not alive
                                            process.failed_restart_attempts += 1;
                                            log!("[daemon] restart attempt failed", 
                                                "name" => &proc.name, 
                                                "id" => id,
                                                "pid" => process.pid,
                                                "failed_attempts" => process.failed_restart_attempts);
                                        }
                                    }

                                    // Save state after restart to persist PID, counters, timestamps
                                    runner.save_direct();
                                }
                            }
                        }
                    }
                }
            } else {
                log!("[daemon] crash detection disabled - skipping crash handling", "name" => &item.name, "id" => id);
            }
        }
        // Handle processes that need to be started (e.g. after restore or `opm start`)
        // Skip this during restore to prevent race condition where daemon restarts
        // processes that restore is already handling, which would create duplicates
        if item.running && item.pid == 0 && !item.crash.crashed && !is_restore_in_progress() {
            log!("[daemon] starting process with no PID", "name" => &item.name, "id" => id);
            runner.restart(id, true, false); // is_daemon_op=true, increment_counter=false
                                             // Save state after restart to persist PID and state changes
            runner.save_direct();
            continue;
        }
    }
}

pub fn health(format: &String) {
    let mut pid: Option<i32> = None;
    let mut cpu_percent: Option<f64> = None;
    let mut uptime: Option<DateTime<Utc>> = None;
    let mut memory_usage: Option<MemoryInfo> = None;
    let mut runner = Runner::new();
    let mut daemon_running = false;

    #[derive(Clone, Debug, Tabled)]
    struct Info {
        #[tabled(rename = "pid file")]
        pid_file: String,
        #[tabled(rename = "fork path")]
        path: String,
        #[tabled(rename = "cpu percent")]
        cpu_percent: String,
        #[tabled(rename = "memory usage")]
        memory_usage: String,
        #[tabled(rename = "daemon type")]
        external: String,
        #[tabled(rename = "process count")]
        process_count: usize,
        role: String,
        uptime: String,
        pid: String,
        status: ColoredString,
    }

    impl Serialize for Info {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let trimmed_json = json!({
             "pid_file": &self.pid_file.trim(),
             "path": &self.path.trim(),
             "cpu": &self.cpu_percent.trim(),
             "mem": &self.memory_usage.trim(),
             "process_count": &self.process_count.to_string(),
             "role": &self.role,
             "uptime": &self.uptime.trim(),
             "pid": &self.pid.trim(),
             "status": &self.status.0.trim(),
            });

            trimmed_json.serialize(serializer)
        }
    }

    if pid::exists() {
        match pid::read() {
            Ok(process_id) => {
                // Check if the process is actually running before trying to get its information
                if pid::running(process_id.get::<i32>()) {
                    daemon_running = true;
                    // Always set PID and uptime if daemon is running
                    pid = Some(process_id.get::<i32>());
                    uptime = pid::uptime().ok();

                    // Try to get process stats (may fail for detached processes)
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    {
                        if let Ok(process) = Process::new(process_id.get::<u32>()) {
                            memory_usage = process.memory_info().ok().map(MemoryInfo::from);
                            cpu_percent = Some(get_process_cpu_usage_with_children_from_process(
                                &process,
                                process_id.get::<i64>(),
                            ));
                        }
                    }
                } else {
                    // Process is not running, remove stale PID file
                    pid::remove();
                }
            }
            Err(err) => {
                // PID file exists but can't be read (corrupted or invalid)
                log!("[daemon] health check found corrupted PID file, removing", "error" => err);
                pid::remove();
            }
        }
    }

    let cpu_percent = match cpu_percent {
        Some(percent) => format!("{:.2}%", percent),
        None => string!("0.00%"),
    };

    let memory_usage = match memory_usage {
        Some(usage) => helpers::format_memory(usage.rss),
        None => string!("0b"),
    };

    let uptime = match uptime {
        Some(uptime) => helpers::format_duration(uptime),
        None => string!("none"),
    };

    let pid = match pid {
        Some(pid) => string!(pid),
        None => string!("n/a"),
    };

    let data = vec![Info {
        pid: pid,
        cpu_percent,
        memory_usage,
        uptime: uptime,
        path: global!("opm.base"),
        external: global!("opm.daemon.kind"),
        process_count: runner.count(),
        role: config::read().get_role_name().to_string(),
        pid_file: format!("{}  ", global!("opm.pid")),
        status: ColoredString(ternary!(
            daemon_running,
            "online".green().bold(),
            "stopped".red().bold()
        )),
    }];

    let table = Table::new(data.clone())
        .with(Rotate::Left)
        .with(Style::modern().remove_horizontals())
        .with(Colorization::exact([Color::FG_CYAN], Columns::first()))
        .with(BorderColor::filled(Color::FG_BRIGHT_BLACK))
        .to_string();

    if let Ok(json) = serde_json::to_string(&data[0]) {
        match format.as_str() {
            "raw" => println!("{:?}", data[0]),
            "json" => println!("{json}"),
            "default" => {
                println!(
                    "{}\n{table}\n",
                    format!("OPM daemon information").on_bright_white().black()
                );
                println!(
                    " {}",
                    format!("Use `opm daemon restart` to restart the daemon").white()
                );
                println!(
                    " {}",
                    format!("Use `opm daemon reset` to clean process id values").white()
                );
            }
            _ => {}
        };
    };
}

pub fn stop(verbose: bool) {
    if pid::exists() {
        if verbose {
            println!("{} Stopping OPM daemon", *helpers::SUCCESS);
        }

        match pid::read() {
            Ok(pid) => {
                if let Err(err) = opm::process::process_stop(pid.get()) {
                    log!("[daemon] failed to stop", "error" => err);
                }
                pid::remove();
                log!("[daemon] stopped", "pid" => pid);
                if verbose {
                    println!("{} OPM daemon stopped", *helpers::SUCCESS);
                }
            }
            Err(err) => {
                // PID file exists but can't be read (corrupted or invalid)
                log!("[daemon] removing corrupted PID file", "error" => err);
                if verbose {
                    println!("{} PID file is corrupted, removing it", *helpers::SUCCESS);
                }
                pid::remove();
                if verbose {
                    println!("{} OPM daemon stopped", *helpers::SUCCESS);
                }
            }
        }
    } else if verbose {
        crashln!("{} The daemon is not running", *helpers::FAIL)
    }
}

pub fn start(verbose: bool) {
    if verbose {
        println!(
            "{} Spawning OPM daemon (opm_base={})",
            *helpers::SUCCESS,
            global!("opm.base")
        );
    }

    if pid::exists() {
        match pid::read() {
            Ok(pid) => {
                if pid::running(pid.get()) {
                    // Daemon is actually running
                    crashln!("{} The daemon is already running", *helpers::FAIL);
                } else {
                    // Stale PID file - process not running
                    log!("[daemon] removing stale PID file", "pid" => pid.get::<i32>());
                    pid::remove();
                }
            }
            Err(err) => {
                // PID file exists but can't be read (corrupted or invalid)
                log!("[daemon] removing corrupted PID file", "error" => err);
                println!("{} Removing corrupted PID file", *helpers::SUCCESS);
                pid::remove();
            }
        }
    }

    // FEATURE: Atomic lock file to prevent concurrent daemon starts
    // Check for lock file and ensure no other daemon is starting
    if let Some(home_dir) = home::home_dir() {
        let lock_path = format!("{}/.opm/daemon.lock", home_dir.display());
        
        // Check if lock file exists and contains a valid PID
        if std::path::Path::new(&lock_path).exists() {
            if let Ok(lock_content) = std::fs::read_to_string(&lock_path) {
                if let Ok(lock_pid) = lock_content.trim().parse::<i32>() {
                    // Check if the process holding the lock is still running
                    if pid::running(lock_pid) {
                        // Another daemon is currently starting or running
                        // Try to kill it to ensure clean state
                        log!("[daemon] found lock file with running process, attempting to kill", "pid" => lock_pid);
                        let _ = nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(lock_pid),
                            nix::sys::signal::Signal::SIGKILL,
                        );
                        std::thread::sleep(std::time::Duration::from_millis(PROCESS_CLEANUP_WAIT_MS));
                    } else {
                        log!("[daemon] removing stale lock file", "pid" => lock_pid);
                    }
                }
            }
            // Remove stale lock file
            let _ = std::fs::remove_file(&lock_path);
        }
        
        // Create lock file with current PID
        let current_pid = std::process::id();
        if let Err(e) = std::fs::write(&lock_path, current_pid.to_string()) {
            log!("[daemon] failed to create lock file", "error" => e);
        } else {
            log!("[daemon] created lock file", "pid" => current_pid);
        }
    }

    #[inline]
    extern "C" fn init() {
        // Wrap initialization in panic::catch_unwind to prevent silent crashes
        let result = std::panic::catch_unwind(|| {
            // Create a tokio runtime for async operations
            let rt = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(err) => {
                    log!("[daemon] Failed to create tokio runtime", "error" => format!("{:?}", err));
                    eprintln!(
                        "[daemon] Fatal error: Failed to create tokio runtime: {:?}",
                        err
                    );
                    panic!("Failed to create tokio runtime: {:?}", err);
                }
            };

            // Enter the runtime context so all async operations work correctly
            let _guard = rt.enter();

            pid::name("OPM Restart Handler Daemon");

            let config = config::read().daemon;
            let api_enabled = ENABLE_API.load(Ordering::Acquire);
            let ui_enabled = ENABLE_WEBUI.load(Ordering::Acquire);

            unsafe {
                libc::signal(
                    libc::SIGTERM,
                    handle_termination_signal as *const () as usize,
                );
                libc::signal(libc::SIGPIPE, handle_sigpipe as *const () as usize);
            };

            DAEMON_START_TIME.set(Utc::now().timestamp_millis() as f64);

            pid::write(process::id());
            log!("[daemon] new fork", "pid" => process::id());
            
            // Clean up old lock file and create new one
            if let Some(home_dir) = home::home_dir() {
                let lock_path = format!("{}/.opm/daemon.lock", home_dir.display());
                let _ = std::fs::remove_file(&lock_path);
                if let Err(e) = std::fs::write(&lock_path, process::id().to_string()) {
                    log!("[daemon] failed to update lock file", "error" => e);
                }
            }

        if api_enabled {
            log!(
                "[daemon] Starting API server",
                "address" => config::read().fmt_address(),
                "webui" => ui_enabled
            );

            // Spawn API server in a separate task using tokio::spawn now that we're in the runtime context
            let api_handle = tokio::spawn(async move { api::start(ui_enabled).await });

            // Wait for the API server to start and bind to the port
            // Use a retry loop with exponential backoff to allow time for Rocket initialization
            let addr = config::read().fmt_address();
            let max_retries = 10;
            let mut retry_count = 0;
            let mut is_listening = false;

            while retry_count < max_retries {
                // Wait before checking - start with 300ms and increase
                let wait_ms = 300 + (retry_count * 200);
                std::thread::sleep(std::time::Duration::from_millis(wait_ms));

                // Try to connect to the API server using synchronous TCP connection
                if std::net::TcpStream::connect(&addr).is_ok() {
                    is_listening = true;
                    break;
                }

                // Check if the task has already failed - if so, no point retrying
                if api_handle.is_finished() {
                    log!("[daemon] API server task has terminated", "status" => "unexpected", "retry" => retry_count);
                    break;
                }

                retry_count += 1;
            }

            if is_listening {
                log!(
                    "[daemon] API server successfully started",
                    "address" => addr,
                    "webui" => ui_enabled,
                    "retries" => retry_count
                );
            } else {
                log!(
                    "[daemon] API server may have failed to start",
                    "address" => addr,
                    "status" => "check logs and port availability",
                    "retries" => retry_count
                );
            }
        }

        // Do not load permanent dump on daemon startup.
        // Permanent dump should be loaded into memory only during `opm restore`.
        opm::process::dump::clear_memory();

        // Clean up all stale timestamp files from previous daemon sessions
        // This prevents old timestamps from interfering with crash detection
        cleanup_all_timestamp_files();

        // Start Unix socket server for CLI-daemon communication
        // Socket server must be started AFTER init_on_startup() to ensure memory cache is ready
        // Use a channel to synchronize socket server readiness
        let socket_path = global!("opm.socket").to_string();
        let socket_path_clone = socket_path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();

        match std::thread::Builder::new()
            .name("socket-server".to_string())
            .spawn(move || {
                // Use start_socket_server_with_callback to signal when ready
                if let Err(e) = opm::socket::start_socket_server_with_callback(&socket_path, Some(move || {
                    // Signal that socket server is ready to accept connections
                    if let Err(e) = ready_tx.send(()) {
                        log!("[daemon] Failed to send socket readiness signal", "error" => format!("{}", e));
                    }
                })) {
                    log!("[daemon] Unix socket server error", "error" => format!("{}", e));
                    eprintln!("[daemon] Critical: Unix socket server failed to start: {}", e);
                }
            })
        {
            Ok(_) => {
                // Wait for socket server to be ready with a timeout
                // This ensures the socket is fully initialized before the daemon continues
                match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    Ok(_) => {
                        log!("[daemon] Unix socket server ready", "path" => socket_path_clone);
                    }
                    Err(_) => {
                        log!("[daemon] Socket server initialization timeout", "path" => &socket_path_clone, "timeout" => "5s");
                        eprintln!("[daemon] Warning: Socket server failed to initialize within 5 seconds. CLI commands may fail until initialization completes.");
                    }
                }
            }
            Err(e) => {
                log!("[daemon] Failed to spawn socket server thread", "error" => format!("{}", e));
                eprintln!("[daemon] Warning: Socket server could not be started. CLI commands may not work correctly.");
            }
        }

        loop {
            if api_enabled {
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    if let Ok(process_info) = Process::new(process::id()) {
                        let cpu_usage = get_process_cpu_usage_with_children_from_process(
                            &process_info,
                            process::id() as i64,
                        );
                        DAEMON_CPU_PERCENTAGE.observe(cpu_usage);

                        if let Ok(mem_info) = process_info.memory_info() {
                            DAEMON_MEM_USAGE.observe(mem_info.rss() as f64);
                        }
                    }
                }
            }

            // Wrap restart_process in catch_unwind to prevent daemon crashes
            // This is a last-resort safety net - restart_process() has internal error handling,
            // but catch_unwind ensures that even unexpected panics won't crash the daemon.
            // This is placed in the hot loop because:
            // 1. restart_process() doesn't return Result, so we can't use traditional error handling
            // 2. The performance impact is negligible (catch_unwind is lightweight when no panic occurs)
            // 3. Daemon stability is critical - it manages all processes and must not crash
            // If a process monitoring operation fails, we log it and continue
            // This ensures the daemon remains stable even when individual processes fail
            if !Runner::new().is_empty() {
                let result = panic::catch_unwind(|| {
                    restart_process();
                });

                if let Err(err) = result {
                    // Log the panic but don't crash the daemon
                    log!("[daemon] panic in restart_process", "error" => format!("{:?}", err));
                    eprintln!(
                        "[daemon] Warning: process monitoring encountered an error but daemon continues running"
                    );
                }
            }

            sleep(Duration::from_millis(config.interval));
        }
        });

        // Handle panic result from catch_unwind
        match result {
            Ok(_) => {
                // Normal termination (shouldn't reach here in daemon loop)
                ::log::info!("[daemon] init completed normally");
            }
            Err(e) => {
                // Panic occurred during initialization
                ::log::error!("[daemon] FATAL: init panicked: {:?}", e);
                eprintln!("[daemon] FATAL ERROR: Daemon initialization failed: {:?}", e);
                
                // Clean up lock file before exiting
                if let Some(home_dir) = home::home_dir() {
                    let lock_path = format!("{}/.opm/daemon.lock", home_dir.display());
                    let _ = std::fs::remove_file(&lock_path);
                }
                
                pid::remove();
                std::process::exit(1);
            }
        }
    }

    if verbose {
        println!(
            "{} OPM Successfully daemonized (type={})",
            *helpers::SUCCESS,
            global!("opm.daemon.kind")
        );
    }
    // Keep stderr open so we can see Rocket and other errors
    // This allows error messages to be written to the daemon log or terminal
    match daemon(false, true) {
        Ok(Fork::Parent(_)) => {
            // Wait for the daemon child to write its PID file and start running
            // This prevents race conditions where health checks immediately after start show "stopped"
            let max_wait_ms = 2000; // Wait up to 2 seconds
            let poll_interval_ms = 50; // Check every 50ms
            let mut elapsed_ms = 0;

            while elapsed_ms < max_wait_ms {
                if pid::exists() {
                    match pid::read() {
                        Ok(daemon_pid) => {
                            if pid::running(daemon_pid.get()) {
                                // Daemon is running with valid PID
                                log!("[daemon] verified daemon running", "pid" => daemon_pid.get::<i32>());
                                return;
                            }
                        }
                        Err(_) => {
                            // PID file exists but can't be read yet - keep waiting
                        }
                    }
                }
                sleep(Duration::from_millis(poll_interval_ms));
                elapsed_ms += poll_interval_ms;
            }

            // If we reach here, daemon didn't start within the timeout
            // Log a warning but don't crash - the daemon might still be starting
            log!("[daemon] PID file not created within timeout", "max_wait_ms" => max_wait_ms);
            eprintln!(
                "{} Warning: Daemon PID file not detected within {}ms",
                *helpers::WARN,
                max_wait_ms
            );
        }
        Ok(Fork::Child) => init(),
        Err(err) => crashln!("{} Daemon creation failed with code {err}", *helpers::FAIL),
    }
}

pub fn restart(api: &bool, webui: &bool, verbose: bool) {
    if pid::exists() {
        stop(verbose);
    }

    let config = config::read().daemon;

    if config.web.ui || *webui {
        ENABLE_API.store(true, Ordering::Release);
        ENABLE_WEBUI.store(true, Ordering::Release);
    } else if config.web.api || *api {
        ENABLE_API.store(true, Ordering::Release);
    } else {
        ENABLE_API.store(*api, Ordering::Release);
    }

    start(verbose);
}

pub fn reset() {
    let mut runner = Runner::new();

    // Use the compact() function to compress all IDs and fill gaps
    // This ensures IDs are sequential: 0, 1, 2, etc.
    runner.compact();

    // Write directly to permanent storage without merging memory cache
    // dump::write() updates the permanent dump file without auto-save behavior
    // (dump::commit_memory() removed - it merges memory cache and clears it)
    dump::write(&runner);

    log!("[daemon] reset and compressed IDs", "next_id" => runner.id.to_string());
}

pub fn setup() {
    use std::env;
    use std::fs;
    use std::path::Path;

    println!("{} Setting up OPM systemd service...", *helpers::SUCCESS);

    // Get the current user's home directory
    let home_dir = match home::home_dir() {
        Some(dir) => dir,
        None => crashln!("{} Unable to determine home directory", *helpers::FAIL),
    };

    // Get the path to the opm binary
    let opm_binary = match env::current_exe() {
        Ok(path) => path,
        Err(err) => crashln!(
            "{} Unable to determine opm binary path: {}",
            *helpers::FAIL,
            err
        ),
    };

    let opm_binary_str = opm_binary.to_string_lossy();

    // Determine systemd service directory
    // For user services: ~/.config/systemd/user/
    // For system services: /etc/systemd/system/ (requires root)
    let is_root = unsafe { libc::geteuid() == 0 };

    let (service_dir_path, install_target) = if is_root {
        (
            Path::new("/etc/systemd/system").to_path_buf(),
            "multi-user.target",
        )
    } else {
        (home_dir.join(".config/systemd/user"), "default.target")
    };

    let service_dir = service_dir_path.as_path();

    // Create service directory if it doesn't exist
    if !service_dir.exists() {
        if let Err(err) = fs::create_dir_all(service_dir) {
            crashln!(
                "{} Failed to create service directory {:?}: {}",
                *helpers::FAIL,
                service_dir,
                err
            );
        }
    }

    let service_file_path = service_dir.join("opm.service");
    let opm_dir = global!("opm.base");
    let pid_file = global!("opm.pid");

    // Generate service file content
    let service_content = if is_root {
        format!(
            r#"# OPM Daemon systemd service file (system-wide)

[Unit]
Description=OPM Process Manager Daemon
After=network.target

[Service]
Type=forking
WorkingDirectory={}
PIDFile={}
ExecStart={} daemon start
ExecStop={} daemon stop
Restart=on-failure
RestartSec=5s
LimitNOFILE=infinity
LimitNPROC=infinity
LimitCORE=infinity

[Install]
WantedBy={}
"#,
            opm_dir, pid_file, opm_binary_str, opm_binary_str, install_target
        )
    } else {
        format!(
            r#"# OPM Daemon systemd service file (user service)

[Unit]
Description=OPM Process Manager Daemon
After=network.target

[Service]
Type=forking
WorkingDirectory={}
PIDFile={}
ExecStart={} daemon start
ExecStop={} daemon stop
Restart=on-failure
RestartSec=5s

[Install]
WantedBy={}
"#,
            opm_dir, pid_file, opm_binary_str, opm_binary_str, install_target
        )
    };

    // Write service file
    if let Err(err) = fs::write(&service_file_path, service_content) {
        crashln!(
            "{} Failed to write service file to {:?}: {}",
            *helpers::FAIL,
            service_file_path,
            err
        );
    }

    println!(
        "{} Service file created at: {}",
        *helpers::SUCCESS,
        service_file_path.display()
    );

    // Provide instructions for enabling the service
    if is_root {
        println!(
            "\n{} To enable and start the OPM daemon:",
            *helpers::SUCCESS
        );
        println!("  sudo systemctl daemon-reload");
        println!("  sudo systemctl enable opm.service");
        println!("  sudo systemctl start opm.service");
        println!("\n{} To check daemon status:", *helpers::SUCCESS);
        println!("  sudo systemctl status opm.service");
    } else {
        println!(
            "\n{} To enable and start the OPM daemon:",
            *helpers::SUCCESS
        );
        println!("  systemctl --user daemon-reload");
        println!("  systemctl --user enable opm.service");
        println!("  systemctl --user start opm.service");
        println!(
            "\n{} To enable lingering (start daemon at boot):",
            *helpers::SUCCESS
        );
        println!("  loginctl enable-linger $USER");
        println!("\n{} To check daemon status:", *helpers::SUCCESS);
        println!("  systemctl --user status opm.service");
    }

    println!(
        "\n{} Setup complete! The OPM daemon will now start automatically with the system.",
        *helpers::SUCCESS
    );
}

pub mod pid;

// Constants for timestamp file handling
const TIMESTAMP_FILE_PREFIX: &str = "last_action_";
const TIMESTAMP_FILE_SUFFIX: &str = ".timestamp";

/// Helper function to check if a filename matches the timestamp file pattern
#[inline]
fn is_timestamp_file(filename: &str) -> bool {
    filename.starts_with(TIMESTAMP_FILE_PREFIX) && filename.ends_with(TIMESTAMP_FILE_SUFFIX)
}

/// Cleans up all stale timestamp files from the .opm directory.
///
/// Timestamp files (`last_action_*.timestamp`) are created when CLI actions are performed
/// to prevent the daemon from immediately marking processes as crashed during the grace period.
/// However, these files can become stale when the daemon is stopped/restarted, causing
/// false crash detection on subsequent operations.
///
/// This function:
/// - Scans the `~/.opm/` directory for files matching `last_action_*.timestamp`
/// - Removes all matching files to ensure a clean state
/// - Logs warnings for any errors encountered during cleanup
///
/// # When to call
/// This should be called:
/// - On daemon startup (after `dump::init_on_startup()`)
/// - During restore operations (before restoring processes)
///
/// # Errors
/// This function does not return errors. Instead, it logs warnings for any issues
/// encountered and continues with cleanup to ensure best-effort removal of stale files.
pub fn cleanup_all_timestamp_files() {
    let Some(home_dir) = home::home_dir() else {
        ::log::warn!("Cannot cleanup timestamp files: home directory not available. Stale timestamp files may cause false crash detection.");
        return;
    };

    let opm_dir = home_dir.join(".opm");
    let Ok(entries) = std::fs::read_dir(&opm_dir) else {
        // Directory might not exist yet on first run - this is normal and not a concern
        // If it fails for other reasons (permission denied, etc.), we'll discover it
        // when the daemon tries to create files there during normal operation
        return;
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                ::log::warn!(
                    "Failed to read directory entry during timestamp cleanup: {}",
                    e
                );
                continue;
            }
        };

        let path = entry.path();
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Remove all files matching pattern "last_action_*.timestamp"
        if is_timestamp_file(file_name) {
            if let Err(e) = std::fs::remove_file(&path) {
                ::log::warn!("Failed to remove stale timestamp file {:?}: {}", path, e);
            } else {
                ::log::debug!("Cleaned up stale timestamp file: {:?}", path);
            }
        }
    }
}

/// Set restore in progress flag to prevent daemon from auto-starting processes during restore
pub fn set_restore_in_progress() {
    RESTORE_IN_PROGRESS.store(true, Ordering::SeqCst);
    ::log::info!("[daemon] restore in progress flag set");
}

/// Clear restore in progress flag to allow daemon to resume auto-starting processes
pub fn clear_restore_in_progress() {
    RESTORE_IN_PROGRESS.store(false, Ordering::SeqCst);
    ::log::info!("[daemon] restore in progress flag cleared");
}

/// Check if restore is currently in progress
pub fn is_restore_in_progress() -> bool {
    RESTORE_IN_PROGRESS.load(Ordering::SeqCst)
}

// Helper function to check if there was a recent action timestamp file
#[allow(dead_code)]
fn has_recent_action_timestamp(id: usize) -> bool {
    match home::home_dir() {
        Some(home_dir) => {
            let action_file = format!(
                "{}/.opm/{}{}{}",
                home_dir.display(),
                TIMESTAMP_FILE_PREFIX,
                id,
                TIMESTAMP_FILE_SUFFIX
            );
            let path = std::path::Path::new(&action_file);
            if !path.exists() {
                return false;
            }

            // Check if file is less than 5 seconds old
            // Increased from 3 to 5 seconds to give more time for process startup
            // and prevent daemon from interfering during manual start/restart operations
            if let Ok(metadata) = std::fs::metadata(&action_file) {
                if let Ok(modified_time) = metadata.modified() {
                    let now = std::time::SystemTime::now();
                    if let Ok(elapsed) = now.duration_since(modified_time) {
                        return elapsed.as_secs() < 5; // Less than 5 seconds old
                    }
                }
            }
            false
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_restore_in_progress_flag() {
        // Initially should be false
        assert!(!is_restore_in_progress());

        // Set the flag
        set_restore_in_progress();
        assert!(is_restore_in_progress());

        // Clear the flag
        clear_restore_in_progress();
        assert!(!is_restore_in_progress());

        // Can set and clear multiple times
        set_restore_in_progress();
        assert!(is_restore_in_progress());
        clear_restore_in_progress();
        assert!(!is_restore_in_progress());
    }

    #[test]
    fn test_restore_in_progress_flag_concurrent() {
        // Test thread-safety of atomic flag operations under concurrent access
        // This verifies the flag works correctly when multiple threads check/set it simultaneously

        // Start with clean state
        clear_restore_in_progress();
        assert!(!is_restore_in_progress());

        // Use Arc to share state transition counter across threads
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;

        let true_reads = Arc::new(AtomicUsize::new(0));
        let false_reads = Arc::new(AtomicUsize::new(0));

        // Spawn multiple reader threads that will check the flag
        let readers: Vec<_> = (0..10)
            .map(|_| {
                let true_count = Arc::clone(&true_reads);
                let false_count = Arc::clone(&false_reads);
                thread::spawn(move || {
                    // Simulate daemon loop checking if restore is in progress
                    for _ in 0..100 {
                        let in_progress = is_restore_in_progress();
                        if in_progress {
                            true_count.fetch_add(1, Ordering::Relaxed);
                        } else {
                            false_count.fetch_add(1, Ordering::Relaxed);
                        }
                        thread::yield_now();
                    }
                })
            })
            .collect();

        // Spawn writer thread that toggles the flag
        let writer = thread::spawn(|| {
            for _ in 0..50 {
                set_restore_in_progress();
                thread::yield_now();
                clear_restore_in_progress();
                thread::yield_now();
            }
        });

        // Wait for all threads to complete
        writer.join().expect("Writer thread panicked");
        for reader in readers {
            reader.join().expect("Reader thread panicked");
        }

        // Verify we observed both states (flag was toggled successfully)
        let true_count = true_reads.load(Ordering::Relaxed);
        let false_count = false_reads.load(Ordering::Relaxed);

        // Total reads should be 10 threads * 100 iterations = 1000
        assert_eq!(
            true_count + false_count,
            1000,
            "All reads should be accounted for"
        );

        // Both states should have been observed (writer toggled 50 times)
        assert!(
            true_count > 0,
            "Flag should have been observed as true at least once"
        );
        assert!(
            false_count > 0,
            "Flag should have been observed as false at least once"
        );

        // After writer completes (50 set/clear pairs), flag should be cleared
        assert!(
            !is_restore_in_progress(),
            "Flag should be cleared after all operations"
        );
    }
}
