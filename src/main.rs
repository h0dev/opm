mod cli;
mod daemon;
mod globals;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{LogLevel, Verbosity};
use macros_rs::{str, string, then};
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
    Restore,
    /// Check daemon health
    #[command(visible_alias = "info", visible_alias = "status")]
    Health {
        /// Format output
        #[arg(long, default_value_t = string!("default"))]
        format: String,
    },
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
        } => cli::start(name, args, watch, max_memory, reset_env, &defaults(server)),
        Commands::Stop { items, server } => cli::stop(items, &defaults(server)),
        Commands::Remove { items, server } => cli::remove(items, &defaults(server)),
        Commands::Restore { server } => {
            // Ensure daemon is running before restore
            if !daemon::pid::exists() {
                println!("{} Daemon not running, starting it now...", *opm::helpers::SUCCESS);
                daemon::start(false);
            } else {
                // Check if daemon is actually running (not just a stale PID file)
                match daemon::pid::read() {
                    Ok(pid) => {
                        if !daemon::pid::running(pid.get()) {
                            println!("{} Daemon not running (stale PID file), starting it now...", *opm::helpers::SUCCESS);
                            daemon::pid::remove();
                            daemon::start(false);
                        }
                    }
                    Err(_) => {
                        // PID file exists but can't be read, remove and start daemon
                        println!("{} Daemon PID file corrupted, starting daemon...", *opm::helpers::SUCCESS);
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
            Daemon::Restore => daemon::restart(level.as_str() != "OFF"),
        },

        Commands::Restart { items, server } => cli::restart(items, &defaults(server)),
        Commands::Reload { items, server } => cli::reload(items, &defaults(server)),
        Commands::GetCommand { item, server } => cli::get_command(item, &defaults(server)),
    };

    if !matches!(&cli.command, Commands::Daemon { .. })
        && !matches!(&cli.command, Commands::Save { .. })
        && !matches!(&cli.command, Commands::Env { .. })
        && !matches!(&cli.command, Commands::Export { .. })
        && !matches!(&cli.command, Commands::GetCommand { .. })
    {
        then!(!daemon::pid::exists(), daemon::restart(false));
    }
}
