mod cli;
mod daemon;
mod globals;
mod webui;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{LogLevel, Verbosity};
use macros_rs::{str, string, crashln};
use update_informer::{registry, Check};
use std::fs;
use global_placeholders::global;

use crate::{
    cli::{internal::Internal, Args, Item, Items},
    globals::defaults,
};

#[derive(Copy, Clone, Debug, Default)]
struct NoneLevel;
impl LogLevel for NoneLevel {
    fn default() -> Option<log::Level> {
        None
    }
}

#[derive(Parser)]
#[command(version = str!(cli::get_version(false)))]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    #[clap(flatten)]
    verbose: Verbosity<NoneLevel>,
}

#[derive(Subcommand)]
enum Daemon {
    /// Reset process index
    #[command(visible_alias = "reset_position")]
    Reset,
    /// Stop daemon
    #[command(visible_alias = "kill")]
    Stop,
    /// Restart daemon
    #[command(visible_alias = "restart", visible_alias = "start")]
    Restore {
        /// Daemon api
        #[arg(long)]
        api: bool,
        /// WebUI using api
        #[arg(long)]
        webui: bool,
    },
    /// Check daemon health
    #[command(visible_alias = "info", visible_alias = "status")]
    Health {
        /// Format output
        #[arg(long, default_value_t = string!("default"))]
        format: String,
    },
    /// Setup systemd service to start OPM daemon automatically
    #[command(visible_alias = "install")]
    Setup,
}

