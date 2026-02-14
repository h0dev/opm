use colored::Colorize;
use lazy_static::lazy_static;
use macros_rs::{crashln, string, ternary, then};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use opm::process::{unix::NativeProcess as Process, MemoryInfo};
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
        get_process_cpu_usage_with_children_from_process, get_process_memory_with_children, http,
        is_any_descendant_alive, is_pid_alive, ItemSingle, Runner,
    },
};

use tabled::{
    settings::{
        object::{Columns, Rows, Segment},
        style::{BorderColor, Style},
        themes::Colorization,
        Color, Modify, Rotate, Width,
    },
    Table, Tabled,
};

lazy_static! {
    static ref SCRIPT_EXTENSION_PATTERN: Regex =
        Regex::new(r"^[^\s]+\.(js|ts|mjs|cjs|py|py3|pyw|sh|bash|zsh|rb|pl|php|lua|r|R|go|java|kt|kts|scala|groovy|swift)(\s|$)").unwrap();
    static ref SIMPLE_PATH_PATTERN: Regex = Regex::new(r"^[a-zA-Z0-9]+(/[a-zA-Z0-9]+)*$").unwrap();
}

/// Extract a search pattern from a command for process adoption during restore
/// Looks for distinctive parts like JAR files, script names, executables
/// 
/// NOTE: This is a copy of the logic from daemon/mod.rs::extract_search_pattern to avoid circular dependencies.
/// TODO: Consider moving this to a shared utility module (e.g., opm::process::search_pattern) that both
/// daemon and cli can import, eliminating the duplication while avoiding circular dependencies.
fn extract_search_pattern_for_restore(command: &str) -> String {
    // Look for patterns that uniquely identify the process
    // Priority: JAR files, then .py/.js/.sh files, then first word
    
    // Check for JAR files (e.g., "java -jar Stirling-PDF.jar")
    if let Some(jar_pos) = command.find(".jar") {
        // Find the start of the filename (after last space or slash)
        let before_jar = &command[..jar_pos];
        if let Some(start) = before_jar.rfind(|c: char| c == ' ' || c == '/') {
            let end = (jar_pos + 4).min(command.len());
            if start + 1 < end {
                let jar_name = &command[start+1..end];
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
                    let script_name = &command[start+1..end];
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

fn ensure_daemon_running() {
    use global_placeholders::global;

    let socket_path = global!("opm.socket");
    if opm::socket::is_daemon_running(&socket_path) {
        return;
    }

    println!("{} Starting OPM daemon...", *helpers::SUCCESS);
    let config = config::read();
    let api_enabled = config.daemon.web.api;
    let webui_enabled = config.daemon.web.ui;

    crate::daemon::restart(&api_enabled, &webui_enabled, false);

    let max_retries = 20;
    let mut retry_count = 0;
    let mut socket_ready = false;

    loop {
        if opm::socket::is_daemon_running(&socket_path) {
            socket_ready = true;
            break;
        }

        if retry_count >= max_retries {
            break;
        }

        let wait_ms = 200 + (retry_count * 100);
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));
        retry_count += 1;
    }

    if !socket_ready {
        eprintln!(
            "{} Warning: Daemon socket not ready after initial attempts, retrying...",
            *helpers::WARN
        );

        let additional_retries = 5;
        for i in 0..additional_retries {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if opm::socket::is_daemon_running(&socket_path) {
                socket_ready = true;
                break;
            }

            if i == additional_retries - 1 {
                eprintln!(
                    "{} Warning: Daemon socket may not be ready after extended wait",
                    *helpers::WARN
                );
            }
        }
    }

    if socket_ready {
        println!("{} OPM daemon started", *helpers::SUCCESS);
    } else {
        crashln!(
            "{} Failed to connect to OPM daemon socket after {} total retries\n{}\n{}",
            *helpers::FAIL,
            max_retries + 5,
            "The daemon may have failed to start or the socket is not accessible.".white(),
            "Try running 'opm daemon --no-daemonize' to see error messages.".white()
        );
    }
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
            ensure_daemon_running();
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
                .start(&name, &script_to_run, file::cwd(), watch, max_memory_bytes);
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
            ensure_daemon_running();
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
                        &format!(
                            "Process '{}' {} via CLI",
                            process.name,
                            if increment_counter {
                                "restarted"
                            } else {
                                "started"
                            }
                        ),
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

        let process_name = self
            .runner
            .info(self.id)
            .map(|p| p.name.clone())
            .unwrap_or_default(); // Get name before stop
        let mut item = self.runner.get(self.id);
        item.stop();
        self.runner = item.get_runner().clone();

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

        // Freeze process before removal to prevent auto-restart during deletion
        // Give it 10 seconds freeze window - more than enough for removal to complete
        self.runner.freeze(self.id, 10);

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
                .with(Style::modern().remove_horizontals())
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
                // Check both the actual PID and shell PID (if present) to determine if process is alive.
                // For shell-wrapped processes, either PID being alive means the process is running.
                // This is consistent with the daemon monitoring logic and prevents false crash detection.
                let crash_detection_enabled = full_config.daemon.crash_detection;
                let pid_valid = item.pid > 0;
                let main_pid_alive = pid_valid && is_pid_alive(item.pid);
                let shell_pid_alive = item.shell_pid.map_or(false, |pid| is_pid_alive(pid));
                let pid_alive = main_pid_alive || shell_pid_alive;
                let process_actually_running = item.running && pid_alive;
                // Process is crashed if:
                // 1. It's marked as running but not actually running (consistent with opm ls)
                // 2. The crash.crashed flag is explicitly set by the daemon
                // This ensures consistent status display between opm ls and opm info
                let crashed_while_running = item.running && !process_actually_running && crash_detection_enabled;
                let crashed_by_flag = item.crash.crashed && crash_detection_enabled;

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
                } else if item.errored {
                    // Process reached restart limit - show as errored
                    "errored  ".red().bold()
                } else if crashed_while_running || crashed_by_flag {
                    // Check if process is in restart cooldown period
                    if item.is_in_restart_cooldown() {
                        // Process is in cooldown - show as waiting/restarting
                        "waiting  ".yellow().bold()
                    } else {
                        // Process crashed: either marked as running but not alive, or crash flag set
                        "crashed   ".red().bold()
                    }
                } else {
                    // Process is not running (running=false) - always show as stopped
                    // This ensures stopped processes display correctly even if they have
                    // stale crash.crashed flags from old dumps or failed restore operations
                    "stopped   ".red().bold()
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
                    // Always show restarts counter
                    // restarts is persisted and provides accurate restart count
                    restarts: item.restarts,
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
            } else if item.errored {
                "errored  ".red().bold()
            } else {
                // Process is not running (running=false) - always show as stopped
                // This ensures stopped processes display correctly even if they have
                // stale crash.crashed flags from old dumps or failed restore operations
                "stopped   ".red().bold()
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
                    // Always show restarts counter
                    // restarts is persisted and provides accurate restart count
                    restarts: item.restarts,
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

        // Verify process exists before attempting to freeze
        if !self.runner.exists(self.id) {
            crashln!("{} Process ({}) not found", *helpers::FAIL, self.id);
        }

        // Freeze process during editing to prevent auto-restart conflicts
        // Give it 5 seconds freeze window - enough for edit to complete
        self.runner.freeze(self.id, 5);

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

        // Save changes and unfreeze
        self.runner.save();
        self.runner.unfreeze(self.id);

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
        Runner::new().save_permanent();
    }

    pub fn restore(server_name: &String) {
        let (kind, _list_name) = super::format(server_name);

        if !matches!(&**server_name, "internal" | "local") {
            crashln!("{} Cannot restore on remote servers", *helpers::FAIL)
        }



        // Kill any running processes before restoring to ensure clean state
        // This prevents port conflicts and resource issues
        // Note: This is primarily for backward compatibility with old dump files
        // that still have PIDs saved. New dumps won't have PIDs (they're skipped).
        //
        // OPTIMIZATION: We no longer load the full dump file into RAM here.
        // Instead, we rely on the daemon's init_on_startup() to handle any
        // cleanup needed. This avoids unnecessary memory usage and file I/O
        // during restore operations, especially for large dump files.
        // The daemon will properly handle process state when it starts.

        // Read config to check if API/WebUI should be enabled (before daemon operations)
        let config = config::read();
        let api_enabled = config.daemon.web.api;
        let webui_enabled = config.daemon.web.ui;

        // Reset daemon first to compress process IDs and clean state
        // This must happen BEFORE starting daemon to ensure clean startup
        crate::daemon::reset();

        // Always restart daemon (stop if running, then start)
        // This ensures daemon starts fresh with reset state
        crate::daemon::restart(&api_enabled, &webui_enabled, false);

        // Wait for daemon socket to be ready before proceeding
        // Use socket readiness check instead of fixed sleep
        use global_placeholders::global;
        let socket_path = global!("opm.socket");
        // Increased max retries to allow more time for daemon initialization
        // This is particularly important on slower systems or during high load
        let max_retries = 20; // Increased from 10 to give daemon more time to start
        let mut retry_count = 0;
        let mut socket_ready = false;

        loop {
            if opm::socket::is_daemon_running(&socket_path) {
                socket_ready = true;
                break;
            }

            if retry_count >= max_retries {
                break;
            }

            // Start with 200ms and increase by 100ms each retry
            // (matches SOCKET_RETRY_INITIAL_MS and SOCKET_RETRY_INCREMENT_MS in main.rs)
            let wait_ms = 200 + (retry_count * 100);
            std::thread::sleep(std::time::Duration::from_millis(wait_ms));
            retry_count += 1;
        }

        if !socket_ready {
            // Socket not ready after initial retries, but daemon may still be starting
            // Try a few more times with longer waits before giving up
            eprintln!(
                "{} Warning: Daemon socket not ready after initial attempts, retrying...",
                *helpers::WARN
            );

            // Additional retry loop with longer waits (1 second each)
            let additional_retries = 5;
            for i in 0..additional_retries {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if opm::socket::is_daemon_running(&socket_path) {
                    socket_ready = true;
                    break;
                }

                if i == additional_retries - 1 {
                    eprintln!(
                        "{} Warning: Daemon socket may not be ready after extended wait",
                        *helpers::WARN
                    );
                }
            }
        }

        // Print success message only once, after all retries are complete
        if socket_ready {
            println!("{} OPM daemon started", *helpers::SUCCESS);
        } else {
            // Socket still not ready after all retries - fail with clear error message
            crashln!(
                "{} Failed to connect to OPM daemon socket after {} total retries\n{}\n{}",
                *helpers::FAIL,
                max_retries + 5,
                "The daemon may have failed to start or the socket is not accessible.".white(),
                "Try running 'opm daemon --no-daemonize' to see error messages.".white()
            );
        }

        // Clean up all stale timestamp files before restore to ensure fresh start
        // This prevents old timestamp files from interfering with crash detection
        crate::daemon::cleanup_all_timestamp_files();

        // Set restore in progress flag to prevent daemon from auto-starting processes
        // This prevents race condition where daemon and restore both try to start the same process
        crate::daemon::set_restore_in_progress();

        // Load permanent dump into daemon memory for restore operations
        match opm::socket::send_request(&socket_path, opm::socket::SocketRequest::LoadPermanent) {
            Ok(opm::socket::SocketResponse::Success) => {}
            Ok(opm::socket::SocketResponse::Error(message)) => {
                crashln!(
                    "{} Failed to load dump into daemon memory: {message}",
                    *helpers::FAIL
                )
            }
            Ok(_) => crashln!("{} Unexpected response from daemon", *helpers::FAIL),
            Err(e) => crashln!("{} Failed to contact daemon: {e}", *helpers::FAIL),
        }

        // Read state from daemon only (no disk fallback) since daemon is guaranteed to be running
        // If daemon connection fails here, it may be due to daemon still initializing
        // The socket readiness checks above should have given the daemon enough time to start
        let mut runner = match Runner::new_from_daemon() {
            Ok(runner) => runner,
            Err(e) => {
                crashln!(
                    "{} Failed to read process state from daemon: {}\n{}",
                    *helpers::FAIL,
                    e,
                    "Make sure the daemon is running properly.".white()
                );
            }
        };

        // Get restore cleanup configuration
        let config = config::read();
        let restore_cleanup = config.daemon.restore_cleanup.as_ref();

        // Clear process logs if enabled (default: true)
        let should_cleanup_process_logs = restore_cleanup.map(|rc| rc.process_logs).unwrap_or(true);

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
                                            ::log::warn!(
                                                "Failed to delete process log {:?}: {}",
                                                path,
                                                e
                                            );
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
        let should_cleanup_daemon_log = restore_cleanup.map(|rc| rc.daemon_log).unwrap_or(true);

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
        let should_cleanup_agent_log = restore_cleanup.map(|rc| rc.agent_log).unwrap_or(true);

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

        // Clear opm log if enabled (default: true)
        let should_cleanup_opm_log = restore_cleanup.map(|rc| rc.opm_log).unwrap_or(true);

        if should_cleanup_opm_log {
            if let Some(path) = home::home_dir() {
                let opm_log_path = path.join(".opm").join("opm.log");
                if opm_log_path.exists() {
                    if let Err(e) = fs::remove_file(&opm_log_path) {
                        ::log::warn!("Failed to delete opm.log: {}", e);
                    }
                }
            }
        }

        let mut restored_ids = Vec::new();
        let mut failed_ids = Vec::new();

        // Restore processes that were running before daemon stopped
        // Now we restore all processes that have running=true, regardless of crashed state
        // This preserves the original state and only resets counters
        // Do NOT restore processes that were manually stopped (running=false)
        let processes_to_restore: Vec<(usize, String, bool, bool)> = runner
            .list()
            .filter_map(|(id, p)| {
                // Only restore processes that have running=true
                // This includes both:
                // 1. Clean running processes (running=true, crashed=false)
                // 2. Processes that crashed while running (running=true, crashed=true)
                // We intentionally check ONLY p.running to include both cases.
                // In both cases, restore will restart them. After successful restart,
                // the daemon will clear the crashed flag if the process stays alive.
                if p.running {
                    Some((*id, p.name.clone(), p.running, p.crash.crashed))
                } else {
                    // Skip processes with running=false (manually stopped processes)
                    None
                }
            })
            .collect();

        if processes_to_restore.is_empty() {
            println!("{} Success: 0 processes restored.", *helpers::SUCCESS);
            return;
        }

        // FIX #3: UPTIME JUMP & SYNC
        // Capture a single timestamp for all restored processes to synchronize uptimes
        // This ensures processes started in the same batch have the same start time
        use chrono::Utc;
        let batch_start_time = Utc::now();

        // PARALLEL RESTORATION: Spawn all processes concurrently
        // This dramatically reduces restore time for multiple processes
        use std::sync::{Arc, Mutex};
        use std::thread;
        
        let runner_arc = Arc::new(Mutex::new(runner.clone()));
        let mut handles = vec![];
        
        for (id, name, _was_running, _was_crashed) in &processes_to_restore {
            let id = *id;
            let name = name.clone();
            let server_name = server_name.clone();
            let kind = kind.clone();
            let runner_arc_clone = Arc::clone(&runner_arc);
            
            let handle = thread::spawn(move || {
                // FIX #4: SYSTEM-WIDE "RUNNING" CHECK (Anti-Duplication)
                // Before spawning a new process, check if one is already running
                let mut runner_guard = match runner_arc_clone.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => {
                        ::log::error!("Mutex poisoned during restore for process {}: {}", id, poisoned);
                        // Recover from poison by taking ownership of the data
                        poisoned.into_inner()
                    }
                };
                
                // Check if a process matching this command is already running in the system
                if runner_guard.exists(id) {
                    let process = runner_guard.process(id);
                    let search_identifier = extract_search_pattern_for_restore(&process.script);
                    
                    if !search_identifier.is_empty() {
                        if let Some(existing_pid) = opm::process::find_process_by_command(&search_identifier) {
                            ::log::info!(
                                "Found existing process for '{}' (id={}) with PID {}, attaching instead of spawning",
                                name,
                                id,
                                existing_pid
                            );
                            
                            // Attach to the existing process instead of spawning new one
                            process.pid = existing_pid;
                            process.shell_pid = None; // No shell wrapper for existing process
                            process.running = true;
                            process.crash.crashed = false;
                            
                            // Update session ID for the attached process
                            #[cfg(any(target_os = "linux", target_os = "macos"))]
                            {
                                process.session_id = opm::process::unix::get_session_id(existing_pid as i32);
                            }
                            
                            // Save the state with attached PID
                            runner_guard.save_direct();
                            
                            // Return early - no need to spawn
                            return (id, name);
                        }
                    }
                }
                
                // No existing process found, proceed with normal restart
                *runner_guard = Internal {
                    id,
                    server_name: &server_name,
                    kind: kind.clone(),
                    runner: runner_guard.clone(),
                }
                .restart(&None, &None, false, true, false);
                
                // Create timestamp file for this restore action
                if let Err(e) = opm::process::write_action_timestamp(id) {
                    ::log::warn!(
                        "Failed to create action timestamp file for process {}: {}",
                        id,
                        e
                    );
                }
                
                // Return the ID and name for tracking
                (id, name)
            });
            
            handles.push(handle);
        }
        
        // Wait for all spawns to complete
        let spawn_results: Vec<(usize, String)> = handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .collect();
        
        // Get the updated runner state after all parallel spawns
        runner = match Arc::try_unwrap(runner_arc) {
            Ok(mutex) => match mutex.into_inner() {
                Ok(inner) => inner,
                Err(poisoned) => {
                    ::log::warn!("Mutex poisoned during restore, recovering data");
                    poisoned.into_inner()
                }
            },
            Err(arc) => match arc.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => {
                    ::log::warn!("Mutex poisoned during restore, recovering data");
                    poisoned.into_inner().clone()
                }
            },
        };
        
        // Wait 1 second for all processes to stabilize after parallel spawning
        // This gives the OS time to register all process trees before verification
        std::thread::sleep(std::time::Duration::from_secs(1));
        
        // FIX #3: Apply synchronized start time to all successfully restored processes
        // This ensures processes started in the same batch show consistent uptimes
        for (id, _name) in &spawn_results {
            if runner.exists(*id) {
                let process = runner.process(*id);
                if process.running && process.pid > 0 {
                    // Only update start time for successfully running processes
                    process.started = batch_start_time;
                }
            }
        }
        
        // Verify each process started successfully
        for (id, name) in spawn_results {
            // Check if the restart was successful
            if let Some(process) = runner.info(id) {
                // Verify the process is actually running using the same logic as daemon
                // Check the actual process PID first (long-running), then shell_pid (transient)
                // This ensures consistent behavior between restore and daemon monitoring
                // The daemon checks: is_pid_alive(item.pid) || shell_alive
                // So we should also check pid first, then shell_pid
                let process_alive = opm::process::is_process_actually_alive(process.pid, process.shell_pid);

                // Small startup grace period to avoid falsely reporting as crashed
                let recently_started =
                    (chrono::Utc::now() - process.started) < chrono::Duration::seconds(2);

                if process.running && process_alive {
                    restored_ids.push(id);
                } else if process.running && recently_started {
                    // Still starting up - give it the benefit of the doubt for initial report
                    // The daemon will verify and handle any issues during its monitoring cycle
                    restored_ids.push(id);
                } else {
                    failed_ids.push((id, name.clone()));
                    // Mark process as crashed so daemon can pick it up for auto-restart
                    // Keep running=true (set_crashed doesn't change it) so daemon will attempt restart
                    // Don't increment crash counter here - let the daemon do it when it detects the crash
                    runner.set_crashed(id);
                    // Don't auto-save here - save will happen at the end of restore
                }
            } else {
                failed_ids.push((id, name.clone()));
                println!(
                    "{} Failed to restore process '{}' (id={}) - process not found",
                    *helpers::FAIL,
                    name,
                    id
                );
                // Mark process as crashed so daemon can pick it up for auto-restart
                // Keep running=true (set_crashed doesn't change it) so daemon will attempt restart
                // Don't increment crash counter here - let the daemon do it when it detects the crash
                runner.set_crashed(id);
                // Don't auto-save here - save will happen at the end of restore
            }
        }

        // Save final state after restore attempts to persist crashed process states
        // This ensures processes that failed to restore (via set_crashed() calls) are properly
        // marked as crashed in permanent storage for daemon monitoring
        runner.save_permanent();

        // Clear restore in progress flag to allow daemon to resume normal operations
        // This must be done after all processes have been started to prevent duplicates
        crate::daemon::clear_restore_in_progress();

        // FIX #2: RESTORE COUNT DISCREPANCY
        // Calculate the final count based on actual process states, not the loop counter
        // Count processes that are actually online (running=true and process is alive)
        let final_online_count = runner
            .list()
            .filter(|(_id, p)| {
                if !p.running {
                    return false;
                }
                
                // Check if the process is actually alive using shared helper method
                opm::process::is_process_actually_alive(p.pid, p.shell_pid)
            })
            .count();
        
        println!("{} Success: {} processes restored.", *helpers::SUCCESS, final_online_count);
        
        // Display the process list immediately after restore
        // This allows users to see the current status without manually running 'opm ls'
        Internal::list(&"default".to_string(), &"local".to_string());
        
        // Restore operation is complete - exit the restore process
        // The daemon is now running in a separate PID and will continue independently
        std::process::exit(0);
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
                    let crash_detection_enabled = config::read().daemon.crash_detection;
                    // Check if process actually exists before reporting as online
                    // Include shell PID and tracked descendants to avoid false crashed status
                    let any_descendant_alive = is_any_descendant_alive(item.pid, &item.children)
                        || item
                            .shell_pid
                            .map_or(false, |pid| is_any_descendant_alive(pid, &item.children));
                    let process_actually_running = item.running && any_descendant_alive;

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
                    } else if item.errored {
                        "errored  ".red().bold()
                    } else if item.running {
                        // Check if process is in restart cooldown period
                        if item.is_in_restart_cooldown() {
                            // Process is in cooldown - show as waiting/restarting
                            "waiting  ".yellow().bold()
                        } else if crash_detection_enabled {
                            // Process is marked as running but PID doesn't exist
                            "crashed   ".red().bold()
                        } else {
                            "stopped   ".red().bold()
                        }
                    } else {
                        // Process is not running (running=false) - always show as stopped
                        // This ensures stopped processes display correctly even if they have
                        // stale crash.crashed flags from old dumps or failed restore operations
                        "stopped   ".red().bold()
                    };

                    // Only count uptime when the process is actually running
                    // Crashed or stopped processes should show "none" uptime
                    let uptime = if process_actually_running {
                        format!("{}  ", helpers::format_duration(item.started))
                    } else {
                        string!("none  ")
                    };

                    // Always show restarts counter
                    // restarts is persisted and provides accurate restart count
                    let restarts_value = item.restarts;

                    processes.push(ProcessItem {
                        status: status.into(),
                        cpu: format!("{cpu_percent}   "),
                        mem: format!("{memory_usage}   "),
                        id: id.to_string().cyan().bold().into(),
                        restarts: format!("{}  ", restarts_value),
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
                    .with(Style::modern().remove_verticals())
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

    /// Lists processes using the provided runner or falls back to loading from disk.
    ///
    /// This prevents race conditions with the daemon when displaying state immediately after modifications
    /// by using the in-memory runner state instead of reloading from disk.
    pub fn list_with_runner(format: &String, server_name: &String, runner_opt: Option<&Runner>) {
        // If a runner is provided for local/internal servers, use it directly
        // Otherwise fall back to the standard list behavior
        if matches!(&**server_name, "internal" | "local" | "default") && runner_opt.is_some() {
            let mut runner_clone = runner_opt.unwrap().clone();

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
                        // Check both the actual PID and shell PID (if present) to determine if process is alive.
                        // For shell-wrapped processes, either PID being alive means the process is running.
                        // This is consistent with the daemon monitoring logic and prevents false crash detection.
                        let pid_alive = is_pid_alive(item.pid)
                            || item.shell_pid.map_or(false, |pid| is_pid_alive(pid));
                        let process_actually_running = item.running && pid_alive;

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
                        } else if item.errored {
                            "errored  ".red().bold()
                        } else if item.running {
                            // Check if process is in restart cooldown period
                            if item.is_in_restart_cooldown() {
                                // Process is in cooldown - show as waiting/restarting
                                "waiting  ".yellow().bold()
                            } else {
                                // Process is marked as running but PID doesn't exist - it crashed
                                "crashed   ".red().bold()
                            }
                        } else {
                            // Process is not running (running=false) - always show as stopped
                            // This ensures stopped processes display correctly even if they have
                            // stale crash.crashed flags from old dumps or failed restore operations
                            "stopped   ".red().bold()
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
                            restarts: format!("{}  ", item.restarts),
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

                    let table =
                        Table::new(&processes)
                            .with(Style::modern().remove_verticals())
                            .with(Modify::new(Segment::all()).with(BorderColor::filled(
                                Color::new("\x1b[38;2;45;55;72m", "\x1b[39m"),
                            )))
                            .with(Colorization::exact([Color::FG_BRIGHT_CYAN], Rows::first()))
                            .with(
                                Modify::new(Columns::single(1))
                                    .with(Width::truncate(40).suffix("... ")),
                            )
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

            render_list(&mut runner_clone, true);
        } else {
            // For remote servers or when no runner provided, use standard list
            Self::list(format, server_name);
        }
    }
}
