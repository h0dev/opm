#[macro_use]
mod log;
mod api;
mod fork;

use api::{DAEMON_CPU_PERCENTAGE, DAEMON_MEM_USAGE, DAEMON_START_TIME, GLOBAL_EVENT_MANAGER, GLOBAL_NOTIFICATION_MANAGER};
use chrono::{DateTime, Utc};
use colored::Colorize;
use fork::{Fork, daemon};
use global_placeholders::global;
use macros_rs::{crashln, str, string, ternary};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use opm::process::{MemoryInfo, unix::NativeProcess as Process};
use serde::Serialize;
use serde_json::json;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{process, thread::sleep, time::Duration};

use opm::{
    config,
    helpers::{self, ColoredString},
    process::{Runner, get_process_cpu_usage_with_children_from_process, hash, id::Id},
};

use tabled::{
    Table, Tabled,
    settings::{
        Color, Rotate,
        object::Columns,
        style::{BorderColor, Style},
        themes::Colorization,
    },
};

// Grace period in seconds to wait after process start before checking for crashes
// This prevents false crash detection when shell processes haven't spawned children yet
// Reduced to 1 second to allow faster detection of immediately-crashing processes
const STARTUP_GRACE_PERIOD_SECS: i64 = 1;

static ENABLE_API: AtomicBool = AtomicBool::new(false);
static ENABLE_WEBUI: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_termination_signal(_: libc::c_int) {
    // SAFETY: Signal handlers should be kept simple and avoid complex operations.
    // However, we need to save crashed process state before daemon exits.
    // This is a critical operation to ensure crashed processes can be restored.
    // We accept the small risk of issues during signal handling because:
    // 1. The alternative (losing crashed process state) is worse
    // 2. This only runs on daemon shutdown, not during normal operation
    // 3. Worst case: state isn't saved, but daemon still exits cleanly
    
    // Try to save crashed process state before exiting
    // Use catch_unwind to prevent panics from crashing the signal handler
    let save_result = std::panic::catch_unwind(|| {
        let mut runner = Runner::new();
        let process_ids: Vec<usize> = runner.process_ids().collect();
        for id in process_ids {
            // Get process info once and clone name if needed to avoid multiple lookups
            if let Some(process) = runner.info(id) {
                if process.crash.crashed && process.running {
                    let name = process.name.clone();
                    runner.process(id).running = false;
                    log!("[daemon] marking crashed process as stopped for restore", 
                         "id" => id, "name" => name);
                }
            }
        }
        runner.save();
        
        // Delete temp file after saving crashed state
        use std::fs;
        if let Ok(_) = fs::remove_file(global!("opm.dump.temp")) {
            log!("[daemon] cleaned up temp dump file", "action" => "cleanup");
        }
    });
    
    // If save failed, log a warning (but still proceed with cleanup)
    if save_result.is_err() {
        // Note: Can't use log! macro without key-value pairs, using eprintln instead
        eprintln!("[daemon] warning: failed to save crashed process state during shutdown");
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
        notification_manager.send(
            opm::notifications::NotificationEvent::ProcessCrash,
            "Process Crashed",
            &format!("Process '{}' has crashed", name),
        ).await;
    }
}

