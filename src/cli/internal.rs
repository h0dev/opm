use colored::Colorize;
use lazy_static::lazy_static;
use macros_rs::{crashln, string, ternary, then};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use opm::process::{MemoryInfo, unix::NativeProcess as Process};
use regex::Regex;
use serde::Serialize;
use serde_json::json;
use std::fs;

#[cfg(not(target_os = "linux"))]
use nix::{errno::Errno, sys::signal::kill, unistd::Pid};

use opm::{
    config, file,
    helpers::{self, ColoredString},
    log,
    process::{
        ItemSingle, Runner, get_process_cpu_usage_with_children_from_process,
        get_process_memory_with_children, http, is_pid_alive,
    },
};

use tabled::{
    Table, Tabled,
    settings::{
        Color, Modify, Rotate, Width,
        object::{Columns, Rows, Segment},
        style::{BorderColor, Style},
        themes::Colorization,
    },
};

lazy_static! {
    static ref SCRIPT_EXTENSION_PATTERN: Regex =
        Regex::new(r"^[^\s]+\.(js|ts|mjs|cjs|py|py3|pyw|sh|bash|zsh|rb|pl|php|lua|r|R|go|java|kt|kts|scala|groovy|swift)(\s|$)").unwrap();
    static ref SIMPLE_PATH_PATTERN: Regex = Regex::new(r"^[a-zA-Z0-9]+(/[a-zA-Z0-9]+)*$").unwrap();
}

// Constants for real-time statistics display timing
pub(crate) const STATS_PRE_LIST_DELAY_MS: u64 = 100;

pub struct Internal<'i> {
    pub id: usize,
    pub runner: Runner,
    pub kind: String,
    pub server_name: &'i str,
}

