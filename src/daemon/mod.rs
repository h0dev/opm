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
use macros_rs::{crashln, string, ternary};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use opm::process::{unix::NativeProcess as Process, MemoryInfo};
use serde::Serialize;
use serde_json::json;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{process, thread::sleep, time::Duration};
use home;

use opm::{
    config,
    helpers::{self, ColoredString},
    process::{dump, get_process_cpu_usage_with_children_from_process, hash, Runner},
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

extern "C" fn handle_termination_signal(_: libc::c_int) {
    // SAFETY: Signal handlers should be kept simple and avoid complex operations.
    // However, we need to save process state before daemon exits.
    // This is a critical operation to ensure process state is preserved for restore.
    // We accept the small risk of issues during signal handling because:
    // 1. The alternative (losing process state) is worse
    // 2. This only runs on daemon shutdown, not during normal operation
    // 3. Worst case: state isn't saved, but daemon still exits cleanly

    // Try to save process state before exiting
    // Use catch_unwind to prevent panics from crashing the signal handler
    let save_result = std::panic::catch_unwind(|| {
        // Load current process state from memory cache
        let runner = Runner::new();
        // Save current state to both memory cache AND permanent storage
        // This preserves running/crashed state as-is for restore
        // CRITICAL FIX: Must call save_permanent() to persist to disk,
        // otherwise state is lost when daemon restarts (memory cache is cleared)
        runner.save_permanent();
        
        log!("[daemon] shutdown complete - state saved to permanent storage", "action" => "shutdown");
    });

    // If save failed, log a warning (but still proceed with cleanup)
    if save_result.is_err() {
        // Note: Can't use log! macro without key-value pairs, using eprintln instead
        eprintln!("[daemon] warning: failed to save process state during shutdown");
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

fn restart_process() {
    log!("[DAEMON_V2_CHECK] Monitoring cycle initiated", "fingerprint" => "v2_fix");
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
        // The runner is no longer reloaded inside the loop.
        // This ensures that state changes from socket commands (like `opm start`)
        // are not wiped out during the daemon's monitoring cycle.
        // The entire cycle now operates on a single, consistent state.

        // Clone item to avoid borrowing issues when we mutate runner later.
        // This is required by Rust's borrow checker - we can't hold an immutable
        // reference to runner (via runner.info()) while also calling mutable
        // methods on runner (e.g., runner.stop(), runner.restart()).
        // The clone overhead is acceptable given that:
        // - Process struct is relatively small
        // - This runs infrequently (daemon interval)
        // - Correctness is more important than micro-optimizations
        let item = match runner.info(id) {
            Some(item) => item.clone(),
            None => continue, // Process was removed, skip it
        };

        let children = opm::process::process_find_children(item.pid);

        if !children.is_empty() && children != item.children {
            log!("[daemon] added", "children" => format!("{children:?}"));
            runner.set_children(id, children.clone()).save();
        }

        // Check memory limit if configured
        if item.running && item.max_memory > 0 {
            let pid_for_monitoring = item.shell_pid.unwrap_or(item.pid);
            if let Some(memory_info) =
                opm::process::get_process_memory_with_children(pid_for_monitoring)
            {
                if memory_info.rss > item.max_memory {
                    log!("[daemon] memory limit exceeded", "name" => item.name, "id" => id, 
                         "memory" => memory_info.rss, "limit" => item.max_memory);
                    println!(
                        "{} Process ({}) exceeded memory limit: {} > {} - stopping process",
                        *helpers::FAIL,
                        item.name,
                        helpers::format_memory(memory_info.rss),
                        helpers::format_memory(item.max_memory)
                    );
                    runner.stop(id);
                    // Don't mark as crashed since this is intentional enforcement
                    continue;
                }
            }
        }

        if item.running && item.watch.enabled {
            let path = item.path.join(item.watch.path.clone());
            let hash = hash::create(path);

            if hash != item.watch.hash {
                log!("[daemon] watch triggered reload", "name" => item.name, "id" => id);
                runner.restart(id, false, true); // Watch reload should increment counter
                log!("[daemon] watch reload complete", "name" => item.name, "id" => id);
                continue;
            }
        }

        // Check if process is alive based on PID
        // is_pid_alive() handles all PID validation (including PID <= 0)
        // For processes with children, also check if any children are alive
        // This prevents false positives when shell scripts exit but leave background processes running
        // 
        // Check both the actual PID and shell PID (if present) to determine if process is alive.
        // For shell-wrapped processes, either PID being alive means the process is running.
        // This is consistent with the display logic in build_process_item() and prevents
        // false crash detection when one PID exits but the other is still alive.
        let mut process_alive = opm::process::is_pid_alive(item.pid) || 
            item.shell_pid.map_or(false, |pid| opm::process::is_pid_alive(pid));

        // Handle case where main process died but children are still alive
        // This commonly occurs when shell scripts exit after spawning background processes
        if !process_alive && !item.children.is_empty() {
            // Check if any children are still alive
            // Note: We select the first alive child arbitrarily. In practice, for shell scripts
            // that spawn a single background service (the common use case), there will typically
            // be only one child. For multiple children, adopting any alive child allows continued
            // monitoring rather than falsely marking the entire process group as crashed.
            if let Some(&alive_child_pid) = item.children.iter().find(|&&child_pid| opm::process::is_pid_alive(child_pid)) {
                log!("[daemon] main process died but child process still alive, adopting child", 
                    "name" => item.name, "id" => id, "old_pid" => item.pid, 
                    "new_pid" => alive_child_pid, "children" => format!("{:?}", item.children));
                
                // Adopt the alive child as the new monitored PID
                // This prevents false crash detection when shell scripts exit after spawning services
                if runner.exists(id) {
                    let process = runner.process(id);
                    // Perform both mutations before saving to ensure atomicity
                    process.pid = alive_child_pid;
                    // Remove the adopted child from the children list
                    process.children.retain(|&pid| pid != alive_child_pid);
                    // Save the updated state atomically
                    runner.save();
                    
                    // Mark process as alive since we adopted a child
                    process_alive = true;
                    log!("[daemon] successfully adopted child process", 
                        "name" => item.name, "id" => id, "new_pid" => alive_child_pid);
                }
            }
        }

        // Check if process is alive and has been running successfully, keep monitoring
        // Note: We no longer auto-reset crash counter here - it persists to show
        // crash history over time. Only explicit reset (via reset_counters()) will clear it.
        if process_alive && item.running && item.crash.value > 0 {
            // Check if process has been running for at least the grace period
            let uptime_secs = (Utc::now() - item.started).num_seconds();
            let grace_period = daemon_config.crash_grace_period as i64;
            if uptime_secs >= grace_period {
                // Process has been stable - clear crashed flag but keep crash count
                if runner.exists(id) {
                    let process = runner.process(id);
                    // Clear crashed flag but keep crash.value to preserve history
                    process.crash.crashed = false;
                    // Save state after clearing crashed flag
                    runner.save();
                }
            }
        }

         // Handle processes that need to be started
         // If running=true but pid=0, the process needs to be spawned
         // This happens after manual start/restart commands via socket
         if item.running && item.pid == 0 && !item.crash.crashed {
             log!("[daemon] starting process with no PID", "name" => item.name, "id" => id);
             let is_daemon_operation = true;  // This is a daemon-initiated operation
             let should_increment_counter = false;  // Don't increment counter for initial start
             runner.restart(id, is_daemon_operation, should_increment_counter);
             log!("[daemon] process started", "name" => item.name, "id" => id, "new_pid" => runner.info(id).map(|p| p.pid).unwrap_or(0));
             continue;
         }

         // If process is dead, handle crash/restart logic
         if !process_alive {
             // Check if process was very recently started (within grace period)
             // This prevents the daemon from immediately restarting a process that just started
             // and gives the process time to initialize
             let grace_period = daemon_config.crash_grace_period as i64;
             let just_started = (Utc::now() - item.started).num_seconds() < grace_period;
             
             // Check if there was a recent manual action (to prevent daemon from marking as crashed immediately after manual start/restart)
             let recently_acted = has_recent_action_timestamp(id);
             
             // Check if this is a newly detected crash by looking at PID
             // If the PID is > 0, it means we thought the process was alive, so this is a new crash event
             // Note: We check item.pid (the actual process), not shell_pid
             let is_new_crash = item.pid > 0;

             // Don't mark as crashed if:
             // 1. There was a recent manual action (within 5 seconds), OR
             // 2. Process was just started (within grace period) - give it time to initialize
             if is_new_crash && !recently_acted && !just_started {
                // Check if process exited successfully (exit code 0) by checking the child handle
                // Use shell_pid if available (since we store handles by shell_pid), otherwise use regular pid
                // The handle is stored by shell_pid because that's the direct child of our spawn call
                let process_handle_pid = item.shell_pid.unwrap_or(item.pid);
                let mut exited_successfully = false;
                let mut handle_found = false;
                
                // Remove and check the handle to get exit status
                // This ensures we only check the exit status once (try_wait consumes it)
                if let Some((_, handle_ref)) = opm::process::PROCESS_HANDLES.remove(&process_handle_pid) {
                    handle_found = true;
                    if let Ok(mut child) = handle_ref.lock() {
                        // Check if process has exited and get its exit status
                        if let Ok(Some(status)) = child.try_wait() {
                            exited_successfully = status.success();
                            log!("[daemon] process exited", 
                                 "name" => item.name, "id" => id, "shell_pid" => process_handle_pid, 
                                 "success" => exited_successfully, "status" => format!("{:?}", status));
                        }
                    }
                }
                
                // Only treat as clean stop if we found the handle AND it exited successfully
                // If no handle found, treat as a crash (don't skip crash handling)
                if handle_found && exited_successfully {
                    // No longer reloading runner state here.
                    // The check below is sufficient to handle concurrently deleted processes.
                    if !runner.exists(id) {
                        log!("[daemon] process was deleted during exit detection, skipping", 
                             "name" => item.name, "id" => id);
                        continue;
                    }
                    
                    let process = runner.process(id);
                    process.running = false;
                    process.pid = 0;
                    process.shell_pid = None;
                    // Clear crashed flag for successful exits - this ensures processes
                    // show as "stopped" instead of "crashed" after clean exit
                    process.crash.crashed = false;
                    // Don't increment crash counter for successful exits
                    log!("[daemon] process stopped cleanly", 
                         "name" => item.name, "id" => id);
                    runner.save();
                    continue; // Skip crash handling
                }
                
                // No longer reloading runner state here.
                if !runner.exists(id) {
                    log!("[daemon] process was deleted during crash detection, skipping", 
                         "name" => item.name, "id" => id);
                    continue;
                }
                
                // Reset PID and shell_pid to 0 to indicate no valid PID
                let process = runner.process(id);
                process.pid = 0;
                process.shell_pid = None;
                
                // Increment crash counter - allow it to exceed limit to show total crash history
                process.crash.value += 1;
                process.crash.crashed = true;
                let crash_count = process.crash.value;

                // Only handle restart logic if process was supposed to be running
                if item.running {
                    // Emit crash event and notification for newly detected crashes
                    let process_name = item.name.clone();
                    if let Some(handle) = tokio::runtime::Handle::try_current().ok() {
                        handle.spawn(emit_crash_event_and_notification(id, process_name));
                    } else {
                        log!("[daemon] warning: crash event not emitted (no tokio runtime)", 
                             "name" => item.name, "id" => id);
                    }

                    // Check if we've reached or exceeded the maximum crash limit
                    // Using >= to stop when counter reaches the limit:
                    // - crash_count < 10 with max_restarts=10: allow restart (crash counter 1-9)
                    // - crash_count >= 10 with max_restarts=10: stop (counter reaches 10, no more restarts)
                    // Counter can exceed limit to track total crashes, but restarts stop at limit
                    if crash_count >= daemon_config.restarts {
                        // Reached max restarts - give up and set running=false
                        process.running = false;
                        log!("[daemon] process reached max crash limit", 
                             "name" => item.name, "id" => id, "crash_count" => crash_count, "max_restarts" => daemon_config.restarts);
                    } else {
                        // Still within crash limit - mark as crashed
                        log!("[daemon] process crashed", 
                             "name" => item.name, "id" => id, "crash_count" => crash_count, "max_restarts" => daemon_config.restarts);
                    }
                } else {
                    // Process was already stopped but crashed again (e.g., after manual restart)
                    // Counter has been incremented to track crash history even after limit
                    log!("[daemon] stopped process crashed again", 
                         "name" => item.name, "id" => id, "crash_count" => crash_count);
                }
                
                // Save state after crash detection to persist crash counter and PID updates
                runner.save();
             } else if item.running {
                 // Check if process was very recently started (within grace period)
                 // This prevents the daemon from immediately restarting a process that just started
                 let grace_period = daemon_config.crash_grace_period as i64;
                 let just_started = (Utc::now() - item.started).num_seconds() < grace_period;
                 
                 // Check if there was a recent manual action (to prevent daemon from setting running=false immediately after manual start/restart)
                 let recently_acted = has_recent_action_timestamp(id);
                 
                 // Process is already marked as crashed - check limit before attempting restart
                 // This handles cases where counter may have been incremented by restart failures
                 if item.crash.value >= daemon_config.restarts && !recently_acted {
                     // Already reached max restarts - set running=false and stop trying
                     // Skip setting to false if there was a recent manual action
                     let process = runner.process(id);
                     process.running = false;
                     log!("[daemon] process already reached max crash limit, stopping restart attempts", 
                          "name" => item.name, "id" => id, "crash_count" => item.crash.value, "max_restarts" => daemon_config.restarts);
                     // Save state after updating running flag
                     runner.save();
                 } else if !recently_acted && !just_started {
                     // Still within limit, no recent action, and not just started - attempt restart now
                     if runner.exists(id) {
                         // Check if process is frozen (being edited/deleted)
                         if runner.is_frozen(id) {
                             log!("[daemon] skipping restart - process is frozen (being edited/deleted)", 
                                  "name" => item.name, "id" => id);
                             continue;
                         }
                         
                         // Check if process is still marked as running after reload
                         // This prevents restarting processes that were stopped/removed during reload
                         if let Some(proc) = runner.info(id) {
                             if proc.running {
                                 log!("[daemon] restarting crashed process", 
                                      "name" => item.name, "id" => id, "crash_count" => item.crash.value, "max_restarts" => daemon_config.restarts);
                                 runner.restart(id, true, true);
                                 log!("[daemon] restart complete", 
                                      "name" => item.name, "id" => id, "new_pid" => runner.info(id).unwrap().pid);
                                 // Note: restart() now calls save() internally, so we don't need to save here
                             } else {
                                 log!("[daemon] process was marked as stopped during reload, skipping restart",
                                      "name" => item.name, "id" => id);
                             }
                         }
                     } else {
                         log!("[daemon] process was deleted, skipping restart",
                              "name" => item.name, "id" => id);
                     }
                 } else {
                     // Recent action was taken or process just started, skip automatic restart attempts
                     if just_started {
                         log!("[daemon] skipping restart - process just started (giving it time to initialize)", "name" => item.name, "id" => id);
                     } else {
                         log!("[daemon] skipping restart due to recent manual action", "name" => item.name, "id" => id);
                     }
                 }
            } else {
                // Process was already stopped and marked as crashed
                // Don't log anything to avoid spam - user already knows it's stopped
            }
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
        .with(Style::rounded().remove_horizontals())
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

pub fn stop() {
    if pid::exists() {
        println!("{} Stopping OPM daemon", *helpers::SUCCESS);

        match pid::read() {
            Ok(pid) => {
                if let Err(err) = opm::process::process_stop(pid.get()) {
                    log!("[daemon] failed to stop", "error" => err);
                }
                pid::remove();
                log!("[daemon] stopped", "pid" => pid);
                println!("{} OPM daemon stopped", *helpers::SUCCESS);
            }
            Err(err) => {
                // PID file exists but can't be read (corrupted or invalid)
                log!("[daemon] removing corrupted PID file", "error" => err);
                println!("{} PID file is corrupted, removing it", *helpers::SUCCESS);
                pid::remove();
                println!("{} OPM daemon stopped", *helpers::SUCCESS);
            }
        }
    } else {
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

    #[inline]
    extern "C" fn init() {
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
            libc::signal(libc::SIGTERM, handle_termination_signal as *const () as usize);
            libc::signal(libc::SIGPIPE, handle_sigpipe as *const () as usize);
        };

        DAEMON_START_TIME.set(Utc::now().timestamp_millis() as f64);

        pid::write(process::id());
        log!("[daemon] new fork", "pid" => process::id());

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

        // Initialize on daemon startup: load state from disk and clear any old temp files
        // IMPORTANT: This must be done BEFORE starting the socket server to avoid race conditions
        // where CLI commands arrive before the cache is initialized, causing state to be lost
        use opm::process::dump;
        let _startup_runner = dump::init_on_startup();

        // Clean up all stale timestamp files from previous daemon sessions
        // This prevents old timestamps from interfering with crash detection
        cleanup_all_timestamp_files();

        // Start Unix socket server for CLI-daemon communication
        // Socket server must be started AFTER init_on_startup() to ensure memory cache is ready
        let socket_path = global!("opm.socket").to_string();
        let socket_path_clone = socket_path.clone();
        match std::thread::Builder::new()
            .name("socket-server".to_string())
            .spawn(move || {
                if let Err(e) = opm::socket::start_socket_server(&socket_path) {
                    log!("[daemon] Unix socket server error", "error" => format!("{}", e));
                    eprintln!("[daemon] Critical: Unix socket server failed to start: {}", e);
                }
            })
        {
            Ok(_) => {
                log!("[daemon] Unix socket server started", "path" => socket_path_clone);
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
        stop();
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
                ::log::warn!("Failed to read directory entry during timestamp cleanup: {}", e);
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

// Helper function to check if there was a recent action timestamp file
fn has_recent_action_timestamp(id: usize) -> bool {
    match home::home_dir() {
        Some(home_dir) => {
            let action_file = format!("{}/.opm/{}{}{}", home_dir.display(), TIMESTAMP_FILE_PREFIX, id, TIMESTAMP_FILE_SUFFIX);
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
        },
        None => false,
    }
}
