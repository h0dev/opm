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
// Increased to 5 seconds to accommodate slower-starting services like Cloudflare tunnels
const STARTUP_GRACE_PERIOD_SECS: i64 = 5;

extern "C" fn handle_termination_signal(_: libc::c_int) {
    pid::remove();
    log!("[daemon] killed", "pid" => process::id());
    unsafe { libc::_exit(0) }
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
                    runner.stop(item.id);
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
                runner.restart(item.id, false);
                runner.save();
                log!("[daemon] watch reload complete", "name" => item.name, "id" => id);
                continue;
            }
        }

        // Determine if we should attempt to restart this process
        let process_running = pid::running(item.pid as i32);
        
        // Check if process was recently started (within grace period)
        // This prevents false crash detection when shell processes haven't spawned children yet
        // Only apply grace period on initial start (not restarts) to avoid blocking crash detection
        let now = Utc::now();
        let seconds_since_start = (now - item.started).num_seconds();
        // Note: crash.value tracks failed restarts, restarts tracks all restart attempts
        // Both should be 0 only on the very first start before any crashes
        let is_initial_start = item.crash.value == 0 && item.restarts == 0;
        let recently_started = is_initial_start && seconds_since_start < STARTUP_GRACE_PERIOD_SECS;
        
        // Check if we can actually read process stats (CPU, memory, etc.)
        // If Process::new_fast() fails, it means the process is dead/inaccessible
        // even if pid::running() returns true (e.g., zombie, PID reused, permission issue)
        // Use the same PID selection logic as memory monitoring (line 58)
        let pid_for_monitoring = item.shell_pid.unwrap_or(item.pid);
        let mut process_readable = false;
        
        if process_running {
            // Try to create a process handle and check its readability
            if let Ok(process) = Process::new_fast(pid_for_monitoring as u32) {
                // Process handle created successfully
                // Additional check: detect zombie processes or PIDs that were reused
                // by checking if memory info is completely unreadable (not just zero, but truly unreadable).
                // Note: We do NOT check CPU usage here, as 0% CPU is legitimate for idle processes.
                // Only check memory readability for processes marked as running and outside the grace period.
                if item.running && !recently_started {
                    // If memory_info() fails, the process is likely a zombie or PID was reused
                    process_readable = process.memory_info().is_ok();
                } else {
                    // Within grace period or not marked as running - assume readable
                    process_readable = true;
                }
            }
        }
        
        // For processes started through a shell (e.g., /bin/sh -c 'command'), we need to check
        // if the actual child process is still alive, not just the shell wrapper.
        // When a child process crashes immediately, get_actual_child_pid may fall back to returning 
        // the shell PID. The shell remains alive even after its child exits, so we need to 
        // verify that it still has children.
        // 
        // We already computed current children at line 41, so reuse that value.
        let child_process_alive = if item.shell_pid.is_some() && process_running && process_readable {
            // This is a shell-spawned process - check if the shell still has children
            // If the shell has no children, the actual process has crashed
            // UNLESS it was just started and needs time to spawn children
            !children.is_empty() || recently_started
        } else if process_running && process_readable {
            // Not a shell-spawned process (or shell_pid wasn't detected)
            // If the stored PID is actually a shell that lost its child, it would have no children
            // We need to detect this case to catch immediately-crashing processes where
            // get_actual_child_pid fell back to the shell PID but didn't set shell_pid
            //
            // If process has no children, it might be:
            // 1. A simple process that doesn't spawn children (normal) - stays alive
            // 2. A shell whose child crashed (problem) - shell stays alive but orphaned
            // 
            // To distinguish: if we've never seen this process with children, it's probably case 1.
            // If item.children was previously populated, it's probably case 2.
            // For now, conservatively assume no children = crashed only if we previously had children
            // Also apply grace period to avoid false positives immediately after start
            if children.is_empty() && !item.children.is_empty() && !recently_started {
                // Process previously had children but now doesn't - likely crashed
                false
            } else {
                // Process is running (either with children or never had them)
                true
            }
        } else {
            // Process itself is dead (PID not running or not readable)
            false
        };
        
        // We should restart if:
        // 1. Process is marked as running but the actual process is not alive (fresh crash)
        let fresh_crash = item.running && !child_process_alive;
        // 2. OR process is marked as crashed and not running (failed previous restart attempt that should be retried)
        let failed_restart = item.crash.crashed && !item.running;
        
        let should_restart = fresh_crash || failed_restart;
        
        // Skip if process doesn't need restarting
        if !should_restart {
            continue;
        }

        // Process crashed - handle restart logic
        let max_restarts = config::read().daemon.restarts;

        if item.crash.value >= max_restarts {
            log!("[daemon] process exceeded max crashes", "name" => item.name, "id" => id, "crashes" => item.crash.value);
            println!(
                "{} Process '{}' (id={}) exceeded max crash limit ({}) - stopping",
                *helpers::FAIL,
                item.name,
                id,
                max_restarts
            );
            runner.stop(item.id);
            runner.set_crashed(*id).save();
            continue;
        }

        // Attempt to restart the crashed process
        // Use the already-computed failed_restart variable to determine if this is a retry
        if failed_restart {
            log!("[daemon] retrying failed restart", "name" => item.name, "id" => id, "crashes" => item.crash.value);
            println!(
                "{} Retrying restart for process '{}' (id={}) (attempt {}/{})",
                *helpers::FAIL,
                item.name,
                id,
                item.crash.value + 1,
                max_restarts
            );
        } else {
            log!("[daemon] attempting restart", "name" => item.name, "id" => id, "crashes" => item.crash.value);
            println!(
                "{} Process '{}' (id={}) crashed - attempting restart (attempt {}/{})",
                *helpers::FAIL,
                item.name,
                id,
                item.crash.value + 1,
                max_restarts
            );
        }
        
        // This calls restart and saves to disk, consuming runner
        runner.get(item.id).crashed();
        
        // Reload runner from disk to get the updated state after restart
        // This is necessary because we're iterating over a snapshot and the restart
        // operation updates the saved state on disk
        let restarted_runner = Runner::new();
        if let Some(restarted_process) = restarted_runner.info(*id) {
            if restarted_process.running {
                log!("[daemon] restarted successfully", "name" => item.name, "id" => id, "crashes" => item.crash.value + 1);
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

        unsafe { libc::signal(libc::SIGTERM, handle_termination_signal as usize) };

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