impl<'i> Internal<'i> {
    pub fn create(
        mut self,
        script: &String,
        name: &Option<String>,
        watch: &Option<String>,
        max_memory: &Option<String>,
        silent: bool,
    ) -> Runner {
        let config = config::read();
        let name = match name {
            Some(name) => string!(name),
            None => string!(script.split_whitespace().next().unwrap_or_default()),
        };

        // Parse max_memory if provided
        let max_memory_bytes = match max_memory {
            Some(mem_str) => match helpers::parse_memory(mem_str) {
                Ok(bytes) => bytes,
                Err(err) => crashln!("{} {}", *helpers::FAIL, err),
            },
            None => 0,
        };

        if matches!(self.server_name, "internal" | "local") {
            // Check if script is a file path with an extension
            let script_to_run = if let Some(ext_start) = script.rfind('.') {
                let ext = &script[ext_start..];

                if SCRIPT_EXTENSION_PATTERN.is_match(script) {
                    // It's a script file with extension - determine the interpreter
                    let interpreter = match ext {
                        ".js" | ".ts" | ".mjs" | ".cjs" => config.runner.node.clone(),
                        ".py" | ".py3" | ".pyw" => "python3".to_string(),
                        ".sh" | ".bash" | ".zsh" => "bash".to_string(),
                        ".rb" => "ruby".to_string(),
                        ".pl" => "perl".to_string(),
                        ".php" => "php".to_string(),
                        ".lua" => "lua".to_string(),
                        ".r" | ".R" => "Rscript".to_string(),
                        ".go" => "go run".to_string(),
                        ".java" => "java".to_string(),
                        ".kt" | ".kts" => "kotlin".to_string(),
                        ".scala" => "scala".to_string(),
                        ".groovy" => "groovy".to_string(),
                        ".swift" => "swift".to_string(),
                        _ => "".to_string(),
                    };

                    if !interpreter.is_empty() {
                        format!("{} {}", interpreter, script)
                    } else {
                        script.clone()
                    }
                } else {
                    script.clone()
                }
            } else {
                // No extension, check old pattern for js/ts
                if SIMPLE_PATH_PATTERN.is_match(script) {
                    format!("{} {}", config.runner.node, script)
                } else {
                    script.clone()
                }
            };

            self.runner
                .start(&name, &script_to_run, file::cwd(), watch, max_memory_bytes)
                .save();
        } else {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(mut remote) => {
                        remote.start(&name, script, file::cwd(), watch, max_memory_bytes)
                    }
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name,
                )
            };
        }

        then!(
            !silent,
            println!(
                "{} Creating {}process with ({name})",
                *helpers::SUCCESS,
                self.kind
            )
        );
        then!(
            !silent,
            println!("{} {}Created ({name}) ✓", *helpers::SUCCESS, self.kind)
        );

        return self.runner;
    }

    pub fn restart(
        mut self,
        name: &Option<String>,
        watch: &Option<String>,
        reset_env: bool,
        silent: bool,
        increment_counter: bool,
    ) -> Runner {
        then!(
            !silent,
            println!(
                "{} Applying {}action restartProcess on ({})",
                *helpers::SUCCESS,
                self.kind,
                self.id
            )
        );

        if matches!(self.server_name, "internal" | "local") {
            let mut item = self.runner.get(self.id);

            match watch {
                Some(path) => item.watch(path),
                None => item.disable_watch(),
            }

            then!(reset_env, item.clear_env());

            name.as_ref()
                .map(|n| item.rename(n.trim().replace("\n", "")));
            item.restart(increment_counter);

            self.runner = item.get_runner().clone();
            self.runner.save();
        } else {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => {
                        let mut item = remote.get(self.id);

                        then!(reset_env, item.clear_env());

                        name.as_ref()
                            .map(|n| item.rename(n.trim().replace("\n", "")));
                        item.restart(increment_counter);
                    }
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                }
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        if !silent {
            println!(
                "{} Restarted {}({}) ✓",
                *helpers::SUCCESS,
                self.kind,
                self.id
            );
            log!("process started (id={})", self.id);
            
            // Emit event for CLI operation if on local server
            if matches!(self.server_name, "internal" | "local") {
                if let Some(process) = self.runner.info(self.id) {
                    let event_type = if increment_counter {
                        opm::events::EventType::ProcessRestart
                    } else {
                        opm::events::EventType::ProcessStart
                    };
                    super::events::emit_event(
                        event_type,
                        self.id,
                        &process.name,
                        &format!("Process '{}' {} via CLI", process.name, if increment_counter { "restarted" } else { "started" }),
                    );
                }
            }
        }

        return self.runner;
    }

    pub fn reload(mut self, silent: bool) -> Runner {
        then!(
            !silent,
            println!(
                "{} Applying {}action reloadProcess on ({})",
                *helpers::SUCCESS,
                self.kind,
                self.id
            )
        );

        if matches!(self.server_name, "internal" | "local") {
            let mut item = self.runner.get(self.id);
            item.reload(true); // Reload command should increment counter
            self.runner = item.get_runner().clone();
            self.runner.save();
        } else {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => {
                        let mut item = remote.get(self.id);
                        item.reload(true); // Reload command should increment counter
                    }
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                }
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        if !silent {
            println!(
                "{} Reloaded {}({}) ✓",
                *helpers::SUCCESS,
                self.kind,
                self.id
            );
            log!("process reloaded (id={})", self.id);
            
            // Emit event for CLI operation if on local server
            if matches!(self.server_name, "internal" | "local") {
                if let Some(process) = self.runner.info(self.id) {
                    super::events::emit_event(
                        opm::events::EventType::ProcessRestart,
                        self.id,
                        &process.name,
                        &format!("Process '{}' reloaded via CLI", process.name),
                    );
                }
            }
        }

        return self.runner;
    }

    pub fn stop(mut self, silent: bool) -> Runner {
        then!(
            !silent,
            println!(
                "{} Applying {}action stopProcess on ({})",
                *helpers::SUCCESS,
                self.kind,
                self.id
            )
        );

        if !matches!(self.server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                self.runner = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        let process_name = self.runner.info(self.id).map(|p| p.name.clone()).unwrap_or_default(); // Get name before stop
        let mut item = self.runner.get(self.id);
        item.stop();
        self.runner = item.get_runner().clone();
        self.runner.save();

        if !silent {
            println!("{} Stopped {}({}) ✓", *helpers::SUCCESS, self.kind, self.id);
            log!("process stopped {}(id={})", self.kind, self.id);
            
            // Emit event for CLI operation if on local server
            if matches!(self.server_name, "internal" | "local") {
                super::events::emit_event(
                    opm::events::EventType::ProcessStop,
                    self.id,
                    &process_name,
                    &format!("Process '{}' stopped via CLI", process_name),
                );
            }
        }

        return self.runner;
    }

    pub fn remove(mut self) {
        println!(
            "{} Applying {}action removeProcess on ({})",
            *helpers::SUCCESS,
            self.kind,
            self.id
        );

        if !matches!(self.server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                self.runner = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to remove (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        // Get process info before removal for event emission
        let process_name = self.runner.info(self.id).map(|p| p.name.clone());
        
        self.runner.remove(self.id);
        println!("{} Removed {}({}) ✓", *helpers::SUCCESS, self.kind, self.id);
        log!("process removed (id={})", self.id);
        
        // Emit event for CLI operation if on local server
        if matches!(self.server_name, "internal" | "local") {
            if let Some(name) = process_name {
                super::events::emit_event(
                    opm::events::EventType::ProcessDelete,
                    self.id,
                    &name,
                    &format!("Process '{}' deleted via CLI", name),
                );
            }
        }
    }

    pub fn flush(&mut self) {
        println!(
            "{} Applying {}action flushLogs on ({})",
            *helpers::SUCCESS,
            self.kind,
            self.id
        );

        if !matches!(self.server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                self.runner = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to remove (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        self.runner.flush(self.id);
        println!(
            "{} Flushed Logs {}({}) ✓",
            *helpers::SUCCESS,
            self.kind,
            self.id
        );
        log!("process logs cleaned (id={})", self.id);
    }

    pub fn info(&self, format: &String) {
        #[derive(Clone, Debug, Tabled)]
        struct Info {
            #[tabled(rename = "error log path ")]
            log_error: String,
            #[tabled(rename = "out log path")]
            log_out: String,
            #[tabled(rename = "cpu percent")]
            cpu_percent: String,
            #[tabled(rename = "memory usage")]
            memory_usage: String,
            #[tabled(rename = "memory limit")]
            memory_limit: String,
            #[tabled(rename = "path hash")]
            hash: String,
            #[tabled(rename = "watching")]
            watch: String,
            children: String,
            #[tabled(rename = "exec cwd")]
            path: String,
            #[tabled(rename = "script command ")]
            command: String,
            #[tabled(rename = "script id")]
            id: String,
            restarts: u64,
            uptime: String,
            pid: String,
            name: String,
            status: ColoredString,
        }

        impl Serialize for Info {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let trimmed_json = json!({
                     "id": &self.id.trim(),
                     "pid": &self.pid.trim(),
                     "name": &self.name.trim(),
                     "path": &self.path.trim(),
                     "restarts": &self.restarts,
                     "hash": &self.hash.trim(),
                     "watch": &self.watch.trim(),
                     "children": &self.children,
                     "uptime": &self.uptime.trim(),
                     "status": &self.status.0.trim(),
                     "log_out": &self.log_out.trim(),
                     "cpu": &self.cpu_percent.trim(),
                     "command": &self.command.trim(),
                     "mem": &self.memory_usage.trim(),
                     "mem_limit": &self.memory_limit.trim(),
                     "log_error": &self.log_error.trim(),
                });

                trimmed_json.serialize(serializer)
            }
        }

        let render_info = |data: Vec<Info>| {
            let table = Table::new(data.clone())
                .with(Rotate::Left)
                .with(Style::rounded().remove_horizontals())
                .with(Colorization::exact([Color::FG_CYAN], Columns::first()))
                .with(
                    Modify::new(Segment::all()).with(BorderColor::filled(Color::new(
                        "\x1b[38;2;45;55;72m",
                        "\x1b[39m",
                    ))),
                )
                .to_string();

            if let Ok(json) = serde_json::to_string(&data[0]) {
                match format.as_str() {
                    "raw" => println!("{:?}", data[0]),
                    "json" => println!("{json}"),
                    _ => {
                        println!(
                            "{}\n{table}\n",
                            format!("Describing {}process with id ({})", self.kind, self.id)
                                .on_bright_white()
                                .black()
                        );
                        println!(
                            " {}",
                            format!("Use `opm logs {} [--lines <num>]` to display logs", self.id)
                                .white()
                        );
                        println!(
                            " {}",
                            format!(
                                "Use `opm env {}`  to display environment variables",
                                self.id
                            )
                            .white()
                        );
                    }
                };
            };
        };

        if matches!(self.server_name, "internal" | "local") {
            if let Some(home) = home::home_dir() {
                let full_config = config::read();
                let config = full_config.runner;
                let mut runner = Runner::new();
                let item = runner.process(self.id);

                // Check if process actually exists before reporting as online
                // A process marked as running but with a non-existent PID should be shown as crashed
                let process_actually_running = item.running && is_pid_alive(item.pid);

                let mut memory_usage: Option<MemoryInfo> = None;
                let mut cpu_percent: Option<f64> = None;

                let path = file::make_relative(&item.path, &home)
                    .to_string_lossy()
                    .into_owned();
                let children = if item.children.is_empty() {
                    "none".to_string()
                } else {
                    format!("{:?}", item.children)
                };

                // Only fetch CPU and memory stats if process is actually running
                // Stopped or crashed processes should always show 0% CPU and 0b memory
                if process_actually_running {
                    // For shell scripts, use shell_pid to capture the entire process tree
                    let pid_for_monitoring = item.shell_pid.unwrap_or(item.pid);

                    if let Ok(process) = Process::new(pid_for_monitoring as u32) {
                        memory_usage = get_process_memory_with_children(pid_for_monitoring);
                        cpu_percent = Some(get_process_cpu_usage_with_children_from_process(
                            &process,
                            pid_for_monitoring,
                        ));
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

                let status = if process_actually_running {
                    "online   ".green().bold()
                } else if item.running {
                    // Process is marked as running but PID doesn't exist - it crashed
                    "crashed   ".red().bold()
                } else {
                    match item.crash.crashed {
                        true => "crashed   ",
                        false => "stopped   ",
                    }
                    .red()
                    .bold()
                };

                let memory_limit = if item.max_memory > 0 {
                    format!("{}  ", helpers::format_memory(item.max_memory))
                } else {
                    string!("none  ")
                };

                // Only count uptime when the process is actually running
                // Crashed or stopped processes should show "none" uptime
                let uptime = if process_actually_running {
                    format!("{}", helpers::format_duration(item.started))
                } else {
                    string!("none")
                };

                let data = vec![Info {
                    children,
                    cpu_percent,
                    memory_usage,
                    memory_limit,
                    id: string!(self.id),
                    restarts: if item.crash.crashed { 
                        item.crash.value 
                    } else { 
                        item.restarts 
                    },
                    name: item.name.clone(),
                    log_out: item.logs().out,
                    path: format!("{} ", path),
                    log_error: item.logs().error,
                    status: ColoredString(status),
                    pid: ternary!(
                        process_actually_running,
                        format!("{}", item.pid),
                        string!("n/a")
                    ),
                    command: format!(
                        "{} {} '{}'",
                        config.shell,
                        config.args.join(" "),
                        item.script
                    ),
                    hash: ternary!(
                        item.watch.enabled,
                        format!("{}  ", item.watch.hash),
                        string!("none  ")
                    ),
                    watch: ternary!(
                        item.watch.enabled,
                        format!("{path}/{}  ", item.watch.path),
                        string!("disabled  ")
                    ),
                    uptime,
                }];

                render_info(data)
            } else {
                crashln!("{} Impossible to get your home directory", *helpers::FAIL);
            }
        } else {
            let data: (opm::process::Process, Runner);
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                data = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(mut remote) => (remote.process(self.id).clone(), remote),
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };

            let (item, remote) = data;
            let remote = remote.remote.unwrap();
            let info = http::info(&remote, self.id);
            let path = item.path.to_string_lossy().into_owned();

            let status = if item.running {
                "online   ".green().bold()
            } else {
                match item.crash.crashed {
                    true => "crashed   ",
                    false => "stopped   ",
                }
                .red()
                .bold()
            };

            // Only count uptime when the process is actually running (not crashed or stopped)
            // For remote processes, we can't check is_pid_alive() since the PID is on a different machine,
            // so we trust the server's running and crashed flags instead of checking the PID directly
            let uptime_value = if item.running && !item.crash.crashed {
                format!("{}", helpers::format_duration(item.started))
            } else {
                string!("none")
            };

            if let Ok(info) = info {
                let stats = info.json::<ItemSingle>().unwrap().stats;
                let children = if item.children.is_empty() {
                    "none".to_string()
                } else {
                    format!("{:?}", item.children)
                };

                let cpu_percent = match stats.cpu_percent {
                    Some(percent) => format!("{percent:.2}%"),
                    None => string!("0.00%"),
                };

                let memory_usage = match stats.memory_usage {
                    Some(usage) => helpers::format_memory(usage.rss),
                    None => string!("0b"),
                };

                let memory_limit = if item.max_memory > 0 {
                    format!("{}  ", helpers::format_memory(item.max_memory))
                } else {
                    string!("none  ")
                };

                let data = vec![Info {
                    children,
                    cpu_percent,
                    memory_usage,
                    memory_limit,
                    id: string!(self.id),
                    path: path.clone(),
                    status: status.into(),
                    restarts: if item.crash.crashed { 
                        item.crash.value 
                    } else { 
                        item.restarts 
                    },
                    name: item.name.clone(),
                    pid: ternary!(
                        item.running && !item.crash.crashed,
                        format!("{pid}", pid = item.pid),
                        string!("n/a")
                    ),
                    log_out: format!("{}/{}-out.log", remote.config.log_path, item.name),
                    log_error: format!("{}/{}-error.log", remote.config.log_path, item.name),
                    hash: ternary!(
                        item.watch.enabled,
                        format!("{}  ", item.watch.hash),
                        string!("none  ")
                    ),
                    command: format!(
                        "{} {} '{}'",
                        remote.config.shell,
                        remote.config.args.join(" "),
                        item.script
                    ),
                    watch: ternary!(
                        item.watch.enabled,
                        format!("{path}/{}  ", item.watch.path),
                        string!("disabled  ")
                    ),
                    uptime: uptime_value,
                }];

                render_info(data)
            }
        }
    }

    pub fn logs(
        mut self,
        lines: &usize,
        follow: bool,
        filter: Option<&str>,
        errors_only: bool,
        stats: bool,
    ) {
        if !matches!(self.server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                self.runner = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };

            let item = self
                .runner
                .info(self.id)
                .unwrap_or_else(|| crashln!("{} Process ({}) not found", *helpers::FAIL, self.id));
            println!(
                "{}",
                format!("Showing last {lines} lines for {}process [{}] (change the value with --lines option)", self.kind, self.id).yellow()
            );

            for kind in vec!["error", "out"] {
                if errors_only && kind == "out" {
                    continue;
                }

                let logs = http::logs(&self.runner.remote.as_ref().unwrap(), self.id, kind);

                if let Ok(log) = logs {
                    if log.lines.is_empty() {
                        println!(
                            "{}",
                            format!("[OPM] No logs found for {}/{kind}", item.name).bright_black()
                        );
                        continue;
                    }

                    file::logs_internal_with_options(
                        log.lines, *lines, log.path, self.id, kind, &item.name, filter, stats,
                    )
                }
            }
        } else {
            let item = self
                .runner
                .info(self.id)
                .unwrap_or_else(|| crashln!("{} Process ({}) not found", *helpers::FAIL, self.id));

            if follow {
                println!(
                    "{}",
                    format!(
                        "Following logs for {}process [{}] (press Ctrl+C to exit)",
                        self.kind, self.id
                    )
                    .yellow()
                );
            } else {
                println!(
                    "{}",
                    format!("Showing last {lines} lines for {}process [{}] (change the value with --lines option)", self.kind, self.id).yellow()
                );
            }

            if errors_only {
                file::logs_with_options(item, *lines, "error", follow, filter, stats);
            } else {
                // When follow mode is enabled, we can't follow both logs simultaneously
                // So we'll only display initial content for both, then follow stdout
                if follow {
                    println!("{}", "\n--- Error Logs (last lines) ---".bright_red());
                    file::logs_with_options(item, *lines, "error", false, filter, false);
                    println!("{}", "\n--- Standard Output (following) ---".bright_green());
                    file::logs_with_options(item, *lines, "out", true, filter, stats);
                } else {
                    file::logs_with_options(item, *lines, "error", false, filter, stats);
                    file::logs_with_options(item, *lines, "out", false, filter, stats);
                }
            }
        }
    }

    pub fn env(mut self) {
        println!(
            "{}",
            format!("Showing env for {}process {}:\n", self.kind, self.id).bright_yellow()
        );

        if !matches!(self.server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                self.runner = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        let item = self.runner.process(self.id);
        item.env
            .iter()
            .for_each(|(key, value)| println!("{}: {}", key, value.green()));
    }

    pub fn get_command(mut self) {
        println!(
            "{}",
            format!(
                "Showing startup command for {}process {}:\n",
                self.kind, self.id
            )
            .bright_yellow()
        );

        if !matches!(self.server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                self.runner = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        let item = self.runner.process(self.id);
        let config = config::read().runner;
        let command = format!(
            "{} {} '{}'",
            config.shell,
            config.args.join(" "),
            item.script
        );

        println!("{}", command.green().bold());
        println!(
            "\n{}",
            "You can use this command to start the process manually:".dimmed()
        );
        println!("{}", command.white());
    }

    pub fn adjust(mut self, command: &Option<String>, name: &Option<String>) {
        println!(
            "{} Adjusting {}process ({})",
            *helpers::SUCCESS,
            self.kind,
            self.id
        );

        if !matches!(self.server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(self.server_name) {
                self.runner = match Runner::connect(self.server_name.into(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to connect (name={}, address={})",
                        *helpers::FAIL,
                        self.server_name,
                        server.address
                    ),
                };
            } else {
                crashln!(
                    "{} Server '{}' does not exist",
                    *helpers::FAIL,
                    self.server_name
                )
            };
        }

        // Check if at least one parameter is provided
        if command.is_none() && name.is_none() {
            crashln!(
                "{} At least one of --command or --name must be provided",
                *helpers::FAIL
            );
        }

        let process = self.runner.process(self.id);

        // Update command if provided
        if let Some(new_command) = command {
            println!(
                "  {} Updating command from '{}' to '{}'",
                *helpers::SUCCESS,
                process.script,
                new_command
            );
            process.script = new_command.clone();
        }

        // Update name if provided
        if let Some(new_name) = name {
            println!(
                "  {} Updating name from '{}' to '{}'",
                *helpers::SUCCESS,
                process.name,
                new_name
            );
            process.name = new_name.clone();
        }

        self.runner.save();

        println!(
            "{} Adjusted {}({}) ✓",
            *helpers::SUCCESS,
            self.kind,
            self.id
        );
        log!("process adjusted (id={})", self.id);
    }

    pub fn save(server_name: &String) {
        if !matches!(&**server_name, "internal" | "local") {
            crashln!("{} Cannot force save on remote servers", *helpers::FAIL)
        }

        println!("{} Saved current processes to dumpfile", *helpers::SUCCESS);
        Runner::new().save();
    }

    pub fn restore(server_name: &String) {
        let mut runner = Runner::new();
        let (kind, list_name) = super::format(server_name);

        if !matches!(&**server_name, "internal" | "local") {
            crashln!("{} Cannot restore on remote servers", *helpers::FAIL)
        }

        println!("{} Starting restore process...", *helpers::SUCCESS);

        // Get restore cleanup configuration
        let config = config::read();
        let restore_cleanup = config.daemon.restore_cleanup.as_ref();
        
        // Clear process logs if enabled (default: true)
        let should_cleanup_process_logs = restore_cleanup
            .map(|rc| rc.process_logs)
            .unwrap_or(true);
        
        if should_cleanup_process_logs {
            let log_path = &config.runner.log_path;
            if file::Exists::check(log_path).folder() {
                // Remove all log files in the log directory
                if let Ok(entries) = fs::read_dir(log_path) {
                    for entry in entries.flatten() {
                        if let Ok(file_type) = entry.file_type() {
                            if file_type.is_file() {
                                let path = entry.path();
                                if let Some(ext) = path.extension() {
                                    if ext == "log" {
                                        if let Err(e) = fs::remove_file(&path) {
                                            ::log::warn!("Failed to delete process log {:?}: {}", path, e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // Clear daemon log if enabled (default: true)
        let should_cleanup_daemon_log = restore_cleanup
            .map(|rc| rc.daemon_log)
            .unwrap_or(true);
        
        if should_cleanup_daemon_log {
            if let Some(path) = home::home_dir() {
                let daemon_log_path = path.join(".opm").join("daemon.log");
                if daemon_log_path.exists() {
                    if let Err(e) = fs::remove_file(&daemon_log_path) {
                        ::log::warn!("Failed to delete daemon.log: {}", e);
                    }
                }
            }
        }
        
        // Clear agent log if enabled (default: true)
        let should_cleanup_agent_log = restore_cleanup
            .map(|rc| rc.agent_log)
            .unwrap_or(true);
        
        if should_cleanup_agent_log {
            if let Some(path) = home::home_dir() {
                let agent_log_path = path.join(".opm").join("agent.log");
                if agent_log_path.exists() {
                    if let Err(e) = fs::remove_file(&agent_log_path) {
                        ::log::warn!("Failed to delete agent.log: {}", e);
                    }
                }
            }
        }

        // Reset restart and crash counters after restore for ALL processes in the system
        // This gives each process a fresh start after system restore/reboot
        // Both running and stopped processes get their counters reset so they have
        // full restart attempts available
        // Do this BEFORE checking if there are processes to restore, so counters are reset
        // even when all processes are stopped
        let all_process_ids: Vec<usize> = runner.items().keys().copied().collect();
        for id in all_process_ids {
            runner.reset_counters(id);
        }
        runner.save();

        let mut restored_ids = Vec::new();
        let mut failed_ids = Vec::new();

        // Restore processes that were running OR crashed before daemon stopped
        // This includes:
        // 1. Processes with running=true (were running normally)
        // 2. Processes with running=false AND crashed=true (were crashed and stopped by daemon)
        // Do NOT restore processes that were manually stopped (running=false, crashed=false)
        let processes_to_restore: Vec<(usize, String, bool, bool)> = Runner::new()
            .list()
            .filter_map(|(id, p)| {
                if p.running || (p.crash.crashed && !p.running) {
                    Some((*id, p.name.clone(), p.running, p.crash.crashed))
                } else {
                    None
                }
            })
            .collect();

        if processes_to_restore.is_empty() {
            println!("{} No processes to restore", *helpers::SUCCESS);
            Internal::list(&string!("default"), &list_name);
            return;
        }

        for (id, name, was_running, was_crashed) in &processes_to_restore {
            // Set process back to running before restoring since we reset crash counters
            // This ensures crashed processes get a fresh start
            if *was_crashed && !*was_running {
                runner.process(*id).running = true;
                runner.save();
            }
            
            // status_str is currently unused but kept for potential future logging
            let _status_str = if *was_crashed {
                "crashed"
            } else if *was_running {
                "running"
            } else {
                "stopped"
            };

            runner = Internal {
                id: *id,
                server_name,
                kind: kind.clone(),
                runner: runner.clone(),
            }
            .restart(&None, &None, false, true, false);

            // Wait a short period to allow processes to start up before checking PID
            // This prevents false crash detection for processes that take time to initialize
            // Shell scripts in particular need time for the shell to spawn the actual process
            std::thread::sleep(std::time::Duration::from_millis(150));

            // Check if the restart was successful
            if let Some(process) = runner.info(*id) {
                // Verify the process is actually running using the same logic as daemon
                // Use shell_pid if available (for shell scripts), otherwise use main pid
                // This ensures consistent behavior between restore and daemon monitoring
                let pid_to_check = process.shell_pid.unwrap_or(process.pid);
                let process_alive = opm::process::is_pid_alive(pid_to_check);

                if process.running && process_alive {
                    restored_ids.push(*id);
                } else {
                    failed_ids.push((*id, name.clone()));
                    println!(
                        "{} Failed to restore process '{}' (id={}) - process is not running",
                        *helpers::FAIL,
                        name,
                        id
                    );
                    // Mark process as crashed so daemon can pick it up for auto-restart
                    // Keep running=true (set_crashed doesn't change it) so daemon will attempt restart
                    // Don't increment crash counter here - let the daemon do it when it detects the crash
                    runner.set_crashed(*id);
                    // Don't auto-save here - save will happen at the end of restore
                }
            } else {
                failed_ids.push((*id, name.clone()));
                println!(
                    "{} Failed to restore process '{}' (id={}) - process not found",
                    *helpers::FAIL,
                    name,
                    id
                );
                // Mark process as crashed so daemon can pick it up for auto-restart
                // Keep running=true (set_crashed doesn't change it) so daemon will attempt restart
                // Don't increment crash counter here - let the daemon do it when it detects the crash
                runner.set_crashed(*id);
                // Don't auto-save here - save will happen at the end of restore
            }
        }

        Internal::list(&string!("default"), &list_name);
    }

    pub fn list(format: &String, server_name: &String) {
        // Check permissions for remote operations
        super::check_remote_permission(server_name);

        let render_list = |runner: &mut Runner, internal: bool| {
            let mut processes: Vec<ProcessItem> = Vec::new();

            #[derive(Tabled, Debug)]
            struct ProcessItem {
                id: ColoredString,
                name: String,
                pid: String,
                uptime: String,
                #[tabled(rename = "↺")]
                restarts: String,
                status: ColoredString,
                cpu: String,
                mem: String,
                #[tabled(rename = "watching")]
                watch: String,
            }

            impl serde::Serialize for ProcessItem {
                fn serialize<S: serde::Serializer>(
                    &self,
                    serializer: S,
                ) -> Result<S::Ok, S::Error> {
                    let trimmed_json = json!({
                        "cpu": &self.cpu.trim(),
                        "mem": &self.mem.trim(),
                        "id": &self.id.0.trim(),
                        "pid": &self.pid.trim(),
                        "name": &self.name.trim(),
                        "watch": &self.watch.trim(),
                        "uptime": &self.uptime.trim(),
                        "status": &self.status.0.trim(),
                        "restarts": &self.restarts.trim(),
                    });
                    trimmed_json.serialize(serializer)
                }
            }

            if runner.is_empty() {
                println!("{} Process table empty", *helpers::SUCCESS);
            } else {
                for (id, item) in runner.items() {
                    // Check if process actually exists before reporting as online
                    // A process marked as running but with a non-existent PID should be shown as crashed
                    let process_actually_running = item.running && is_pid_alive(item.pid);

                    let mut cpu_percent: String = string!("0.00%");
                    let mut memory_usage: String = string!("0b");

                    // Only fetch CPU and memory stats if process is actually running
                    // Stopped or crashed processes should always show 0% CPU and 0b memory
                    if process_actually_running {
                        if internal {
                            let mut usage_internals: (Option<f64>, Option<MemoryInfo>) =
                                (None, None);

                            // For shell scripts, use shell_pid to capture the entire process tree
                            let pid_for_monitoring = item.shell_pid.unwrap_or(item.pid);

                            if let Ok(process) = Process::new(pid_for_monitoring as u32) {
                                usage_internals = (
                                    Some(get_process_cpu_usage_with_children_from_process(
                                        &process,
                                        pid_for_monitoring,
                                    )),
                                    get_process_memory_with_children(pid_for_monitoring),
                                );
                            }

                            cpu_percent = match usage_internals.0 {
                                Some(percent) => format!("{:.2}%", percent),
                                None => string!("0.00%"),
                            };

                            memory_usage = match usage_internals.1 {
                                Some(usage) => helpers::format_memory(usage.rss),
                                None => string!("0b"),
                            };
                        } else {
                            let info = http::info(&runner.remote.as_ref().unwrap(), id);

                            if let Ok(info) = info {
                                let stats = info.json::<ItemSingle>().unwrap().stats;

                                cpu_percent = match stats.cpu_percent {
                                    Some(percent) => format!("{:.2}%", percent),
                                    None => string!("0.00%"),
                                };

                                memory_usage = match stats.memory_usage {
                                    Some(usage) => helpers::format_memory(usage.rss),
                                    None => string!("0b"),
                                };
                            }
                        }
                    }

                    let status = if process_actually_running {
                        "online   ".green().bold()
                    } else if item.running {
                        // Process is marked as running but PID doesn't exist - it crashed
                        "crashed   ".red().bold()
                    } else {
                        match item.crash.crashed {
                            true => "crashed   ",
                            false => "stopped   ",
                        }
                        .red()
                        .bold()
                    };

                    // Only count uptime when the process is actually running
                    // Crashed or stopped processes should show "none" uptime
                    let uptime = if process_actually_running {
                        format!("{}  ", helpers::format_duration(item.started))
                    } else {
                        string!("none  ")
                    };

                    processes.push(ProcessItem {
                        status: status.into(),
                        cpu: format!("{cpu_percent}   "),
                        mem: format!("{memory_usage}   "),
                        id: id.to_string().cyan().bold().into(),
                        restarts: format!("{}  ", item.crash.value),
                        name: format!("{}   ", item.name.clone()),
                        pid: ternary!(
                            process_actually_running,
                            format!("{}  ", item.pid),
                            string!("n/a  ")
                        ),
                        watch: ternary!(
                            item.watch.enabled,
                            format!("{}  ", item.watch.path),
                            string!("disabled  ")
                        ),
                        uptime,
                    });
                }

                let table = Table::new(&processes)
                    .with(Style::rounded().remove_verticals())
                    .with(
                        Modify::new(Segment::all()).with(BorderColor::filled(Color::new(
                            "\x1b[38;2;45;55;72m",
                            "\x1b[39m",
                        ))),
                    )
                    .with(Colorization::exact([Color::FG_BRIGHT_CYAN], Rows::first()))
                    .with(Modify::new(Columns::single(1)).with(Width::truncate(40).suffix("... ")))
                    .to_string();

                if let Ok(json) = serde_json::to_string(&processes) {
                    match format.as_str() {
                        "raw" => println!("{:?}", processes),
                        "json" => println!("{json}"),
                        "default" => println!("{table}"),
                        _ => {}
                    };
                };
            }
        };

        if let Some(servers) = config::servers().servers {
            let mut failed: Vec<(String, String)> = vec![];

            if let Some(server) = servers.get(server_name) {
                match Runner::connect(server_name.clone(), server.get(), true) {
                    Some(mut remote) => render_list(&mut remote, false),
                    None => println!(
                        "{} Failed to fetch (name={server_name}, address={})",
                        *helpers::FAIL,
                        server.address
                    ),
                }
            } else {
                if matches!(&**server_name, "internal" | "all" | "global" | "local") {
                    if *server_name == "all" || *server_name == "global" {
                        println!("{} Internal daemon", *helpers::SUCCESS);
                    }
                    render_list(&mut Runner::new(), true);
                } else {
                    crashln!("{} Server '{server_name}' does not exist", *helpers::FAIL);
                }
            }

            if *server_name == "all" || *server_name == "global" {
                for (name, server) in servers {
                    match Runner::connect(name.clone(), server.get(), true) {
                        Some(mut remote) => render_list(&mut remote, false),
                        None => failed.push((name, server.address)),
                    }
                }
            }

            if !failed.is_empty() {
                println!("{} Failed servers:", *helpers::FAIL);
                failed.iter().for_each(|server| {
                    println!(
                        " {} {} {}",
                        "-".yellow(),
                        format!("{}", server.0),
                        format!("[{}]", server.1).white()
                    )
                });
            }
        } else {
            render_list(&mut Runner::new(), true);
        }
    }
}
