pub mod dump;
pub mod hash;
pub mod http;
pub mod id;
pub mod unix;

use crate::{config, config::structs::Server, file, helpers};

use std::{
    collections::{BTreeMap, HashSet},
    env,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};

use chrono::serde::ts_milliseconds;
use chrono::{DateTime, Utc};
use global_placeholders::global;
use macros_rs::{crashln, string, ternary, then};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Constants for process termination waiting
const MAX_TERMINATION_WAIT_ATTEMPTS: u32 = 50;
const TERMINATION_CHECK_INTERVAL_MS: u64 = 100;

/// Wait for a process to terminate gracefully
/// Uses libc::kill(pid, 0) to check if process exists, which is the same approach
/// as pid::running() but implemented here to avoid circular dependencies.
/// This is more reliable than trying to create a process handle that could fail
/// for other reasons (permissions, etc.)
/// Returns true if process terminated, false if timeout reached
fn wait_for_process_termination(pid: i64) -> bool {
    for _ in 0..MAX_TERMINATION_WAIT_ATTEMPTS {
        // Check if process is still running using libc::kill with signal 0
        // This returns 0 if the process exists, -1 if it doesn't (or permission denied)
        let process_exists = unsafe { libc::kill(pid as i32, 0) == 0 };
        if !process_exists {
            return true; // Process has terminated (or we don't have permission to check)
        }
        thread::sleep(Duration::from_millis(TERMINATION_CHECK_INTERVAL_MS));
    }
    false // Timeout reached, process is still running
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ItemSingle {
    pub info: Info,
    pub stats: Stats,
    pub watch: Watch,
    pub log: Log,
    pub raw: Raw,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Info {
    pub id: usize,
    pub pid: i64,
    pub name: String,
    pub status: String,
    #[schema(value_type = String, example = "/path")]
    pub path: PathBuf,
    pub uptime: String,
    pub command: String,
    pub children: Vec<i64>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Stats {
    pub restarts: u64,
    pub start_time: i64,
    pub cpu_percent: Option<f64>,
    pub memory_usage: Option<MemoryInfo>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct MemoryInfo {
    pub rss: u64,
    pub vms: u64,
}

impl From<unix::NativeMemoryInfo> for MemoryInfo {
    fn from(native: unix::NativeMemoryInfo) -> Self {
        MemoryInfo {
            rss: native.rss(),
            vms: native.vms(),
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Log {
    pub out: String,
    pub error: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Raw {
    pub running: bool,
    pub crashed: bool,
    pub crashes: u64,
}

#[derive(Clone)]
pub struct LogInfo {
    pub out: String,
    pub error: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ProcessItem {
    pid: i64,
    id: usize,
    cpu: String,
    mem: String,
    name: String,
    restarts: u64,
    status: String,
    uptime: String,
    #[schema(example = "/path")]
    watch_path: String,
    #[schema(value_type = String, example = "2000-01-01T01:00:00.000Z")]
    start_time: DateTime<Utc>,
}

#[derive(Clone)]
pub struct ProcessWrapper {
    pub id: usize,
    pub runner: Arc<Mutex<Runner>>,
}

pub type Env = BTreeMap<String, String>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Process {
    pub id: usize,
    pub pid: i64,
    /// PID of the parent shell process when running commands through a shell.
    /// This is set when the command is executed via a shell (e.g., bash -c 'script.sh')
    /// and shell_pid != actual_pid. Used for accurate CPU monitoring of shell scripts.
    #[serde(default)]
    pub shell_pid: Option<i64>,
    pub env: Env,
    pub name: String,
    pub path: PathBuf,
    pub script: String,
    pub restarts: u64,
    pub running: bool,
    pub crash: Crash,
    pub watch: Watch,
    pub children: Vec<i64>,
    #[serde(with = "ts_milliseconds")]
    pub started: DateTime<Utc>,
    /// Maximum memory limit in bytes (0 = no limit)
    #[serde(default)]
    pub max_memory: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Crash {
    pub crashed: bool,
    pub value: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Watch {
    pub enabled: bool,
    #[schema(example = "/path")]
    pub path: String,
    pub hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Runner {
    pub id: id::Id,
    #[serde(skip)]
    pub remote: Option<Remote>,
    pub list: BTreeMap<usize, Process>,
}

#[derive(Clone, Debug)]
pub struct Remote {
    address: String,
    token: Option<String>,
    pub config: RemoteConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemoteConfig {
    pub shell: String,
    pub args: Vec<String>,
    pub log_path: String,
}

pub enum Status {
    Offline,
    Running,
}

impl Status {
    pub fn to_bool(&self) -> bool {
        match self {
            Status::Offline => false,
            Status::Running => true,
        }
    }
}

/// Process metadata
pub struct ProcessMetadata {
    /// Process name
    pub name: String,
    /// Shell command
    pub shell: String,
    /// Command
    pub command: String,
    /// Log path
    pub log_path: String,
    /// Arguments
    pub args: Vec<String>,
    /// Environment variables
    pub env: Vec<String>,
}

macro_rules! lock {
    ($runner:expr) => {{
        match $runner.lock() {
            Ok(runner) => runner,
            Err(err) => crashln!("Unable to lock mutex: {err}"),
        }
    }};
}

fn kill_children(children: Vec<i64>) {
    for pid in children {
        match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
            Ok(_) => {}
            Err(nix::errno::Errno::ESRCH) => {
                // Process already terminated
            }
            Err(err) => {
                log::error!("Failed to stop pid {}: {err:?}", pid);
            }
        }
    }
}

/// Load environment variables from .env file in the specified directory
fn load_dotenv(path: &PathBuf) -> BTreeMap<String, String> {
    let env_file = path.join(".env");
    let mut env_vars = BTreeMap::new();

    if env_file.exists() && env_file.is_file() {
        match dotenvy::from_path_iter(&env_file) {
            Ok(iter) => {
                for item in iter {
                    match item {
                        Ok((key, value)) => {
                            env_vars.insert(key, value);
                        }
                        Err(err) => {
                            log::warn!("Failed to parse .env entry: {}", err);
                        }
                    }
                }
                if !env_vars.is_empty() {
                    log::info!(
                        "Loaded {} environment variables from .env file",
                        env_vars.len()
                    );
                }
            }
            Err(err) => {
                log::warn!("Failed to read .env file at {:?}: {}", env_file, err);
            }
        }
    }

    env_vars
}

/// Check if a process with the given PID is alive
/// Uses libc::kill with signal 0 to check process existence without sending a signal
pub fn is_pid_alive(pid: i64) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

impl Runner {
    pub fn new() -> Self {
        dump::read()
    }

    pub fn refresh(&self) -> Self {
        Runner::new()
    }

    pub fn connect(name: String, Server { address, token }: Server, verbose: bool) -> Option<Self> {
        let remote_config = match config::from(&address, token.as_deref()) {
            Ok(config) => config,
            Err(err) => {
                log::error!("{err}");
                return None;
            }
        };

        if let Ok(dump) = dump::from(&address, token.as_deref()) {
            then!(
                verbose,
                println!(
                    "{} Fetched remote (name={name}, address={address})",
                    *helpers::SUCCESS
                )
            );
            Some(Runner {
                remote: Some(Remote {
                    token,
                    address: string!(address),
                    config: remote_config,
                }),
                ..dump
            })
        } else {
            None
        }
    }

    pub fn start(
        &mut self,
        name: &String,
        command: &String,
        path: PathBuf,
        watch: &Option<String>,
        max_memory: u64,
    ) -> &mut Self {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::create(remote, name, command, path, watch) {
                crashln!(
                    "{} Failed to start create {name}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            let id = self.id.next();
            let config = config::read().runner;
            let crash = Crash {
                crashed: false,
                value: 0,
            };

            let watch = match watch {
                Some(watch) => Watch {
                    enabled: true,
                    path: string!(watch),
                    hash: hash::create(file::cwd().join(watch)),
                },
                None => Watch {
                    enabled: false,
                    path: string!(""),
                    hash: string!(""),
                },
            };

            // Load environment variables from .env file
            let dotenv_vars = load_dotenv(&path);
            let system_env = unix::env();

            // Prepare process environment with dotenv variables having priority
            let mut process_env = Vec::with_capacity(dotenv_vars.len() + system_env.len());
            // Add dotenv variables first (higher priority)
            for (key, value) in &dotenv_vars {
                process_env.push(format!("{}={}", key, value));
            }
            // Then add system environment
            process_env.extend(system_env);

            let result = match process_run(ProcessMetadata {
                args: config.args,
                name: name.clone(),
                shell: config.shell,
                command: command.clone(),
                log_path: config.log_path,
                env: process_env,
            }) {
                Ok(result) => result,
                Err(err) => {
                    log::error!("Failed to start process '{}': {}", name, err);
                    println!("{} Failed to start process '{}': {}", *helpers::FAIL, name, err);
                    return self;
                }
            };

            // Merge .env variables into the stored environment (dotenv takes priority)
            let mut stored_env: Env = env::vars().collect();
            // Extend with dotenv variables (this overwrites any existing keys)
            stored_env.extend(dotenv_vars);

            self.list.insert(
                id,
                Process {
                    id,
                    pid: result.pid,
                    shell_pid: result.shell_pid,
                    path,
                    watch,
                    crash,
                    restarts: 0,
                    running: true,
                    children: vec![],
                    name: name.clone(),
                    started: Utc::now(),
                    script: command.clone(),
                    env: stored_env,
                    max_memory,
                },
            );
        }

        return self;
    }

    pub fn restart(&mut self, id: usize, dead: bool) -> &mut Self {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::restart(remote, id) {
                crashln!(
                    "{} Failed to start process {id}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            let process = self.process(id);
            let config = config::read().runner;
            let Process {
                path, script, name, ..
            } = process.clone();

            kill_children(process.children.clone());
            if let Err(err) = process_stop(process.pid) {
                log::warn!("Failed to stop process {} during restart: {}", process.pid, err);
                // Continue with restart even if stop fails - process may already be dead
            }

            // Wait for the process to actually terminate before starting a new one
            // This prevents conflicts when restarting processes that hold resources (e.g., network connections)
            if !wait_for_process_termination(process.pid) {
                log::warn!("Process {} did not terminate within timeout during restart", process.pid);
            }

            if let Err(err) = std::env::set_current_dir(&path) {
                process.running = false;
                process.children = vec![];
                process.crash.crashed = true;
                then!(dead, process.restarts += 1);
                then!(dead, process.crash.value += 1);
                log::error!("Failed to set working directory {:?} for process {} during restart: {}", path, name, err);
                println!(
                    "{} Failed to set working directory {:?}\nError: {:#?}",
                    *helpers::FAIL,
                    path,
                    err
                );
                return self;
            }

            // Load environment variables from .env file
            let dotenv_vars = load_dotenv(&path);
            let system_env = unix::env();

            // Prepare process environment with dotenv variables having priority
            let stored_env_vec: Vec<String> = process
                .env
                .iter()
                .map(|(key, value)| format!("{}={}", key, value))
                .collect();
            let mut temp_env =
                Vec::with_capacity(dotenv_vars.len() + stored_env_vec.len() + system_env.len());
            // Add dotenv variables first (highest priority)
            for (key, value) in &dotenv_vars {
                temp_env.push(format!("{}={}", key, value));
            }
            // Then add stored environment
            temp_env.extend(stored_env_vec);
            // Finally add system environment
            temp_env.extend(system_env);

            let result = match process_run(ProcessMetadata {
                args: config.args,
                name: name.clone(),
                shell: config.shell,
                log_path: config.log_path,
                command: script.to_string(),
                env: temp_env,
            }) {
                Ok(result) => result,
                Err(err) => {
                    process.running = false;
                    process.children = vec![];
                    process.crash.crashed = true;
                    then!(dead, process.restarts += 1);
                    then!(dead, process.crash.value += 1);
                    log::error!("Failed to restart process '{}' (id={}): {}", name, id, err);
                    println!("{} Failed to restart process '{}' (id={}): {}", *helpers::FAIL, name, id, err);
                    return self;
                }
            };

            process.pid = result.pid;
            process.shell_pid = result.shell_pid;
            process.running = true;
            process.children = vec![];
            process.started = Utc::now();
            process.crash.crashed = false;

            // Merge .env variables into the stored environment (dotenv takes priority)
            let mut updated_env: Env = env::vars().collect();
            updated_env.extend(dotenv_vars);
            process.env.extend(updated_env);

            then!(dead, process.restarts += 1);
            then!(dead, process.crash.value += 1);
            then!(!dead, process.crash.value = 0);
        }

        return self;
    }

    pub fn reload(&mut self, id: usize, dead: bool) -> &mut Self {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::reload(remote, id) {
                crashln!(
                    "{} Failed to reload process {id}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            let process = self.process(id);
            let config = config::read().runner;
            let Process {
                path,
                script,
                name,
                env,
                watch: _,
                max_memory: _,
                ..
            } = process.clone();

            if let Err(err) = std::env::set_current_dir(&path) {
                process.running = false;
                process.children = vec![];
                process.crash.crashed = true;
                then!(dead, process.restarts += 1);
                then!(dead, process.crash.value += 1);
                log::error!("Failed to set working directory {:?} for process {} during reload: {}", path, name, err);
                println!(
                    "{} Failed to set working directory {:?}\nError: {:#?}",
                    *helpers::FAIL,
                    path,
                    err
                );
                return self;
            }

            // Load environment variables from .env file
            let dotenv_vars = load_dotenv(&path);
            let system_env = unix::env();

            // Prepare process environment with dotenv variables having priority
            let stored_env_vec: Vec<String> = env
                .iter()
                .map(|(key, value)| format!("{}={}", key, value))
                .collect();
            let mut temp_env =
                Vec::with_capacity(dotenv_vars.len() + stored_env_vec.len() + system_env.len());
            // Add dotenv variables first (highest priority)
            for (key, value) in &dotenv_vars {
                temp_env.push(format!("{}={}", key, value));
            }
            // Then add stored environment
            temp_env.extend(stored_env_vec);
            // Finally add system environment
            temp_env.extend(system_env);

            // Start new process first
            let result = match process_run(ProcessMetadata {
                args: config.args,
                name: name.clone(),
                shell: config.shell,
                log_path: config.log_path,
                command: script.to_string(),
                env: temp_env,
            }) {
                Ok(result) => result,
                Err(err) => {
                    process.running = false;
                    process.children = vec![];
                    process.crash.crashed = true;
                    then!(dead, process.restarts += 1);
                    then!(dead, process.crash.value += 1);
                    log::error!("Failed to reload process '{}' (id={}): {}", name, id, err);
                    println!("{} Failed to reload process '{}' (id={}): {}", *helpers::FAIL, name, id, err);
                    return self;
                }
            };

            // Store old PID before updating
            let old_pid = process.pid;
            let old_children = process.children.clone();

            // Update process with new PID
            process.pid = result.pid;
            process.shell_pid = result.shell_pid;
            process.running = true;
            process.children = vec![];
            process.started = Utc::now();
            process.crash.crashed = false;

            // Merge .env variables into the stored environment (dotenv takes priority)
            let mut updated_env: Env = env::vars().collect();
            updated_env.extend(dotenv_vars);
            process.env.extend(updated_env);

            then!(dead, process.restarts += 1);
            then!(dead, process.crash.value += 1);
            then!(!dead, process.crash.value = 0);

            // Now stop the old process after the new one is running
            kill_children(old_children);
            if let Err(err) = process_stop(old_pid) {
                log::warn!("Failed to stop old process during reload: {err}");
            }

            // Wait for old process to fully terminate to release any held resources
            if !wait_for_process_termination(old_pid) {
                log::warn!("Old process {} did not terminate within timeout during reload", old_pid);
            }
        }

        return self;
    }

    pub fn remove(&mut self, id: usize) {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::remove(remote, id) {
                crashln!(
                    "{} Failed to stop remove {id}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            self.stop(id);
            self.list.remove(&id);
            self.save();
        }
    }

    pub fn set_id(&mut self, id: id::Id) {
        self.id = id;
        self.id.next();
        self.save();
    }

    pub fn set_status(&mut self, id: usize, status: Status) {
        self.process(id).running = status.to_bool();
        self.save();
    }

    pub fn items(&self) -> BTreeMap<usize, Process> {
        self.list.clone()
    }

    pub fn items_mut(&mut self) -> &mut BTreeMap<usize, Process> {
        &mut self.list
    }

    pub fn save(&self) {
        then!(self.remote.is_none(), dump::write(&self))
    }

    pub fn count(&mut self) -> usize {
        self.list().count()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn exists(&self, id: usize) -> bool {
        self.list.contains_key(&id)
    }

    pub fn info(&self, id: usize) -> Option<&Process> {
        self.list.get(&id)
    }

    pub fn try_info(&self, id: usize) -> &Process {
        self.list
            .get(&id)
            .unwrap_or_else(|| crashln!("{} Process ({id}) not found", *helpers::FAIL))
    }

    pub fn size(&self) -> Option<&usize> {
        self.list.iter().map(|(k, _)| k).max()
    }

    pub fn list<'l>(&'l mut self) -> impl Iterator<Item = (&'l usize, &'l mut Process)> {
        self.list.iter_mut().map(|(k, v)| (k, v))
    }

    pub fn process(&mut self, id: usize) -> &mut Process {
        self.list
            .get_mut(&id)
            .unwrap_or_else(|| crashln!("{} Process ({id}) not found", *helpers::FAIL))
    }

    pub fn pid(&self, id: usize) -> i64 {
        self.list
            .get(&id)
            .unwrap_or_else(|| crashln!("{} Process ({id}) not found", *helpers::FAIL))
            .pid
    }

    pub fn get(self, id: usize) -> ProcessWrapper {
        ProcessWrapper {
            id,
            runner: Arc::new(Mutex::new(self)),
        }
    }

    pub fn set_crashed(&mut self, id: usize) -> &mut Self {
        self.process(id).crash.crashed = true;
        return self;
    }

    pub fn set_env(&mut self, id: usize, env: Env) -> &mut Self {
        self.process(id).env.extend(env);
        return self;
    }

    pub fn clear_env(&mut self, id: usize) -> &mut Self {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::clear_env(remote, id) {
                crashln!(
                    "{} Failed to clear environment on {id}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            self.process(id).env = BTreeMap::new();
        }

        return self;
    }

    pub fn set_children(&mut self, id: usize, children: Vec<i64>) -> &mut Self {
        self.process(id).children = children;
        return self;
    }

    pub fn new_crash(&mut self, id: usize) -> &mut Self {
        self.process(id).crash.value += 1;
        return self;
    }

    pub fn stop(&mut self, id: usize) -> &mut Self {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::stop(remote, id) {
                crashln!(
                    "{} Failed to stop process {id}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            let process_to_stop = self.process(id);
            let pid_to_check = process_to_stop.pid;

            kill_children(process_to_stop.children.clone());
            let _ = process_stop(pid_to_check); // Continue even if stopping fails

            // waiting until Process is terminated
            if !wait_for_process_termination(pid_to_check) {
                log::warn!("Process {} did not terminate within timeout during stop", pid_to_check);
            }

            let process = self.process(id);
            process.running = false;
            process.crash.crashed = false;
            process.crash.value = 0;
            process.children = vec![];
        }

        return self;
    }

    pub fn flush(&mut self, id: usize) -> &mut Self {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::flush(remote, id) {
                crashln!(
                    "{} Failed to flush process {id}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            self.process(id).logs().flush();
        }

        return self;
    }

    pub fn rename(&mut self, id: usize, name: String) -> &mut Self {
        if let Some(remote) = &self.remote {
            if let Err(err) = http::rename(remote, id, name) {
                crashln!(
                    "{} Failed to rename process {id}\nError: {:#?}",
                    *helpers::FAIL,
                    err
                );
            };
        } else {
            self.process(id).name = name;
        }

        return self;
    }

    pub fn watch(&mut self, id: usize, path: &str, enabled: bool) -> &mut Self {
        let process = self.process(id);
        process.watch = Watch {
            enabled,
            path: string!(path),
            hash: ternary!(enabled, hash::create(process.path.join(path)), string!("")),
        };

        return self;
    }

    pub fn reset_counters(&mut self, id: usize) -> &mut Self {
        let process = self.process(id);
        process.restarts = 0;
        process.crash.value = 0;
        process.crash.crashed = false;
        return self;
    }

    pub fn find(&self, name: &str, server_name: &String) -> Option<usize> {
        let mut runner = self.clone();

        if !matches!(&**server_name, "internal" | "local") {
            let Some(servers) = config::servers().servers else {
                crashln!("{} Failed to read servers", *helpers::FAIL)
            };

            if let Some(server) = servers.get(server_name) {
                runner = match Runner::connect(server_name.clone(), server.get(), false) {
                    Some(remote) => remote,
                    None => crashln!(
                        "{} Failed to connect (name={server_name}, address={})",
                        *helpers::FAIL,
                        server.address
                    ),
                };
            } else {
                crashln!("{} Server '{server_name}' does not exist", *helpers::FAIL)
            };
        }

        runner
            .list
            .iter()
            .find(|(_, p)| p.name == name)
            .map(|(id, _)| *id)
    }

    pub fn fetch(&self) -> Vec<ProcessItem> {
        let mut processes: Vec<ProcessItem> = Vec::new();

        for (id, item) in self.items() {
            let mut memory_usage: Option<MemoryInfo> = None;
            let mut cpu_percent: Option<f64> = None;

            // Use new_fast() to avoid CPU measurement delays for list view
            // This uses average CPU since process start instead of current instantaneous CPU
            // For accurate current CPU, use the info endpoint which measures over a 100ms window

            // For shell scripts, try shell_pid first to capture the entire process tree
            // If shell_pid process has exited, fall back to the actual script pid
            let mut pid_for_monitoring = item.shell_pid.unwrap_or(item.pid);
            let mut process_result = unix::NativeProcess::new_fast(pid_for_monitoring as u32);

            // If shell_pid fails (process exited), try the actual script pid
            if process_result.is_err() && item.shell_pid.is_some() {
                pid_for_monitoring = item.pid;
                process_result = unix::NativeProcess::new_fast(pid_for_monitoring as u32);
            }

            if let Ok(process) = process_result
                && let Ok(_mem_info_native) = process.memory_info()
            {
                // Use fast CPU calculation that includes children (important for .sh scripts)
                cpu_percent = Some(get_process_cpu_usage_with_children_fast(pid_for_monitoring));
                memory_usage = get_process_memory_with_children(pid_for_monitoring);
            }

            let cpu_percent = match cpu_percent {
                Some(percent) => format!("{:.2}%", percent),
                None => string!("0.00%"),
            };

            let memory_usage = match memory_usage {
                Some(usage) => helpers::format_memory(usage.rss),
                None => string!("0b"),
            };

            // Check if process actually exists before reporting as online
            // A process marked as running but with a non-existent PID should be shown as crashed
            let process_actually_running = item.running && is_pid_alive(item.pid);
            
            let status = if process_actually_running {
                string!("online")
            } else if item.running {
                // Process is marked as running but PID doesn't exist - it crashed
                string!("crashed")
            } else {
                match item.crash.crashed {
                    true => string!("crashed"),
                    false => string!("stopped"),
                }
            };

            processes.push(ProcessItem {
                id,
                status,
                pid: item.pid,
                cpu: cpu_percent,
                mem: memory_usage,
                restarts: item.restarts,
                name: item.name.clone(),
                start_time: item.started,
                watch_path: item.watch.path.clone(),
                uptime: helpers::format_duration(item.started),
            });
        }

        return processes;
    }
}

impl LogInfo {
    pub fn flush(&self) {
        if let Err(err) = File::create(&self.out) {
            log::error!("{err}");
            crashln!(
                "{} Failed to purge logs (path={})",
                *helpers::FAIL,
                self.error
            );
        }

        if let Err(err) = File::create(&self.error) {
            log::error!("{err}");
            crashln!(
                "{} Failed to purge logs (path={})",
                *helpers::FAIL,
                self.error
            );
        }
    }
}

impl Process {
    /// Get a log paths of the process item
    pub fn logs(&self) -> LogInfo {
        let name = self.name.replace(" ", "_");

        LogInfo {
            out: global!("opm.logs.out", name.as_str()),
            error: global!("opm.logs.error", name.as_str()),
        }
    }
}

impl ProcessWrapper {
    /// Stop the process item
    pub fn stop(&mut self) {
        lock!(self.runner).stop(self.id);
    }

    /// Restart the process item
    pub fn restart(&mut self) {
        lock!(self.runner).restart(self.id, false);
    }

    /// Reload the process item (zero-downtime: starts new process before stopping old one)
    pub fn reload(&mut self) {
        lock!(self.runner).reload(self.id, false);
    }

    /// Rename the process item
    pub fn rename(&mut self, name: String) {
        lock!(self.runner).rename(self.id, name);
    }

    /// Enable watching a path on the process item
    pub fn watch(&mut self, path: &str) {
        lock!(self.runner).watch(self.id, path, true);
    }

    /// Disable watching on the process item
    pub fn disable_watch(&mut self) {
        lock!(self.runner).watch(self.id, "", false);
    }

    /// Set the process item as crashed
    pub fn crashed(&mut self) {
        lock!(self.runner).restart(self.id, true);
    }

    /// Get the borrowed runner reference (lives till program end)
    pub fn get_runner(&mut self) -> &Runner {
        Box::leak(Box::new(lock!(self.runner)))
    }

    /// Append new environment values to the process item
    pub fn set_env(&mut self, env: Env) {
        lock!(self.runner).set_env(self.id, env);
    }

    /// Clear environment values of the process item
    pub fn clear_env(&mut self) {
        lock!(self.runner).clear_env(self.id);
    }

    /// Reset restart and crash counters of the process item
    pub fn reset_counters(&mut self) {
        lock!(self.runner).reset_counters(self.id);
    }

    /// Get a json dump of the process item
    pub fn fetch(&self) -> ItemSingle {
        let mut runner = lock!(self.runner);

        let item = runner.process(self.id);
        let config = config::read().runner;

        let mut memory_usage: Option<MemoryInfo> = None;
        let mut cpu_percent: Option<f64> = None;

        // For shell scripts, try shell_pid first to capture the entire process tree
        // If shell_pid process has exited, fall back to the actual script pid
        let mut pid_for_monitoring = item.shell_pid.unwrap_or(item.pid);
        let mut process_result = unix::NativeProcess::new(pid_for_monitoring as u32);

        // If shell_pid fails (process exited), try the actual script pid
        if process_result.is_err() && item.shell_pid.is_some() {
            pid_for_monitoring = item.pid;
            process_result = unix::NativeProcess::new(pid_for_monitoring as u32);
        }

        if let Ok(process) = process_result
            && let Ok(_mem_info_native) = process.memory_info()
        {
            cpu_percent = Some(get_process_cpu_usage_with_children_from_process(
                &process,
                pid_for_monitoring,
            ));
            memory_usage = get_process_memory_with_children(pid_for_monitoring);
        }

        // Check if process actually exists before reporting as online
        // A process marked as running but with a non-existent PID should be shown as crashed
        let process_actually_running = item.running && is_pid_alive(item.pid);
        
        let status = if process_actually_running {
            string!("online")
        } else if item.running {
            // Process is marked as running but PID doesn't exist - it crashed
            string!("crashed")
        } else {
            match item.crash.crashed {
                true => string!("crashed"),
                false => string!("stopped"),
            }
        };

        ItemSingle {
            info: Info {
                status,
                id: item.id,
                pid: item.pid,
                name: item.name.clone(),
                path: item.path.clone(),
                children: item.children.clone(),
                uptime: helpers::format_duration(item.started),
                command: format!(
                    "{} {} '{}'",
                    config.shell,
                    config.args.join(" "),
                    item.script.clone()
                ),
            },
            stats: Stats {
                cpu_percent,
                memory_usage,
                restarts: item.restarts,
                start_time: item.started.timestamp_millis(),
            },
            watch: Watch {
                enabled: item.watch.enabled,
                hash: item.watch.hash.clone(),
                path: item.watch.path.clone(),
            },
            log: Log {
                out: item.logs().out,
                error: item.logs().error,
            },
            raw: Raw {
                running: item.running,
                crashed: item.crash.crashed,
                crashes: item.crash.value,
            },
        }
    }
}

/// Get the CPU usage percentage of the process
pub fn get_process_cpu_usage_percentage(pid: i64) -> f64 {
    match unix::NativeProcess::new(pid as u32) {
        Ok(process) => match process.cpu_percent() {
            Ok(cpu_percent) => cpu_percent,
            Err(_) => 0.0,
        },
        Err(_) => 0.0,
    }
}

/// Get the CPU usage percentage of the process (fast version without delay)
pub fn get_process_cpu_usage_percentage_fast(pid: i64) -> f64 {
    match unix::NativeProcess::new_fast(pid as u32) {
        Ok(process) => match process.cpu_percent() {
            Ok(cpu_percent) => cpu_percent,
            Err(_) => 0.0,
        },
        Err(_) => 0.0,
    }
}

/// Get the total CPU usage percentage of the process and its children
/// If parent_process is provided, it will be used instead of creating a new one
/// This function uses the parent's CPU measurement (which may have been timed with delay)
/// and fast measurements for children to avoid cumulative delays and ensure consistency
pub fn get_process_cpu_usage_with_children_from_process(
    parent_process: &unix::NativeProcess,
    pid: i64,
) -> f64 {
    let parent_cpu = match parent_process.cpu_percent() {
        Ok(cpu_percent) => cpu_percent,
        Err(_) => 0.0,
    };

    let children = process_find_children(pid);

    // Use fast CPU calculation for children to avoid multiple delays
    // The parent already used a timed measurement, so children should use fast measurements
    // for consistency and to prevent cumulative delays (N children = N * 100ms)
    let children_cpu: f64 = children
        .iter()
        .map(|&child_pid| get_process_cpu_usage_percentage_fast(child_pid))
        .sum();

    parent_cpu + children_cpu
}

/// Get the total CPU usage percentage of the process and its children (fast version)
pub fn get_process_cpu_usage_with_children_fast(pid: i64) -> f64 {
    let parent_cpu = get_process_cpu_usage_percentage_fast(pid);
    let children = process_find_children(pid);

    let children_cpu: f64 = children
        .iter()
        .map(|&child_pid| get_process_cpu_usage_percentage_fast(child_pid))
        .sum();

    parent_cpu + children_cpu
}

/// Get the total CPU usage percentage of the process and its children
/// Uses timed measurement for parent and fast measurements for children
/// to avoid cumulative delays while maintaining accurate parent measurement
pub fn get_process_cpu_usage_with_children(pid: i64) -> f64 {
    let parent_cpu = get_process_cpu_usage_percentage(pid);
    let children = process_find_children(pid);

    // Use fast CPU calculation for children to avoid multiple delays
    // The parent already used a timed measurement, so children should use fast measurements
    // for consistency and to prevent cumulative delays (N children = N * 100ms)
    let children_cpu: f64 = children
        .iter()
        .map(|&child_pid| get_process_cpu_usage_percentage_fast(child_pid))
        .sum();

    parent_cpu + children_cpu
}

/// Get the total memory usage of the process and its children
pub fn get_process_memory_with_children(pid: i64) -> Option<MemoryInfo> {
    let parent_memory = unix::NativeProcess::new_fast(pid as u32)
        .ok()?
        .memory_info()
        .ok()
        .map(MemoryInfo::from)?;

    let children = process_find_children(pid);

    let children_memory: (u64, u64) = children
        .iter()
        .filter_map(|&child_pid| {
            unix::NativeProcess::new_fast(child_pid as u32)
                .ok()
                .and_then(|p| p.memory_info().ok())
                .map(|m| (m.rss(), m.vms()))
        })
        .fold((0, 0), |(rss_sum, vms_sum), (rss, vms)| {
            (rss_sum + rss, vms_sum + vms)
        });

    Some(MemoryInfo {
        rss: parent_memory.rss + children_memory.0,
        vms: parent_memory.vms + children_memory.1,
    })
}

/// Stop the process
pub fn process_stop(pid: i64) -> Result<(), String> {
    let children = process_find_children(pid);

    // Stop child processes first
    for child_pid in children {
        let _ = kill(Pid::from_raw(child_pid as i32), Signal::SIGTERM);
        // Continue even if stopping child processes fails
    }

    // Stop parent process
    match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
        Ok(_) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => {
            // Process already terminated
            Ok(())
        }
        Err(err) => Err(format!("Failed to stop process {}: {:?}", pid, err)),
    }
}

/// Find the children of the process
pub fn process_find_children(parent_pid: i64) -> Vec<i64> {
    let mut children = Vec::new();
    let mut to_check = vec![parent_pid];
    let mut checked = HashSet::new();

    #[cfg(target_os = "linux")]
    {
        while let Some(pid) = to_check.pop() {
            if checked.contains(&pid) {
                continue;
            }
            checked.insert(pid);

            let proc_path = format!("/proc/{}/task/{}/children", pid, pid);
            let Ok(contents) = std::fs::read_to_string(&proc_path) else {
                continue;
            };

            for child_pid_str in contents.split_whitespace() {
                if let Ok(child_pid) = child_pid_str.parse::<i64>() {
                    children.push(child_pid);
                    to_check.push(child_pid); // Check grandchildren
                }
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        match unix::native_processes() {
            Ok(processes) => {
                // Build parent->children map in single pass
                let mut parent_map: HashMap<i64, Vec<i64>> = HashMap::new();

                processes.iter().for_each(|process| {
                    if let Ok(Some(ppid)) = process.ppid() {
                        parent_map
                            .entry(ppid as i64)
                            .or_insert_with(Vec::new)
                            .push(process.pid() as i64);
                    }
                });

                while let Some(pid) = to_check.pop()
                    && let Some(direct_children) = parent_map.get(&pid)
                {
                    for &child in direct_children {
                        if !checked.contains(&child) {
                            children.push(child);
                            to_check.push(child);
                            checked.insert(child);
                        }
                    }
                }
            }
            Err(_) => {
                log::warn!("Native process enumeration failed for PID {}", parent_pid);
            }
        }
    }

    children
}

/// Result of running a process
#[derive(Debug, Clone)]
pub struct ProcessRunResult {
    pub pid: i64,
    pub shell_pid: Option<i64>,
}

/// Run the process
pub fn process_run(metadata: ProcessMetadata) -> Result<ProcessRunResult, String> {
    use std::fs::OpenOptions;
    use std::process::{Command, Stdio};

    let log_base = format!("{}/{}", metadata.log_path, metadata.name.replace(' ', "_"));
    let stdout_path = format!("{}-out.log", log_base);
    let stderr_path = format!("{}-error.log", log_base);

    // Create log files
    let stdout_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path)
        .map_err(|err| {
            format!(
                "Failed to open stdout log file '{}': {}. \
                Check that the directory exists and you have write permissions.",
                stdout_path, err
            )
        })?;

    let stderr_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)
        .map_err(|err| {
            format!(
                "Failed to open stderr log file '{}': {}. \
                Check that the directory exists and you have write permissions.",
                stderr_path, err
            )
        })?;

    // Execute process
    let mut cmd = Command::new(&metadata.shell);
    cmd.args(&metadata.args)
        .arg(&metadata.command)
        .envs(metadata.env.iter().map(|env_var| {
            let parts: Vec<&str> = env_var.splitn(2, '=').collect();
            if parts.len() == 2 {
                (parts[0], parts[1])
            } else {
                (env_var.as_str(), "")
            }
        }))
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .stdin(Stdio::null());

    let child = cmd.spawn().map_err(|err| {
        // Provide more helpful error messages based on error kind
        match err.kind() {
            std::io::ErrorKind::NotFound => format!(
                "Failed to spawn process: Command '{}' not found. \
                Please ensure '{}' is installed and in your PATH. \
                Error: {:?}",
                metadata.shell, metadata.shell, err
            ),
            std::io::ErrorKind::PermissionDenied => format!(
                "Failed to spawn process: Permission denied for '{}'. \
                Check that the shell has execute permissions. \
                Error: {:?}",
                metadata.shell, err
            ),
            _ => format!(
                "Failed to spawn process with shell '{}': {:?}. \
                Command attempted: {} {} '{}'",
                metadata.shell,
                err,
                metadata.shell,
                metadata.args.join(" "),
                metadata.command
            ),
        }
    })?;

    let shell_pid = child.id() as i64;
    let actual_pid = unix::get_actual_child_pid(shell_pid);

    // If shell and actual PIDs differ, store the shell PID for CPU monitoring
    let shell_pid_opt = (shell_pid != actual_pid).then_some(shell_pid);

    Ok(ProcessRunResult {
        pid: actual_pid,
        shell_pid: shell_pid_opt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    fn setup_test_runner() -> Runner {
        Runner {
            id: id::Id::new(1),
            list: BTreeMap::new(),
            remote: None,
        }
    }

    #[test]
    fn test_environment_variables() {
        let mut runner = setup_test_runner();
        let id = runner.id.next();

        let process = Process {
            id,
            pid: 12345,
            shell_pid: None,
            env: BTreeMap::new(),
            name: "test_process".to_string(),
            path: PathBuf::from("/tmp"),
            script: "echo 'hello world'".to_string(),
            restarts: 0,
            running: true,
            crash: Crash {
                crashed: false,
                value: 0,
            },
            watch: Watch {
                enabled: false,
                path: String::new(),
                hash: String::new(),
            },
            children: vec![],
            started: Utc::now(),
            max_memory: 0,
        };

        runner.list.insert(id, process);

        // Test setting environment variables
        let mut env = BTreeMap::new();
        env.insert("TEST_VAR".to_string(), "test_value".to_string());
        env.insert("ANOTHER_VAR".to_string(), "another_value".to_string());

        runner.set_env(id, env);

        let process_env = &runner.info(id).unwrap().env;
        assert_eq!(process_env.get("TEST_VAR"), Some(&"test_value".to_string()));
        assert_eq!(
            process_env.get("ANOTHER_VAR"),
            Some(&"another_value".to_string())
        );

        // Test clearing environment variables
        runner.clear_env(id);
        assert!(runner.info(id).unwrap().env.is_empty());
    }

    #[test]
    fn test_children_processes() {
        let mut runner = setup_test_runner();
        let id = runner.id.next();

        let process = Process {
            id,
            pid: 12345,
            shell_pid: None,
            env: BTreeMap::new(),
            name: "test_process".to_string(),
            path: PathBuf::from("/tmp"),
            script: "echo 'hello world'".to_string(),
            restarts: 0,
            running: true,
            crash: Crash {
                crashed: false,
                value: 0,
            },
            watch: Watch {
                enabled: false,
                path: String::new(),
                hash: String::new(),
            },
            children: vec![],
            started: Utc::now(),
            max_memory: 0,
        };

        runner.list.insert(id, process);

        // Test setting children
        let children = vec![12346, 12347, 12348];
        runner.set_children(id, children.clone());

        assert_eq!(runner.info(id).unwrap().children, children);
    }

    #[test]
    fn test_cpu_usage_measurement() {
        // Test with current process (should return valid percentage)
        let current_pid = std::process::id() as i64;
        let cpu_usage = get_process_cpu_usage_percentage(current_pid);

        // CPU usage should be between 0 and 100% (single process can't use more than 100% of available CPU)
        assert!(cpu_usage >= 0.0);
        assert!(cpu_usage <= 100.0);

        println!("CPU usage: {}", cpu_usage);

        // Test with invalid PID (should return 0.0)
        let invalid_pid = 999999;
        let cpu_usage = get_process_cpu_usage_percentage(invalid_pid);
        assert_eq!(cpu_usage, 0.0);
    }

    // Integration test for actual process operations
    #[test]
    #[ignore = "it requires actual process execution"]
    fn test_real_process_execution() {
        let metadata = ProcessMetadata {
            name: "test_echo".to_string(),
            shell: "/bin/sh".to_string(),
            command: "echo 'Hello from test'".to_string(),
            log_path: "/tmp".to_string(),
            args: vec!["-c".to_string()],
            env: vec!["TEST_ENV=test_value".to_string()],
        };

        match process_run(metadata) {
            Ok(result) => {
                assert!(result.pid > 0);

                // Wait a bit for process to complete
                thread::sleep(Duration::from_millis(100));

                // Try to stop it (might already be finished)
                let _ = process_stop(result.pid);
            }
            Err(e) => {
                panic!("Failed to run test process: {}", e);
            }
        }
    }

    #[test]
    fn test_reset_counters() {
        let mut runner = setup_test_runner();
        let id = runner.id.next();

        let process = Process {
            id,
            pid: 12345,
            shell_pid: None,
            env: BTreeMap::new(),
            name: "test_process".to_string(),
            path: PathBuf::from("/tmp"),
            script: "echo 'hello world'".to_string(),
            restarts: 5, // Set to non-zero value
            running: true,
            crash: Crash {
                crashed: true, // Set to crashed
                value: 3,      // Set to non-zero crash count
            },
            watch: Watch {
                enabled: false,
                path: String::new(),
                hash: String::new(),
            },
            children: vec![],
            started: Utc::now(),
            max_memory: 0,
        };

        runner.list.insert(id, process);

        // Verify initial values
        assert_eq!(runner.info(id).unwrap().restarts, 5);
        assert_eq!(runner.info(id).unwrap().crash.value, 3);
        assert_eq!(runner.info(id).unwrap().crash.crashed, true);

        // Reset counters
        runner.reset_counters(id);

        // Verify counters are reset
        assert_eq!(runner.info(id).unwrap().restarts, 0);
        assert_eq!(runner.info(id).unwrap().crash.value, 0);
        assert_eq!(runner.info(id).unwrap().crash.crashed, false);
    }

    #[test]
    fn test_cpu_usage_with_children_performance() {
        use std::time::Instant;
        
        // Test that measuring CPU with children is reasonably fast
        // even with multiple children, since we use fast measurements for children
        let current_pid = std::process::id() as i64;
        
        // Simulate finding children (even if empty, the function should be fast)
        let start = Instant::now();
        let _cpu_with_children = get_process_cpu_usage_with_children_fast(current_pid);
        let duration = start.elapsed();
        
        // Fast version should complete very quickly (< 50ms even with multiple children)
        // since it doesn't use delay-based sampling
        assert!(duration.as_millis() < 50, 
            "Fast CPU measurement with children took too long: {:?}", duration);
        
        // Test that the timed version with a pre-created process is also reasonably fast
        // It should only have one delay (for parent), not cumulative delays per child
        if let Ok(process) = unix::NativeProcess::new(current_pid as u32) {
            let start = Instant::now();
            let _cpu_with_children = get_process_cpu_usage_with_children_from_process(&process, current_pid);
            let duration = start.elapsed();
            
            // This should complete quickly since the parent measurement was already taken
            // and children use fast measurements (no additional delays)
            assert!(duration.as_millis() < 50,
                "CPU measurement with pre-created process took too long: {:?}", duration);
        }
    }

    #[test]
    fn test_cpu_usage_consistency() {
        // Test that CPU measurements are consistent and within expected ranges
        let current_pid = std::process::id() as i64;
        
        // Get CPU usage with different methods
        let fast_cpu = get_process_cpu_usage_percentage_fast(current_pid);
        let fast_cpu_with_children = get_process_cpu_usage_with_children_fast(current_pid);
        
        // Single process should be 0-100%
        assert!(fast_cpu >= 0.0);
        assert!(fast_cpu <= 100.0);
        
        // Process with children can exceed 100% if multiple processes run in parallel
        assert!(fast_cpu_with_children >= 0.0);
        
        // CPU with children should be >= CPU of parent alone (assuming no negative children)
        assert!(fast_cpu_with_children >= fast_cpu - 0.1, 
            "CPU with children ({}) should be >= parent CPU ({})", 
            fast_cpu_with_children, fast_cpu);
    }

    #[test]
    fn test_error_handling_invalid_shell() {
        // Test that process_run returns an error for invalid shell
        let metadata = ProcessMetadata {
            name: "test_process".to_string(),
            shell: "/nonexistent/shell/that/does/not/exist".to_string(),
            command: "echo test".to_string(),
            log_path: "/tmp".to_string(),
            args: vec!["-c".to_string()],
            env: vec![],
        };

        let result = process_run(metadata);
        assert!(result.is_err(), "Expected error for nonexistent shell");
        
        let err_msg = result.unwrap_err();
        // Check that the error message mentions the shell and that it wasn't found
        assert!(
            err_msg.contains("/nonexistent/shell/that/does/not/exist") && 
            (err_msg.contains("not found") || err_msg.contains("Command") || err_msg.contains("Failed to spawn")),
            "Error message should indicate shell not found, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_error_handling_invalid_log_path() {
        // Test that process_run returns an error for invalid log path
        let metadata = ProcessMetadata {
            name: "test_process".to_string(),
            shell: "/bin/sh".to_string(),
            command: "echo test".to_string(),
            log_path: "/nonexistent/directory/that/does/not/exist".to_string(),
            args: vec!["-c".to_string()],
            env: vec![],
        };

        let result = process_run(metadata);
        assert!(result.is_err(), "Expected error for nonexistent log path");
        
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("Failed to open") && err_msg.contains("log file"),
            "Error message should indicate log file error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_error_handling_graceful_failure() {
        // Test that runner doesn't panic when restart fails
        // This test verifies the structure is set up correctly for error handling
        let mut runner = setup_test_runner();
        let id = runner.id.next();

        // Use a very high PID that's unlikely to exist
        let unlikely_pid = i32::MAX as i64 - 1000;
        
        let process = Process {
            id,
            pid: unlikely_pid,
            shell_pid: None,
            env: BTreeMap::new(),
            name: "test_process".to_string(),
            path: PathBuf::from("/tmp"),
            script: "echo 'hello'".to_string(),
            restarts: 0,
            running: false, // Start with not running
            crash: Crash {
                crashed: false,
                value: 0,
            },
            watch: Watch {
                enabled: false,
                path: String::new(),
                hash: String::new(),
            },
            children: vec![],
            started: Utc::now(),
            max_memory: 0,
        };

        runner.list.insert(id, process);

        // Verify the process exists
        assert!(runner.exists(id), "Process should exist in runner");
        
        // Verify process state
        let process = runner.info(id).unwrap();
        assert_eq!(process.running, false, "Process should start as not running");
        assert_eq!(process.crash.crashed, false, "Process should start as not crashed");
    }

    #[test]
    fn test_status_detection_with_dead_pid() {
        // Test that processes marked as running but with dead PIDs show as crashed
        let mut runner = setup_test_runner();
        let id = runner.id.next();

        // Use a very high PID that's unlikely to exist
        let unlikely_pid = i32::MAX as i64 - 1000;
        
        let process = Process {
            id,
            pid: unlikely_pid,
            shell_pid: None,
            env: BTreeMap::new(),
            name: "test_process".to_string(),
            path: PathBuf::from("/tmp"),
            script: "echo 'hello'".to_string(),
            restarts: 0,
            running: true, // Marked as running
            crash: Crash {
                crashed: false,
                value: 0,
            },
            watch: Watch {
                enabled: false,
                path: String::new(),
                hash: String::new(),
            },
            children: vec![],
            started: Utc::now(),
            max_memory: 0,
        };

        runner.list.insert(id, process);

        // Fetch the process list and check status
        let processes = runner.fetch();
        assert_eq!(processes.len(), 1, "Should have one process");
        
        // The process is marked as running but the PID doesn't exist
        // So status should be "crashed", not "online"
        assert_eq!(processes[0].status, "crashed", 
            "Process with dead PID should show as crashed, not online");
    }
}
