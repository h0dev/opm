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
use std::{panic, process, thread::sleep, time::Duration};

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
                runner.restart(*id, false);
                runner.save();
                log!("[daemon] watch reload complete", "name" => item.name, "id" => id);
                continue;
            }
        }

        // Skip processes with invalid PID (0 or negative)
        if item.pid <= 0 {
            continue;
        }
        
        // Simple check if process is alive using the PID
        let process_alive = opm::process::is_pid_alive(item.pid);
        
        // If process is dead, reset PID immediately (CRITICAL per requirements)
        if !process_alive {
            let process = runner.process(*id);
            process.pid = 0;  // Set to 0 to indicate no valid PID
            
            // Only handle crash/restart logic if process was supposed to be running
            if item.running {
                // Process was supposed to be running but is dead - this is a crash
                log!("[daemon] detected crash", "name" => item.name, "id" => id, "pid" => item.pid);
                
                // Increment consecutive crash counter immediately
                process.crash.value += 1;
                
                let current_crash_value = process.crash.value;
                let max_restarts = config::read().daemon.restarts;
                
                // Check if we should retry (crash.value <= max) or give up (crash.value > max)
                // Using <= to allow exactly max_restarts attempts (e.g., 10 attempts for max_restarts=10)
                if current_crash_value <= max_restarts {
                    // RETRY: Attempt restart
                    log!("[daemon] attempting restart", "name" => item.name, "id" => id, 
                         "crashes" => current_crash_value, "max" => max_restarts);
                    println!(
                        "{} Process '{}' (id={}) crashed - attempting restart (attempt {}/{})",
                        *helpers::FAIL,
                        item.name,
                        id,
                        current_crash_value,
                        max_restarts
                    );
                    
                    // Save state with updated crash counter
                    runner.save();
                    
                    // Attempt to restart the crashed process
                    // Wrap in catch_unwind to prevent daemon crashes from panics in restart logic
                    // Pass dead=true so restart() knows this is a crash restart
                    // restart() will increment restarts counter and handle the restart logic
                    let restart_result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        runner.restart(*id, true);
                        runner.save();
                    }));
                    
                    if let Err(panic_info) = restart_result {
                        log!("[daemon] restart panicked", "name" => item.name, "id" => id);
                        eprintln!("{} Restart panicked for process '{}' (id={}): {:?}", 
                                  *helpers::FAIL, item.name, id, panic_info);
                        // Mark as crashed for retry on next cycle
                        let mut runner = Runner::new();
                        runner.set_crashed(*id).save();
                        continue;
                    }
                    
                    // Check if restart succeeded
                    let restarted_runner = Runner::new();
                    if let Some(restarted_process) = restarted_runner.info(*id) {
                        if restarted_process.running && opm::process::is_pid_alive(restarted_process.pid) {
                            log!("[daemon] restarted successfully", "name" => item.name, "id" => id, 
                                 "new_pid" => restarted_process.pid);
                            println!(
                                "{} Successfully restarted process '{}' (id={})",
                                *helpers::SUCCESS,
                                item.name,
                                id
                            );
                        } else {
                            log!("[daemon] restart failed - process not running", "name" => item.name, "id" => id);
                            println!(
                                "{} Failed to restart process '{}' (id={}) - process not running",
                                *helpers::FAIL,
                                item.name,
                                id
                            );
                            // Mark as crashed for retry on next cycle
                            let mut runner = Runner::new();
                            runner.set_crashed(*id).save();
                        }
                    }
                } else {
                    // GIVE UP: Max restarts reached
                    log!("[daemon] max restarts reached", "name" => item.name, "id" => id, 
                         "crashes" => current_crash_value);
                    println!(
                        "{} Process '{}' (id={}) exceeded max crash limit ({}) - stopping",
                        *helpers::FAIL,
                        item.name,
                        id,
                        max_restarts
                    );
                    
                    // Set running to false and mark as crashed
                    process.running = false;
                    process.crash.crashed = true;
                    // Reset crash counter for next manual start (as per requirements)
                    process.crash.value = 0;
                    
                    runner.save();
                }
            } else {
                // Process was already stopped, just update PID
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
        if let Ok(process_id) = pid::read() {
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
            Err(err) => crashln!("{} Failed to read PID file: {}", *helpers::FAIL, err),
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
            Ok(pid) => then!(!pid::running(pid.get()), pid::remove()),
            Err(_) => crashln!("{} The daemon is already running", *helpers::FAIL),
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
        Ok(Fork::Parent(_)) => {}
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