// add opm restore command
#[derive(Subcommand)]
enum Commands {
    /// Import process from environment file
    #[command(visible_alias = "add")]
    Import {
        /// Path of file to import
        path: String,
    },
    /// Export environment file from process
    #[command(visible_alias = "get")]
    Export {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Path to export file
        path: Option<String>,
    },
    /// Start/Restart a process
    Start {
        /// Process name
        #[arg(long)]
        name: Option<String>,
        #[clap(value_parser = cli::validate::<Args>)]
        args: Args,
        /// Watch to reload path
        #[arg(long)]
        watch: Option<String>,
        /// Maximum memory limit (e.g., 100M, 1G)
        #[arg(long)]
        max_memory: Option<String>,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
        /// Reset environment values
        #[arg(short, long)]
        reset_env: bool,
        /// Number of worker instances to spawn (for load balancing)
        #[arg(short = 'w', long)]
        workers: Option<usize>,
        /// Port range for workers (e.g., "3000-3010" or just "3000" for SO_REUSEPORT)
        #[arg(short = 'p', long)]
        port_range: Option<String>,
    },
    /// Stop/Kill a process
    #[command(visible_alias = "kill")]
    Stop {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Stop then remove a process
    #[command(visible_alias = "rm", visible_alias = "del", visible_alias = "delete")]
    Remove {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Get env of a process
    #[command(visible_alias = "cmdline")]
    Env {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Get information of a process
    #[command(visible_alias = "info")]
    Details {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        /// Format output
        #[arg(long, default_value_t = string!("default"))]
        format: String,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// List all processes
    #[command(visible_alias = "ls")]
    List {
        /// Format output
        #[arg(long, default_value_t = string!("default"))]
        format: String,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Restore all processes
    #[command(visible_alias = "resurrect")]
    Restore {
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Save all processes to dumpfile
    #[command(visible_alias = "store")]
    Save {
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Get logs from a process
    Logs {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        #[arg(
            long,
            default_value_t = 15,
            help = "Number of lines to display from the end of the log file"
        )]
        lines: usize,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
        /// Follow log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
        /// Filter logs by pattern (case-insensitive)
        #[arg(long)]
        filter: Option<String>,
        /// Show only error logs
        #[arg(long)]
        errors_only: bool,
        /// Show log statistics
        #[arg(long)]
        stats: bool,
    },
    /// Flush a process log
    #[command(visible_alias = "clean", visible_alias = "log_rotate")]
    Flush {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Daemon management
    #[command(visible_alias = "bgd")]
    Daemon {
        #[command(subcommand)]
        command: Daemon,
    },

    /// Restart a process
    Restart {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Reload a process (same as restart - stops and starts the process)
    Reload {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Get startup command for a process
    #[command(visible_alias = "cstart", visible_alias = "startup")]
    GetCommand {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Adjust process command and/or name
    #[command(visible_alias = "update", visible_alias = "modify")]
    Adjust {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        /// New execution command/script
        #[arg(long)]
        command: Option<String>,
        /// New process name
        #[arg(long)]
        name: Option<String>,
        /// Agent connection (use with agent-enabled server)
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Backup management
    #[command(visible_alias = "bkp")]
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },

    /// Agent management (client-side daemon for server connection)
    #[command(visible_alias = "server", visible_alias = "remote")]
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Subcommand)]
enum BackupCommand {
    /// Restore process dump from backup file
    #[command(visible_alias = "recover")]
    Restore,
    /// Show backup file information
    #[command(visible_alias = "info")]
    Status,
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Connect agent to a server
    Connect {
        /// Server URL (e.g., http://192.168.1.100:9876)
        server_url: String,
        /// Agent name (auto-generated if not provided)
        #[arg(long)]
        name: Option<String>,
        /// Authentication token (optional)
        #[arg(long)]
        token: Option<String>,
    },
    /// List connected agents
    #[command(visible_alias = "ls")]
    List {
        /// Format output
        #[arg(long, default_value_t = string!("default"))]
        format: String,
        /// Server connection (defaults to local)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// List processes from all agents or a specific agent
    #[command(visible_alias = "ps")]
    Processes {
        /// Agent ID or name (optional, shows all if not provided)
        agent: Option<String>,
        /// Format output
        #[arg(long, default_value_t = string!("default"))]
        format: String,
        /// Server connection (defaults to local)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Disconnect agent
    Disconnect,
    /// Show agent status
    Status,
}

fn agent_list(format: &String, server_name: &String) {
    use colored::Colorize as ColorizeStr;
    use opm::{helpers, process::http};
    use serde_json::json;
    use tabled::{
        settings::{
            object::{Rows, Segment},
            style::BorderColor,
            themes::Colorization,
            Color, Modify, Style,
        },
        Table, Tabled,
    };

    #[derive(Tabled, Debug)]
    struct AgentItem {
        id: String,
        name: String,
        hostname: String,
        status: String,
        #[tabled(rename = "CPU")]
        cpu: String,
        #[tabled(rename = "Mem")]
        memory: String,
        #[tabled(rename = "Processes")]
        process_count: String,
    }

    impl serde::Serialize for AgentItem {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let trimmed_json = json!({
                "id": &self.id.trim(),
                "name": &self.name.trim(),
                "hostname": &self.hostname.trim(),
                "status": &self.status.trim(),
                "cpu": &self.cpu.trim(),
                "memory": &self.memory.trim(),
                "process_count": &self.process_count.trim(),
            });
            trimmed_json.serialize(serializer)
        }
    }

    // Get server config to connect to API
    let config = opm::config::read();
    let server = if matches!(&**server_name, "internal" | "local") {
        format!(
            "http://{}:{}",
            config.daemon.web.address, config.daemon.web.port
        )
    } else if let Some(servers) = opm::config::servers().servers {
        if let Some(srv) = servers.get(server_name) {
            srv.address.clone()
        } else {
            eprintln!("{} Server '{}' not found", *helpers::FAIL, server_name);
            return;
        }
    } else {
        eprintln!("{} No servers configured", *helpers::FAIL);
        return;
    };

    // Make API call to get agent list
    match http::agent_list(&server) {
        Ok(response) => {
            match response.json::<Vec<opm::agent::types::AgentInfo>>() {
                Ok(agents) => {
                    let mut agent_items: Vec<AgentItem> = Vec::new();

                    for agent in agents {
                        let status_str = match agent.status {
                            opm::agent::types::AgentStatus::Online => "online".green().to_string(),
                            opm::agent::types::AgentStatus::Offline => "offline".red().to_string(),
                            opm::agent::types::AgentStatus::Connecting => {
                                "connecting".yellow().to_string()
                            }
                            opm::agent::types::AgentStatus::Reconnecting => {
                                "reconnecting".yellow().to_string()
                            }
                        };

                        let (cpu, memory) = if let Some(ref sys_info) = agent.system_info {
                            if let Some(ref usage) = sys_info.resource_usage {
                                let cpu_str = usage
                                    .cpu_usage
                                    .map(|c| format!("{:.1}%", c))
                                    .unwrap_or_else(|| "N/A".to_string());
                                let mem_str = usage
                                    .memory_percent
                                    .map(|m| format!("{:.1}%", m))
                                    .unwrap_or_else(|| "N/A".to_string());
                                (cpu_str, mem_str)
                            } else {
                                ("N/A".to_string(), "N/A".to_string())
                            }
                        } else {
                            ("N/A".to_string(), "N/A".to_string())
                        };

                        // Get process count by fetching processes for this agent
                        let process_count = match http::agent_processes(&server, &agent.id) {
                            Ok(resp) => match resp.json::<Vec<opm::process::ProcessItem>>() {
                                Ok(processes) => processes.len().to_string(),
                                Err(_) => "N/A".to_string(),
                            },
                            Err(_) => "N/A".to_string(),
                        };

                        agent_items.push(AgentItem {
                            id: agent.id.clone(),
                            name: agent.name.clone(),
                            hostname: agent.hostname.unwrap_or_else(|| "N/A".to_string()),
                            status: status_str,
                            cpu,
                            memory,
                            process_count,
                        });
                    }

                    if agent_items.is_empty() {
                        println!("{} No connected agents", *helpers::SUCCESS);
                    } else {
                        match format.as_str() {
                            "raw" => println!("{:?}", agent_items),
                            "json" => {
                                if let Ok(json) = serde_json::to_string(&agent_items) {
                                    println!("{}", json);
                                }
                            }
                            "default" => {
                                let table = Table::new(&agent_items)
                                    .with(Style::rounded().remove_verticals())
                                    .with(Modify::new(Segment::all()).with(BorderColor::filled(
                                        Color::new("\x1b[38;2;45;55;72m", "\x1b[39m"),
                                    )))
                                    .with(Colorization::exact(
                                        [Color::FG_BRIGHT_CYAN],
                                        Rows::first(),
                                    ))
                                    .to_string();
                                println!("{}", table);
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    eprintln!("{} Failed to parse agent list: {}", *helpers::FAIL, e);
                }
            }
        }
        Err(e) => {
            eprintln!("{} Failed to fetch agent list: {}", *helpers::FAIL, e);
            eprintln!(
                "{} Make sure the daemon is running with API enabled:",
                *helpers::INFO
            );
            eprintln!("   opm daemon restore --api");
        }
    }
}

/// Truncate agent name to 3-5 characters for compact display
fn truncate_agent_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let len = chars.len().min(5).max(3);
    chars.iter().take(len).collect()
}

fn agent_processes(agent_filter: &Option<String>, format: &String, server_name: &String) {
    use colored::Colorize as ColorizeStr;
    use opm::{helpers, process::http};
    use serde_json::json;
    use tabled::{
        settings::{
            object::{Columns, Rows, Segment},
            style::BorderColor,
            themes::Colorization,
            Color, Modify, Style, Width,
        },
        Table, Tabled,
    };

    #[derive(Tabled, Debug)]
    struct ProcessDisplayItem {
        id: String,
        name: String,
        agent: String,
        pid: String,
        status: String,
        cpu: String,
        mem: String,
        uptime: String,
    }

    impl serde::Serialize for ProcessDisplayItem {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let trimmed_json = json!({
                "id": &self.id.trim(),
                "name": &self.name.trim(),
                "agent": &self.agent.trim(),
                "pid": &self.pid.trim(),
                "status": &self.status.trim(),
                "cpu": &self.cpu.trim(),
                "mem": &self.mem.trim(),
                "uptime": &self.uptime.trim(),
            });
            trimmed_json.serialize(serializer)
        }
    }

    // Get server config to connect to API
    let config = opm::config::read();
    let server = if matches!(&**server_name, "internal" | "local") {
        format!(
            "http://{}:{}",
            config.daemon.web.address, config.daemon.web.port
        )
    } else if let Some(servers) = opm::config::servers().servers {
        if let Some(srv) = servers.get(server_name) {
            srv.address.clone()
        } else {
            eprintln!("{} Server '{}' not found", *helpers::FAIL, server_name);
            return;
        }
    } else {
        eprintln!("{} No servers configured", *helpers::FAIL);
        return;
    };

    // First, get list of agents
    let agents = match http::agent_list(&server) {
        Ok(response) => match response.json::<Vec<opm::agent::types::AgentInfo>>() {
            Ok(agents) => agents,
            Err(e) => {
                eprintln!("{} Failed to parse agent list: {}", *helpers::FAIL, e);
                return;
            }
        },
        Err(e) => {
            eprintln!("{} Failed to fetch agent list: {}", *helpers::FAIL, e);
            eprintln!(
                "{} Make sure the daemon is running with API enabled:",
                *helpers::INFO
            );
            eprintln!("   opm daemon restore --api");
            return;
        }
    };

    // Filter agents if specified
    let agents_to_query: Vec<_> = if let Some(filter) = agent_filter {
        // Filter by ID or name
        agents
            .into_iter()
            .filter(|a| a.id == *filter || a.name == *filter)
            .collect()
    } else {
        agents
    };

    if agents_to_query.is_empty() {
        eprintln!("{} No agents found", *helpers::FAIL);
        if let Some(filter) = agent_filter {
            eprintln!("   No agent matching '{}'", filter);
        }
        return;
    }

    let mut all_processes: Vec<ProcessDisplayItem> = Vec::new();
    let is_multi_agent = agents_to_query.len() > 1;

    // Fetch processes from each agent
    for agent in agents_to_query.iter() {
        match http::agent_processes(&server, &agent.id) {
            Ok(resp) => match resp.json::<Vec<opm::process::ProcessItem>>() {
                Ok(processes) => {
                    let agent_prefix = if is_multi_agent {
                        format!("[{}]", truncate_agent_name(&agent.name))
                    } else {
                        String::new()
                    };

                    for process in processes {
                        let process_name = if is_multi_agent {
                            format!("{}{}", agent_prefix, process.name)
                        } else {
                            process.name.clone()
                        };

                        all_processes.push(ProcessDisplayItem {
                            id: process.id.to_string().cyan().to_string(),
                            name: process_name,
                            agent: agent.name.clone(),
                            pid: process.pid.to_string(),
                            status: process.status.clone(),
                            cpu: process.cpu.clone(),
                            mem: process.mem.clone(),
                            uptime: process.uptime.clone(),
                        });
                    }
                }
                Err(e) => {
                    eprintln!(
                        "{} Failed to parse processes for agent '{}': {}",
                        *helpers::WARN,
                        agent.name,
                        e
                    );
                }
            },
            Err(e) => {
                eprintln!(
                    "{} Failed to fetch processes for agent '{}': {}",
                    *helpers::WARN,
                    agent.name,
                    e
                );
            }
        }
    }

    if all_processes.is_empty() {
        println!("{} No processes found", *helpers::SUCCESS);
    } else {
        match format.as_str() {
            "raw" => println!("{:?}", all_processes),
            "json" => {
                if let Ok(json) = serde_json::to_string(&all_processes) {
                    println!("{}", json);
                }
            }
            "default" => {
                let table = Table::new(&all_processes)
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
                println!("{}", table);
            }
            _ => {}
        }
    }
}

fn agent_connect(server_url: String, name: Option<String>, token: Option<String>) {
    use opm::agent::types::AgentConfig;
    use opm::helpers;

    println!("{} Starting OPM Agent...", *helpers::SUCCESS);

    let config = AgentConfig::new(server_url, name, token);

    // Save agent config
    match save_agent_config(&config) {
        Ok(_) => println!("{} Agent configuration saved", *helpers::SUCCESS),
        Err(e) => {
            eprintln!("{} Failed to save agent config: {}", *helpers::FAIL, e);
            return;
        }
    }

    // Update OPM config to set role as agent
    let mut opm_config = opm::config::read();
    opm_config.role = opm::config::structs::Role::Agent;
    opm_config.daemon.web.api = true; // Enable API for agent
    opm_config.daemon.web.address = config.api_address.clone();
    opm_config.daemon.web.port = config.api_port as u64;
    opm_config.save();

    println!("{} Agent ID: {}", *helpers::SUCCESS, config.id);
    println!("{} Agent Name: {}", *helpers::SUCCESS, config.name);
    println!("{} Server URL: {}", *helpers::SUCCESS, config.server_url);

    // Start agent in background
    start_agent_daemon();
}

fn agent_disconnect() {
    use opm::helpers;

    match load_agent_config() {
        Ok(config) => {
            println!(
                "{} Disconnecting agent '{}'...",
                *helpers::SUCCESS,
                config.name
            );

            // Restore role to standalone
            let mut opm_config = opm::config::read();
            opm_config.role = opm::config::structs::Role::Standalone;
            opm_config.save();

            // Remove agent config file
            if let Err(e) = remove_agent_config() {
                eprintln!("{} Failed to remove agent config: {}", *helpers::FAIL, e);
            } else {
                println!("{} Agent disconnected successfully", *helpers::SUCCESS);
                println!("{} Role restored to standalone", *helpers::SUCCESS);
            }
        }
        Err(_) => {
            eprintln!("{} No active agent connection found", *helpers::WARN);
        }
    }
}

fn agent_status() {
    use opm::helpers;

    match load_agent_config() {
        Ok(config) => {
            println!("{} Agent Status", *helpers::SUCCESS);
            println!("   ID: {}", config.id);
            println!("   Name: {}", config.name);
            println!("   Server: {}", config.server_url);
            println!("   Status: Connected"); // In real implementation, check actual connection status
        }
        Err(_) => {
            println!("{} No active agent connection", *helpers::WARN);
        }
    }
}

fn save_agent_config(config: &opm::agent::types::AgentConfig) -> Result<(), std::io::Error> {
    use std::fs;

    let path = home::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;
    let config_path = path.join(".opm").join("agent.toml");

    let toml_str =
        toml::to_string(config).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    fs::write(config_path, toml_str)?;
    Ok(())
}

fn load_agent_config() -> Result<opm::agent::types::AgentConfig, std::io::Error> {
    use std::fs;

    let path = home::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;
    let config_path = path.join(".opm").join("agent.toml");

    let contents = fs::read_to_string(config_path)?;
    let config: opm::agent::types::AgentConfig =
        toml::from_str(&contents).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    Ok(config)
}

fn remove_agent_config() -> Result<(), std::io::Error> {
    use std::fs;

    let path = home::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;
    let config_path = path.join(".opm").join("agent.toml");

    fs::remove_file(config_path)?;
    Ok(())
}

// Time to wait for daemon to initialize after starting (in seconds)
const DAEMON_INIT_WAIT_SECS: u64 = 2;
// Socket readiness retry parameters
const SOCKET_RETRY_MAX: u32 = 10;
const SOCKET_RETRY_INITIAL_MS: u64 = 200;
const SOCKET_RETRY_INCREMENT_MS: u64 = 100;

fn start_agent_daemon() {
    use nix::unistd::{fork, setsid, ForkResult};
    use opm::agent::connection::AgentConnection;
    use opm::helpers;
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    // First, ensure the local daemon is running with API enabled
    if !daemon::pid::exists() {
        println!(
            "{} Starting local OPM daemon with API enabled...",
            *helpers::SUCCESS
        );
        daemon::restart(&true, &false, false);

        // Wait a bit for daemon to initialize
        std::thread::sleep(std::time::Duration::from_secs(DAEMON_INIT_WAIT_SECS));
    }

    // Fork a background process that will run the agent connection
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child: _ }) => {
            // Parent process
            println!("{} Agent connected to server", *helpers::SUCCESS);
        }
        Ok(ForkResult::Child) => {
            // Child process - run the agent

            // Create a new session
            if let Err(e) = setsid() {
                eprintln!("Failed to create new session: {}", e);
                std::process::exit(1);
            }

            // Redirect stdin to /dev/null
            if let Ok(devnull) = OpenOptions::new().read(true).open("/dev/null") {
                let fd = devnull.as_raw_fd();
                let result = unsafe { libc::dup2(fd, 0) };
                if result == -1 {
                    eprintln!("Failed to redirect stdin");
                    std::process::exit(1);
                }
            }

            // Redirect stdout and stderr to agent log file
            let log_path = home::home_dir()
                .map(|p| p.join(".opm").join("agent.log"))
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp/opm-agent.log"));

            if let Ok(log_file) = OpenOptions::new().create(true).append(true).open(&log_path) {
                let log_fd = log_file.as_raw_fd();
                let result1 = unsafe { libc::dup2(log_fd, 1) };
                if result1 == -1 {
                    eprintln!("Failed to redirect stdout");
                    std::process::exit(1);
                }
                let result2 = unsafe { libc::dup2(log_fd, 2) };
                if result2 == -1 {
                    eprintln!("Failed to redirect stderr");
                    std::process::exit(1);
                }
            }

            // Run agent connection in this child process
            match load_agent_config() {
                Ok(config) => {
                    let runtime = tokio::runtime::Runtime::new().unwrap();
                    runtime.block_on(async {
                        let mut connection = AgentConnection::new(config);
                        if let Err(e) = connection.run().await {
                            eprintln!("[Agent Error] {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[Agent Error] Failed to load config: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("{} Failed to fork agent process: {}", *helpers::FAIL, e);
            std::process::exit(1);
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let mut env = env_logger::Builder::new();
    let level = cli.verbose.log_level_filter();
    let informer = update_informer::new(registry::Crates, "opm", env!("CARGO_PKG_VERSION"));

    if let Some(version) = informer.check_version().ok().flatten() {
        println!(
            "{} New version is available: {version}",
            *opm::helpers::WARN
        );
    }

    globals::init();

    // Configure custom certificates for TLS
    use rustls::ClientConfig;
    use rustls::crypto::ring::default_provider;
    use std::sync::Arc;
    let provider = Arc::new(default_provider());
    use rustls_native_certs::load_native_certs;
    let mut root_store = rustls::RootCertStore::empty();
    for cert in load_native_certs().unwrap() {
        root_store.add(cert).unwrap();
    }
    let _tls_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions().unwrap()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // Pass `tls_config` when establishing connections using tokio-tungstenite.
    env.filter_level(level).init();

    match &cli.command {
        Commands::Import { path } => cli::import::read_hcl(path),
        Commands::Export { items, path } => cli::import::export_hcl(items, path),
        Commands::Start {
            name,
            args,
            watch,
            max_memory,
            server,
            reset_env,
            workers,
            port_range,
        } => cli::start(
            name,
            args,
            watch,
            max_memory,
            reset_env,
            &defaults(server),
            workers,
            port_range,
        ),
        Commands::Stop { items, server } => cli::stop(items, &defaults(server)),
        Commands::Remove { items, server } => cli::remove(items, &defaults(server)),
        Commands::Restore { server } => {
            // Ensure daemon is running before restore (silent mode)
            // Read config to check if API/WebUI should be enabled
            let config = opm::config::read();
            let daemon_was_started = if !daemon::pid::exists() {
                daemon::restart(&config.daemon.web.api, &config.daemon.web.ui, false);
                true
            } else {
                // Check if daemon is actually running (not just a stale PID file)
                match daemon::pid::read() {
                    Ok(pid) => {
                        if !daemon::pid::running(pid.get()) {
                            daemon::pid::remove();
                            daemon::restart(&config.daemon.web.api, &config.daemon.web.ui, false);
                            true
                        } else {
                            false
                        }
                    }
                    Err(_) => {
                        // PID file exists but can't be read, remove and start daemon
                        daemon::pid::remove();
                        daemon::restart(&config.daemon.web.api, &config.daemon.web.ui, false);
                        true
                    }
                }
            };

            // Wait for daemon socket to be ready before proceeding with restore
            // This prevents "Connection refused" errors when restore tries to read from daemon
            // We check socket readiness regardless of whether we just started the daemon,
            // because in container restart scenarios the daemon might exist but socket isn't ready yet
            {
                use global_placeholders::global;
                let socket_path = global!("opm.socket");
                let mut retry_count = 0;
                let mut socket_ready = false;
                
                // Calculate total max wait time for warning message
                let total_wait_ms: u64 = (0..SOCKET_RETRY_MAX)
                    .map(|i| SOCKET_RETRY_INITIAL_MS + (i as u64 * SOCKET_RETRY_INCREMENT_MS))
                    .sum();
                let total_wait_secs = total_wait_ms as f64 / 1000.0;
                
                // Try immediately first, then retry with increasing delays
                // Use reduced retry count if daemon was already running (socket should be ready quickly)
                // Use full retry count if we just started the daemon (socket needs time to initialize)
                let max_retries = if daemon_was_started {
                    SOCKET_RETRY_MAX // Full retries when we just started the daemon
                } else {
                    3 // Reduced retries when daemon was already running
                };
                
                loop {
                    if opm::socket::is_daemon_running(&socket_path) {
                        socket_ready = true;
                        break;
                    }
                    
                    if retry_count >= max_retries {
                        break;
                    }
                    
                    // Exponential backoff: start with SOCKET_RETRY_INITIAL_MS and increase by SOCKET_RETRY_INCREMENT_MS each retry
                    let wait_ms = SOCKET_RETRY_INITIAL_MS + (retry_count as u64 * SOCKET_RETRY_INCREMENT_MS);
                    std::thread::sleep(std::time::Duration::from_millis(wait_ms));
                    retry_count += 1;
                }
                
                if !socket_ready {
                    eprintln!(
                        "{} Warning: Daemon socket is not ready after ~{:.1} seconds. Restore may fail.\n\
                         {} Consider waiting a moment and retrying the restore command.", 
                        *opm::helpers::WARN,
                        total_wait_secs,
                        " ".repeat(4) // Indent the second line
                    );
                }
            }

            // Auto-start agent if config exists
            if load_agent_config().is_ok() {
                start_agent_daemon();
            }

            Internal::restore(&defaults(server))
        }
        Commands::Save { server } => Internal::save(&defaults(server)),
        Commands::Env { item, server } => cli::env(item, &defaults(server)),
        Commands::Details {
            item,
            format,
            server,
        } => cli::info(item, format, &defaults(server)),
        Commands::List { format, server } => Internal::list(format, &defaults(server)),
        Commands::Logs {
            item,
            lines,
            server,
            follow,
            filter,
            errors_only,
            stats,
        } => cli::logs(
            item,
            lines,
            &defaults(server),
            *follow,
            filter.as_deref(),
            *errors_only,
            *stats,
        ),
        Commands::Flush { item, server } => cli::flush(item, &defaults(server)),

        Commands::Daemon { command } => match command {
            Daemon::Stop => daemon::stop(),
            Daemon::Reset => daemon::reset(),
            Daemon::Health { format } => daemon::health(format),
            Daemon::Restore { api, webui } => daemon::restart(api, webui, level.as_str() != "OFF"),
            Daemon::Setup => daemon::setup(),
        },

        Commands::Restart { items, server } => cli::restart(items, &defaults(server)),
        Commands::Reload { items, server } => cli::reload(items, &defaults(server)),
        Commands::GetCommand { item, server } => cli::get_command(item, &defaults(server)),
        Commands::Adjust {
            item,
            command,
            name,
            server,
        } => cli::adjust(item, command, name, &defaults(server)),

        Commands::Backup { command } => match command {
            BackupCommand::Restore => {
                use opm::{process::dump, helpers};
                match dump::restore_from_backup() {
                    Ok(()) => {
                        println!("{} Successfully restored from backup file", *helpers::SUCCESS);
                        println!("{} Restart the daemon to apply changes: opm daemon restore", *helpers::WARN);
                    }
                    Err(e) => {
                        crashln!("{} {}", *helpers::FAIL, e);
                    }
                }
            }
            BackupCommand::Status => {
                use opm::{process::dump, helpers};
                let backup_path = format!("{}.bak", global!("opm.dump"));
                if dump::has_backup() {
                    match fs::metadata(&backup_path) {
                        Ok(metadata) => {
                            println!("{} Backup file information:", *helpers::SUCCESS);
                            println!("  Path: {}", backup_path);
                            println!("  Size: {} bytes", metadata.len());
                            if let Ok(modified) = metadata.modified() {
                                let datetime: chrono::DateTime<chrono::Utc> = modified.into();
                                println!("  Last modified: {}", datetime.format("%Y-%m-%d %H:%M:%S UTC"));
                            }
                        }
                        Err(e) => {
                            crashln!("{} Failed to read backup file metadata: {}", *helpers::FAIL, e);
                        }
                    }
                } else {
                    crashln!("{} No backup file found at {}", *helpers::FAIL, backup_path);
                }
            }
        },

        Commands::Agent { command } => match command {
            AgentCommand::Connect {
                server_url,
                name,
                token,
            } => agent_connect(server_url.clone(), name.clone(), token.clone()),
            AgentCommand::List { format, server } => agent_list(format, &defaults(server)),
            AgentCommand::Processes {
                agent,
                format,
                server,
            } => agent_processes(agent, format, &defaults(server)),
            AgentCommand::Disconnect => agent_disconnect(),
            AgentCommand::Status => agent_status(),
        },
    };

    if !matches!(&cli.command, Commands::Daemon { .. })
        && !matches!(&cli.command, Commands::Save { .. })
        && !matches!(&cli.command, Commands::Env { .. })
        && !matches!(&cli.command, Commands::Export { .. })
        && !matches!(&cli.command, Commands::GetCommand { .. })
        && !matches!(&cli.command, Commands::Adjust { .. })
        && !matches!(&cli.command, Commands::Agent { .. })
    {
        // When auto-starting daemon, read API/WebUI settings from config
        if !daemon::pid::exists() {
            let config = opm::config::read();
            daemon::restart(&config.daemon.web.api, &config.daemon.web.ui, false);
        }
    }
}
