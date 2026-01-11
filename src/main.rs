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
        /// Server
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
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Stop then remove a process
    #[command(visible_alias = "rm", visible_alias = "delete")]
    Remove {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Get env of a process
    #[command(visible_alias = "cmdline")]
    Env {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        /// Server
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
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },
    /// List all processes
    #[command(visible_alias = "ls")]
    List {
        /// Format output
        #[arg(long, default_value_t = string!("default"))]
        format: String,
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Restore all processes
    #[command(visible_alias = "resurrect")]
    Restore {
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Save all processes to dumpfile
    #[command(visible_alias = "store")]
    Save {
        /// Server
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
        /// Server
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
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Daemon management
    #[command(visible_alias = "agent", visible_alias = "bgd")]
    Daemon {
        #[command(subcommand)]
        command: Daemon,
    },

    /// Restart a process
    Restart {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Reload a process (same as restart - stops and starts the process)
    Reload {
        #[clap(value_parser = cli::validate_items)]
        items: Items,
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Get startup command for a process
    #[command(visible_alias = "cstart", visible_alias = "startup")]
    GetCommand {
        #[clap(value_parser = cli::validate::<Item>)]
        item: Item,
        /// Server
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
        /// Server
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Server management
    #[command(visible_alias = "remote")]
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
}

#[derive(Subcommand)]
enum ServerCommand {
    /// Connect to a remote server
    Connect {
        /// Server name
        name: String,
        /// Server address (IP/URL)
        #[arg(long)]
        address: String,
        /// Authentication token (optional)
        #[arg(long)]
        token: Option<String>,
    },
    /// List all configured servers
    #[command(visible_alias = "ls")]
    List,
    /// Remove a server
    #[command(visible_alias = "rm", visible_alias = "delete")]
    Remove {
        /// Server name
        name: String,
    },
}

fn server_connect(name: &str, address: &str, token: &Option<String>) {
    use opm::{config, helpers};
    use std::collections::BTreeMap;
    use std::fs;
    
    let mut servers = config::servers();
    let server = config::structs::Server {
        address: address.trim_end_matches('/').to_string(),
        token: token.clone(),
    };
    
    if servers.servers.is_none() {
        servers.servers = Some(BTreeMap::new());
    }
    
    if let Some(ref mut server_map) = servers.servers {
        server_map.insert(name.to_string(), server);
    }
    
    // Save to file
    match home::home_dir() {
        Some(path) => {
            let config_path = format!("{}/.opm/servers.toml", path.display());
            let contents = match toml::to_string(&servers) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{} Failed to serialize server config: {}", *helpers::FAIL, e);
                    return;
                }
            };
            
            if let Err(e) = fs::write(&config_path, contents) {
                eprintln!("{} Failed to write server config: {}", *helpers::FAIL, e);
                return;
            }
            
            println!("{} Server '{}' added successfully", *helpers::SUCCESS, name);
            println!("   Address: {}", address);
            if token.is_some() {
                println!("   Token: (configured)");
            }
        }
        None => eprintln!("{} Cannot get home directory", *helpers::FAIL),
    }
}

fn server_list() {
    use opm::{config, helpers};
    use tabled::{Table, Tabled};
    
    #[derive(Tabled)]
    struct ServerDisplay {
        #[tabled(rename = "Name")]
        name: String,
        #[tabled(rename = "Address")]
        address: String,
        #[tabled(rename = "Token")]
        token: String,
    }
    
    let servers = config::servers();
    
    if let Some(server_map) = servers.servers {
        if server_map.is_empty() {
            println!("{} No servers configured", *helpers::WARN);
            return;
        }
        
        let display: Vec<ServerDisplay> = server_map.into_iter().map(|(name, server)| {
            ServerDisplay {
                name,
                address: server.address,
                token: if server.token.is_some() { "Yes".to_string() } else { "No".to_string() },
            }
        }).collect();
        
        println!("{}", Table::new(display));
    } else {
        println!("{} No servers configured", *helpers::WARN);
    }
}

fn server_remove(name: &str) {
    use opm::{config, helpers};
    use std::fs;
    
    let mut servers = config::servers();
    
    if let Some(ref mut server_map) = servers.servers {
        if server_map.remove(name).is_some() {
            // Save to file
            match home::home_dir() {
                Some(path) => {
                    let config_path = format!("{}/.opm/servers.toml", path.display());
                    let contents = match toml::to_string(&servers) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("{} Failed to serialize server config: {}", *helpers::FAIL, e);
                            return;
                        }
                    };
                    
                    if let Err(e) = fs::write(&config_path, contents) {
                        eprintln!("{} Failed to write server config: {}", *helpers::FAIL, e);
                        return;
                    }
                    
                    println!("{} Server '{}' removed successfully", *helpers::SUCCESS, name);
                }
                None => eprintln!("{} Cannot get home directory", *helpers::FAIL),
            }
        } else {
            eprintln!("{} Server '{}' not found", *helpers::FAIL, name);
        }
    } else {
        eprintln!("{} No servers configured", *helpers::WARN);
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
        
        Commands::Server { command } => match command {
            ServerCommand::Connect { name, address, token } => {
                server_connect(name, address, token)
            }
            ServerCommand::List => server_list(),
            ServerCommand::Remove { name } => server_remove(name),
        },
    };

    if !matches!(&cli.command, Commands::Daemon { .. })
        && !matches!(&cli.command, Commands::Save { .. })
        && !matches!(&cli.command, Commands::Env { .. })
        && !matches!(&cli.command, Commands::Export { .. })
        && !matches!(&cli.command, Commands::GetCommand { .. })
        && !matches!(&cli.command, Commands::Adjust { .. })
        && !matches!(&cli.command, Commands::Server { .. })
    {
        // When auto-starting daemon, read API/WebUI settings from config
        if !daemon::pid::exists() {
            let config = opm::config::read();
            daemon::restart(&config.daemon.web.api, &config.daemon.web.ui, false);
        }
    }
}
