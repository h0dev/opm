#[macro_use]
mod log;
mod fork;

use chrono::{DateTime, Utc};
use colored::Colorize;
use fork::{Fork, daemon};
use global_placeholders::global;
use macros_rs::{crashln, str, string, ternary, then};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use opm::process::{MemoryInfo, unix::NativeProcess as Process};
use serde::Serialize;
use serde_json::json;
use std::{process, thread::sleep, time::Duration};

use opm::{
    config, file,
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

extern "C" fn handle_termination_signal(_: libc::c_int) {
    pid::remove();
    log!("[daemon] killed", "pid" => process::id());
    unsafe { libc::_exit(0) }
}

extern "C" fn handle_sigpipe(_: libc::c_int) {
    // Ignore SIGPIPE - this prevents the daemon from crashing when writing to closed stdout/stderr
    // This can happen when the daemon tries to use println!() after being daemonized
}

fn restart_process() {
    // Load daemon config once at the start to avoid repeated I/O operations
    let daemon_config = config::read().daemon;
    
    for (id, item) in Runner::new().items_mut() {
        let mut runner = Runner::new();
        let children = opm::process::process_find_children(item.pid);

        if !children.is_empty() && children != item.children {
            log!("[daemon] added", "children" => format!("{children:?}"));
            // Clone once for saving to disk via set_children
            runner.set_children(*id, children.clone()).save();
            // Clone again to update the snapshot's item.children, so later logic uses fresh data
            // Both clones are necessary because children is borrowed later in this iteration
            item.children = children.clone();
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
                    runner.stop(*id);
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
                runner.restart(*id, false, true);  // Watch reload should increment counter
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
                if runner.exists(*id) {
                    let process = runner.process(*id);
                    // Clear crashed flag but keep crash.value to preserve history
                    process.crash.crashed = false;
                    runner.save();
                }
            }
        }
        
        // If process is dead, handle crash/restart logic
        if !process_alive {
            // Reset PID to 0 if it wasn't already
            if item.pid > 0 {
                let process = runner.process(*id);
                process.pid = 0;  // Set to 0 to indicate no valid PID
            }
            
            // Only handle crash/restart logic if process was supposed to be running
            if item.running {
                
                // Check if this is a newly detected crash (not already marked as crashed)
                // If already crashed, we've already incremented the counter and are waiting for restart
                if !item.crash.crashed {
                    // Get crash count before modifying
                    let crash_count = {
                        let process = runner.process(*id);
                        // Increment consecutive crash counter
                        process.crash.value += 1;
                        process.crash.crashed = true;
                        // Keep running=true so daemon continues restart attempts
                        // Only set running=false if we've exceeded max crash limit
                        process.crash.value
                    };
                    
                    // Check if we've exceeded the maximum crash limit
                    // Using > instead of >= because:
                    // - crash_count=10 with max_restarts=10: allow restart (10th restart attempt)
                    // - crash_count=11 with max_restarts=10: give up (exceeded 10 restarts)
                    // This means "restarts: 10" allows exactly 10 restart attempts
                    if crash_count > daemon_config.restarts {
                        // Exceeded max restarts - give up and set running=false
                        let process = runner.process(*id);
                        process.running = false;
                        log!("[daemon] process exceeded max crash limit", 
                             "name" => item.name, "id" => id, "crash_count" => crash_count, "max_restarts" => daemon_config.restarts);
                        runner.save();
                    } else {
                        // Still within crash limit - mark as crashed and save
                        // Next daemon cycle will restart it
                        log!("[daemon] process crashed", 
                             "name" => item.name, "id" => id, "crash_count" => crash_count);
                        runner.save();
                    }
                } else {
                    // Process is already marked as crashed - attempt restart now
                    runner.restart(*id, true, true);
                    runner.save();
                }
            } else {
                // Process was already stopped (running=false), just update PID
                // This can happen if:
                // 1. User manually stopped the process
                // 2. Process previously hit max crash limit and running was set to false
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
    let mut runner: Runner = file::read_object(global!("opm.dump"));
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
    println!(
        "{} Spawning OPM daemon (opm_base={})",
        *helpers::SUCCESS,
        global!("opm.base")
    );

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

        unsafe { 
            libc::signal(libc::SIGTERM, handle_termination_signal as usize);
            libc::signal(libc::SIGPIPE, handle_sigpipe as usize);
        };

        pid::write(process::id());
        log!("[daemon] new fork", "pid" => process::id());

        loop {
            then!(!Runner::new().is_empty(), restart_process());
            sleep(Duration::from_millis(config.interval));
        }
    }

    println!(
        "{} OPM Successfully daemonized (type={})",
        *helpers::SUCCESS,
        global!("opm.daemon.kind")
    );
    match daemon(false, verbose) {
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
            eprintln!("{} Warning: Daemon PID file not detected within {}ms", *helpers::WARN, max_wait_ms);
        }
        Ok(Fork::Child) => init(),
        Err(err) => crashln!("{} Daemon creation failed with code {err}", *helpers::FAIL),
    }
}

pub fn restart(verbose: bool) {
    if pid::exists() {
        stop();
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

pub mod pid;
