mod cli;
mod daemon;
mod globals;
mod webui;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{LogLevel, Verbosity};
use macros_rs::{str, string};
use update_informer::{Check, registry};

use crate::{
    cli::{Args, Item, Items, internal::Internal},
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
    #[command(visible_alias = "rm", visible_alias = "delete")]
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

    /// Agent management (client-side daemon for server connection)
    #[command(visible_alias = "server", visible_alias = "remote")]
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
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
    /// List connected agents (view via API/Web UI)
    #[command(visible_alias = "ls")]
    List,
    /// Disconnect agent
    Disconnect,
    /// Show agent status
    Status,
}

fn agent_list() {
    use opm::helpers;
    
    println!("{} Connected Agents", *helpers::INFO);
    println!();
    println!("To view connected agents, use one of the following methods:");
    println!();
    println!("  1. Web UI:");
    println!("     • Start the daemon with Web UI enabled:");
    println!("       opm daemon restore --webui");
    println!("     • Open your browser to: http://localhost:9876");
    println!("     • Navigate to the 'Agents' page");
    println!();
    println!("  2. API Endpoint:");
    println!("     • Start the daemon with API enabled:");
    println!("       opm daemon restore --api");
    println!("     • Query: curl http://localhost:9876/daemon/agents/list");
    println!();
    println!("  3. Connect an agent:");
    println!("     • On a remote machine: opm agent connect <server-url>");
    println!("     • Example: opm agent connect http://192.168.1.100:9876");
}

fn agent_connect(server_url: String, name: Option<String>, token: Option<String>) {
    use opm::helpers;
    use opm::agent::types::AgentConfig;
    use opm::agent::connection::AgentConnection;
    
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
    
    println!("{} Agent ID: {}", *helpers::SUCCESS, config.id);
    println!("{} Agent Name: {}", *helpers::SUCCESS, config.name);
    println!("{} Server URL: {}", *helpers::SUCCESS, config.server_url);
    
    // Start agent in async runtime
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut connection = AgentConnection::new(config);
        if let Err(e) = connection.run().await {
            eprintln!("{} Agent error: {}", *helpers::FAIL, e);
        }
    });
}

fn agent_disconnect() {
    use opm::helpers;
    
    match load_agent_config() {
        Ok(config) => {
            println!("{} Disconnecting agent '{}'...", *helpers::SUCCESS, config.name);
            
            // Remove agent config file
            if let Err(e) = remove_agent_config() {
                eprintln!("{} Failed to remove agent config: {}", *helpers::FAIL, e);
            } else {
                println!("{} Agent disconnected successfully", *helpers::SUCCESS);
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
    
    let path = home::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found"))?;
    let config_path = path.join(".opm").join("agent.toml");
    
    let toml_str = toml::to_string(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    
    fs::write(config_path, toml_str)?;
    Ok(())
}

fn load_agent_config() -> Result<opm::agent::types::AgentConfig, std::io::Error> {
    use std::fs;
    
    let path = home::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found"))?;
    let config_path = path.join(".opm").join("agent.toml");
    
    let contents = fs::read_to_string(config_path)?;
    let config: opm::agent::types::AgentConfig = toml::from_str(&contents)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    
    Ok(config)
}

fn remove_agent_config() -> Result<(), std::io::Error> {
    use std::fs;
    
    let path = home::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found"))?;
    let config_path = path.join(".opm").join("agent.toml");
    
    fs::remove_file(config_path)?;
    Ok(())
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
        } => cli::start(name, args, watch, max_memory, reset_env, &defaults(server), workers, port_range),
        Commands::Stop { items, server } => cli::stop(items, &defaults(server)),
        Commands::Remove { items, server } => cli::remove(items, &defaults(server)),
        Commands::Restore { server } => {
            // Ensure daemon is running before restore (silent mode)
            if !daemon::pid::exists() {
                daemon::start(false);
            } else {
                // Check if daemon is actually running (not just a stale PID file)
                match daemon::pid::read() {
                    Ok(pid) => {
                        if !daemon::pid::running(pid.get()) {
                            daemon::pid::remove();
                            daemon::start(false);
                        }
                    }
                    Err(_) => {
                        // PID file exists but can't be read, remove and start daemon
                        daemon::pid::remove();
                        daemon::start(false);
                    }
                }
            }
            Internal::restore(&defaults(server))
        },
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

        Commands::Agent { command } => match command {
            AgentCommand::Connect { server_url, name, token } => {
                agent_connect(server_url.clone(), name.clone(), token.clone())
            }
            AgentCommand::List => agent_list(),
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