fn restart_process() {
    // Load daemon config once at the start to avoid repeated I/O operations
    let daemon_config = config::read().daemon;

    // Use a single Runner instance to avoid state synchronization issues
    let runner = Runner::new();
    // Collect IDs first to avoid borrowing issues during iteration
    // Use process_ids() instead of items().keys() to avoid cloning all processes
    let process_ids: Vec<usize> = runner.process_ids().collect();

    for id in process_ids {
        // Note: We reload runner at the start of each iteration to ensure we see
        // changes made by previous iterations (e.g., when a previous process was
        // restarted and the state was saved to disk). This is necessary because
        // operations like restart() modify the state and save it, and we need
        // the latest state for accurate crash detection and restart logic.
        //
        // Performance considerations:
        // - Runner::new() loads from disk, which could be expensive
        // - However, the daemon runs infrequently (default 1000ms interval)
        // - There are typically few processes, so total overhead is low
        // - Correctness is prioritized over performance here
        //
        // Alternative approaches considered:
        // - Caching and selective reload: Complex to implement correctly,
        //   and the performance gain would be minimal given typical usage
        // - Using a refresh() method: Would need to be implemented in Runner,
        //   and would still require reading from disk
        //
        // TODO: Consider implementing Runner::reload() method for future optimization
        // that only updates changed state rather than full reconstruction from disk.
        // This would be more efficient but adds complexity.
        let mut runner = Runner::new();

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
                    runner.save();
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
                runner.save();
                log!("[daemon] watch reload complete", "name" => item.name, "id" => id);
                continue;
            }
        }

        // Check if process is alive based on PID
        // is_pid_alive() handles all PID validation (including PID <= 0)
        let process_alive = opm::process::is_pid_alive(item.pid);

        // If process is alive and has been running successfully, keep monitoring
        // Note: We no longer auto-reset crash counter here - it persists to show
        // crash history over time. Only explicit reset (via reset_counters()) will clear it.
        if process_alive && item.running && item.crash.value > 0 {
            // Check if process has been running for at least the grace period
            let uptime_secs = (Utc::now() - item.started).num_seconds();
            if uptime_secs >= STARTUP_GRACE_PERIOD_SECS {
                // Process has been stable - clear crashed flag but keep crash count
                if runner.exists(id) {
                    let process = runner.process(id);
                    // Clear crashed flag but keep crash.value to preserve history
                    process.crash.crashed = false;
                    runner.save();
                }
            }
        }

        // If process is dead, handle crash/restart logic
        if !process_alive {
            // Check if this is a newly detected crash by looking at PID
            // PID > 0 means we thought the process was alive, so this is a new crash event
            let is_new_crash = item.pid > 0;
            
            // Reset PID to 0 if it wasn't already (for new crashes)
            if is_new_crash {
                let process = runner.process(id);
                process.pid = 0; // Set to 0 to indicate no valid PID
            }

            // Increment crash counter for newly detected crashes to track actual crash count
            // This happens when PID was > 0 (we thought process was alive) and now it's dead
            // This ensures we count each actual crash, even beyond the restart limit
            if is_new_crash {
                // Get crash count after incrementing
                let crash_count = {
                    let process = runner.process(id);
                    // Increment crash counter - allow it to exceed limit to show total crash history
                    process.crash.value += 1;
                    process.crash.crashed = true;
                    process.crash.value
                };

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
                        let process = runner.process(id);
                        process.running = false;
                        log!("[daemon] process reached max crash limit", 
                             "name" => item.name, "id" => id, "crash_count" => crash_count, "max_restarts" => daemon_config.restarts);
                        runner.save();
                    } else {
                        // Still within crash limit - mark as crashed and save
                        // Next daemon cycle will restart it
                        log!("[daemon] process crashed", 
                             "name" => item.name, "id" => id, "crash_count" => crash_count, "max_restarts" => daemon_config.restarts);
                        runner.save();
                    }
                } else {
                    // Process was already stopped but crashed again (e.g., after manual restart)
                    // Counter has been incremented to track crash history even after limit
                    log!("[daemon] stopped process crashed again", 
                         "name" => item.name, "id" => id, "crash_count" => crash_count);
                    runner.save();
                }
            } else if item.running {
                // Process is already marked as crashed - check limit before attempting restart
                // This handles cases where counter may have been incremented by restart failures
                if item.crash.value >= daemon_config.restarts {
                    // Already reached max restarts - set running=false and stop trying
                    let process = runner.process(id);
                    process.running = false;
                    log!("[daemon] process already reached max crash limit, stopping restart attempts", 
                         "name" => item.name, "id" => id, "crash_count" => item.crash.value, "max_restarts" => daemon_config.restarts);
                    runner.save();
                } else {
                    // Still within limit - attempt restart now
                    log!("[daemon] restarting crashed process", 
                         "name" => item.name, "id" => id, "crash_count" => item.crash.value, "max_restarts" => daemon_config.restarts);
                    runner.restart(id, true, true);
                    runner.save();
                    log!("[daemon] restart complete", 
                         "name" => item.name, "id" => id, "new_pid" => runner.info(id).map(|p| p.pid).unwrap_or(0));
                }
            } else {
                // Process was already stopped and marked as crashed
                // Don't log anything to avoid spam - user already knows it's stopped
                runner.save();
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
    #[tokio::main]
    async extern "C" fn init() {
        pid::name("OPM Restart Handler Daemon");

        let config = config::read().daemon;
        let api_enabled = ENABLE_API.load(Ordering::Acquire);
        let ui_enabled = ENABLE_WEBUI.load(Ordering::Acquire);

        unsafe {
            libc::signal(libc::SIGTERM, handle_termination_signal as usize);
            libc::signal(libc::SIGPIPE, handle_sigpipe as usize);
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

            // Spawn API server in a separate task
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
                tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;

                // Try to connect to the API server
                if tokio::net::TcpStream::connect(&addr).await.is_ok() {
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

        // Initialize on daemon startup: merge temp into permanent, set crashed to stopped
        // This must be done before the main loop starts
        use opm::process::dump;
        let _startup_runner = dump::init_on_startup();

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

    // Check if ID 0 exists but ID 1 exists
    if !runner.exists(0) && runner.exists(1) {
        // Get the process at ID 1
        if let Some(process_at_1) = runner.info(1).cloned() {
            // Remove it from ID 1
            runner.list.remove(&1);

            // Insert it at ID 0
            let mut new_process = process_at_1;
            new_process.id = 0;
            runner.list.insert(0, new_process);

            // Save the changes
            runner.save();

            println!("{} Rearranged ID 1 to ID 0", *helpers::SUCCESS);
            log!("[daemon] rearranged ID 1 to ID 0", "id" => "0");
        }
    }

    let largest = runner.size();

    match largest {
        Some(id) => runner.set_id(Id::from(str!(id.to_string()))),
        None => runner.set_id(Id::new(0)),
    }

    println!(
        "{} Successfully reset (index={})",
        *helpers::SUCCESS,
        runner.id
    );
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
