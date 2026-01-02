#[macro_use]
mod log;
mod fork;

use chrono::{DateTime, Utc};
use colored::Colorize;
use fork::{daemon, Fork};
use global_placeholders::global;
use macros_rs::{crashln, str, string, ternary, then};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use pmc::process::{MemoryInfo, unix::NativeProcess as Process};
use serde::Serialize;
use serde_json::json;
use std::{process, thread::sleep, time::Duration};

use pmc::{
    config, file,
    helpers::{self, ColoredString},
    process::{hash, id::Id, Runner, Status, get_process_cpu_usage_percentage},
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

extern "C" fn handle_termination_signal(_: libc::c_int) {
    pid::remove();
    log!("[daemon] killed", "pid" => process::id());
    unsafe { libc::_exit(0) }
}

fn restart_process() {
    for (id, item) in Runner::new().items_mut() {
        let mut runner = Runner::new();
        let children = pmc::process::process_find_children(item.pid);

        if !children.is_empty() && children != item.children {
            log!("[daemon] added", "children" => format!("{children:?}"));
            runner.set_children(*id, children).save();
        }

        if item.running && item.watch.enabled {
            let path = item.path.join(item.watch.path.clone());
            let hash = hash::create(path);

            if hash != item.watch.hash {
                runner.restart(item.id, false);
                log!("[daemon] watch reload", "name" => item.name, "hash" => "hash");
                continue;
            }
        }

        // Check if process is marked as running but not actually running
        if !item.running && pid::running(item.pid as i32) {
            Runner::new().set_status(*id, Status::Running);
            log!("[daemon] process fix status", "name" => item.name, "id" => id);
            continue;
        }

        // Skip if process is not running or is actually still running
        then!(!item.running || pid::running(item.pid as i32), continue);

        // Process crashed - handle restart logic
        let max_restarts = config::read().daemon.restarts;
        
        if item.crash.value >= max_restarts {
            log!("[daemon] process exceeded max crashes", "name" => item.name, "id" => id, "crashes" => item.crash.value);
            runner.stop(item.id);
            runner.set_crashed(*id).save();
            continue;
        }
        
        // Attempt to restart the crashed process
        log!("[daemon] attempting restart", "name" => item.name, "id" => id, "crashes" => item.crash.value);
        runner.get(item.id).crashed();
        log!("[daemon] restarted", "name" => item.name, "id" => id, "crashes" => item.crash.value + 1);
    }
}

pub fn health(format: &String) {
    let mut pid: Option<i32> = None;
    let mut cpu_percent: Option<f64> = None;
    let mut uptime: Option<DateTime<Utc>> = None;
    let mut memory_usage: Option<MemoryInfo> = None;
    let mut runner: Runner = file::read_object(global!("opm.dump"));

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
            if let Ok(process) = Process::new(process_id.get::<u32>()) {
                pid = Some(process.pid() as i32);
                uptime = Some(pid::uptime().unwrap());
                memory_usage = process.memory_info().ok().map(MemoryInfo::from);
                cpu_percent = Some(get_process_cpu_usage_percentage(process_id.get::<i64>()));
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
        status: ColoredString(ternary!(pid::exists(), "online".green().bold(), "stopped".red().bold())),
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
                println!("{}\n{table}\n", format!("PMC daemon information").on_bright_white().black());
                println!(" {}", format!("Use `pmc daemon restart` to restart the daemon").white());
                println!(" {}", format!("Use `pmc daemon reset` to clean process id values").white());
            }
            _ => {}
        };
    };
}

pub fn stop() {
    if pid::exists() {
        println!("{} Stopping PMC daemon", *helpers::SUCCESS);

        match pid::read() {
            Ok(pid) => {
                if let Err(err) = pmc::process::process_stop(pid.get()) {
                    log!("[daemon] failed to stop", "error" => err);
                }
                pid::remove();
                log!("[daemon] stopped", "pid" => pid);
                println!("{} PMC daemon stopped", *helpers::SUCCESS);
            }
            Err(err) => crashln!("{} Failed to read PID file: {}", *helpers::FAIL, err),
        }
    } else {
        crashln!("{} The daemon is not running", *helpers::FAIL)
    }
}

pub fn start(verbose: bool) {
    println!("{} Spawning OPM daemon (opm_base={})", *helpers::SUCCESS, global!("opm.base"));

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

    println!("{} OPM Successfully daemonized (type={})", *helpers::SUCCESS, global!("opm.daemon.kind"));
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

    println!("{} Successfully reset (index={})", *helpers::SUCCESS, runner.id);
}

pub mod pid;
