use std::thread;
use std::time::{Duration, SystemTime};

pub mod cpu;
pub mod env;
pub mod memory;
pub mod process_info;
pub mod process_list;

pub use cpu::{get_cpu_percent, get_cpu_percent_fast, get_effective_cpu_count};
pub use env::{env, Vars};
pub use memory::{get_memory_info, NativeMemoryInfo};
pub use process_info::{
    get_parent_pid, get_process_name, get_process_start_time, is_process_zombie,
    get_session_id, get_process_cmdline,
};
pub use process_list::native_processes;

pub const PROCESS_OPERATION_DELAY_MS: u64 = 500;

// Shell names to skip when searching for long-running child processes
// Used in orphan detection and child search logic
const SHELL_NAMES: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "opm"];

// Time window (in seconds) for detecting orphaned child processes
// Processes created within this window are considered potential orphans
const ORPHAN_DETECTION_WINDOW_SECS: u64 = 2;

#[derive(Debug, Clone)]
pub struct NativeProcess {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub memory_info: Option<NativeMemoryInfo>,
    pub cpu_percent: f64,
    pub create_time: SystemTime,
}

impl NativeProcess {
    pub fn new(pid: u32) -> Result<Self, String> {
        let ppid = get_parent_pid(pid as i32)?.map(|p| p as u32);
        let name = get_process_name(pid)?;
        let memory_info = get_memory_info(pid).ok();
        let cpu_percent = get_cpu_percent(pid);
        let create_time = get_process_start_time(pid)?;

        Ok(NativeProcess {
            pid,
            ppid,
            name,
            memory_info,
            cpu_percent,
            create_time,
        })
    }

    /// Create a new NativeProcess using fast CPU calculation.
    /// This is much faster as it doesn't require delay-based sampling,
    /// but returns average CPU usage since process start instead of current usage.
    pub fn new_fast(pid: u32) -> Result<Self, String> {
        let ppid = get_parent_pid(pid as i32)?.map(|p| p as u32);
        let name = get_process_name(pid)?;
        let memory_info = get_memory_info(pid).ok();
        let cpu_percent = get_cpu_percent_fast(pid);
        let create_time = get_process_start_time(pid)?;

        Ok(NativeProcess {
            pid,
            ppid,
            name,
            memory_info,
            cpu_percent,
            create_time,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn ppid(&self) -> Result<Option<u32>, String> {
        Ok(self.ppid)
    }
    pub fn name(&self) -> Result<String, String> {
        Ok(self.name.clone())
    }
    pub fn memory_info(&self) -> Result<NativeMemoryInfo, String> {
        self.memory_info
            .clone()
            .ok_or_else(|| "Memory info not available".to_string())
    }
    pub fn cpu_percent(&self) -> Result<f64, String> {
        Ok(self.cpu_percent)
    }
}

#[cfg(target_os = "linux")]
fn find_immediate_children_linux(parent_pid: i64) -> Vec<i64> {
    let proc_path = format!("/proc/{}/task/{}/children", parent_pid, parent_pid);
    let contents = match std::fs::read_to_string(&proc_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    contents
        .split_whitespace()
        .filter_map(|s| s.parse::<i64>().ok())
        .collect()
}

// Find orphaned children by tracing parent lineage using sysinfo
// This function searches for processes that were recently spawned by a parent PID
// even if the parent has already exited. It uses sysinfo to scan all processes
// and finds those that match timing patterns (created shortly after parent spawn).
#[cfg(target_os = "linux")]
fn find_orphaned_children_by_parent_trace(dead_parent_pid: i64) -> Option<i64> {
    use sysinfo::{ProcessRefreshKind, System, ProcessesToUpdate};
    use std::time::{SystemTime, Duration};
    
    // Refresh all processes using sysinfo
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new(),
    );
    
    // Look for processes that might have been spawned by the dead parent
    // We can't check parent PID directly since parent is dead (children get re-parented to init)
    // Instead, look for recently created processes (within orphan detection window)
    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs();
    let mut candidates = Vec::new();
    
    for (sysinfo_pid, process) in system.processes() {
        let pid = sysinfo_pid.as_u32() as i64;
        
        // Skip the dead parent itself
        if pid == dead_parent_pid {
            continue;
        }
        
        // Check if process was created recently (within orphan detection window)
        // start_time() returns seconds since UNIX epoch
        let process_start = process.start_time();
        if now_secs < process_start {
            // Clock skew detected - process start time is in the future
            log::warn!(
                "Process {} has start time in the future (now={}, start={}), skipping",
                pid, now_secs, process_start
            );
            continue;
        }
        
        let process_age = now_secs - process_start;
            
        if process_age <= ORPHAN_DETECTION_WINDOW_SECS {
            let name = process.name().to_string_lossy().to_string();
            let name_lower = name.to_lowercase();
            let is_shell = SHELL_NAMES.iter().any(|s| name_lower.contains(s));
            
            // Prefer non-shell processes
            if !is_shell {
                log::debug!(
                    "Found potential orphaned child: {} (PID {}, age {}s)",
                    name,
                    pid,
                    process_age
                );
                candidates.push((pid, false)); // false = not a shell
            } else {
                candidates.push((pid, true)); // true = is a shell
            }
        }
    }
    
    // Sort: prefer non-shell processes first
    candidates.sort_by_key(|(_, is_shell)| *is_shell);
    
    // Return the first (best) candidate
    candidates.first().map(|(pid, _)| *pid)
}

// Find the first long-running child (skip shells, find actual service process)
#[cfg(target_os = "linux")]
fn find_first_long_running_child_linux(parent_pid: i64) -> Option<i64> {
    let children = find_immediate_children_linux(parent_pid);
    if children.is_empty() {
        return None;
    }

    // First pass: look for non-shell processes
    for &child in &children {
        if let Ok(exe) = get_process_name(child as u32) {
            let exe_lower = exe.to_lowercase();
            let is_shell = SHELL_NAMES.iter().any(|s| exe_lower.contains(s));

            if !is_shell {
                log::debug!("Found long-running process: {} (PID {})", exe, child);
                return Some(child);
            }
        }
    }

    // Second pass: recursively check shell children
    for &child in &children {
        if let Some(long_running) = find_first_long_running_child_linux(child) {
            return Some(long_running);
        }
    }

    // Fallback: return first child
    children.first().copied()
}

#[cfg(target_os = "linux")]
fn get_process_group_id(pid: i64) -> Option<u32> {
    use std::fs;

    let stat_path = format!("/proc/{}/stat", pid);
    if let Ok(stat_content) = fs::read_to_string(&stat_path) {
        // Parse /proc/pid/stat format: pid (comm) state ppid pgrp ...
        // The process group ID (pgrp) is the 5th field (index 4)
        // We need to handle the comm field which may contain spaces and parentheses
        if let Some(paren_end) = stat_content.rfind(')') {
            let after_comm = &stat_content[paren_end + 1..];
            let parts: Vec<&str> = after_comm.split_whitespace().collect();
            if parts.len() > 4 {
                if let Ok(pgid) = parts[4].parse::<u32>() {
                    return Some(pgid);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn find_alive_process_in_group(pgid: u32, exclude_pid: i64) -> Option<i64> {
    use std::fs;

    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(pid_str) = entry.file_name().into_string() {
                if let Ok(pid) = pid_str.parse::<i64>() {
                    if pid == exclude_pid {
                        continue; // Skip the excluded PID (the shell)
                    }

                    // Check if this process is in the target group
                    if let Some(process_pgid) = get_process_group_id(pid) {
                        if process_pgid == pgid {
                            // Found a process in the group, check if it's alive
                            if crate::process::is_pid_alive(pid) {
                                // Prefer non-shell processes
                                if let Ok(name) = get_process_name(pid as u32) {
                                    let name_lower = name.to_lowercase();
                                    let shell_names = ["sh", "bash", "zsh", "fish", "dash"];
                                    let is_shell =
                                        shell_names.iter().any(|s| name_lower.contains(s));

                                    if !is_shell {
                                        log::debug!(
                                            "Found non-shell process {} (PGID {}) in group",
                                            pid,
                                            pgid
                                        );
                                        return Some(pid);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
pub fn find_alive_process_in_group_for_pid(pid: i64) -> Option<i64> {
    let pgid = get_process_group_id(pid)?;
    find_alive_process_in_group(pgid, pid)
}

#[cfg(target_os = "linux")]
pub fn get_actual_child_pid(shell_pid: i64) -> i64 {
    // FIX #1: ACCURATE PID ADOPTION with Deep Child Search
    // Initial wait of 500ms to allow shell to spawn children
    // This is critical for detecting the actual application PID (e.g., Stirling-PDF issue)
    thread::sleep(Duration::from_millis(500));
    
    log::debug!(
        "Looking for actual child of shell PID {} after 500ms stability wait",
        shell_pid
    );

    // First attempt: look for children while shell is still alive
    if let Some(long_running) = find_first_long_running_child_linux(shell_pid) {
        log::debug!(
            "Found long-running child PID {} for shell PID {} (shell alive)",
            long_running,
            shell_pid
        );
        return long_running;
    }

    // Check if shell exited but left children (common pattern for wrapper scripts)
    if !crate::process::is_pid_alive(shell_pid) {
        log::debug!(
            "Shell PID {} has exited, performing deep child search using sysinfo",
            shell_pid
        );
        
        // Use sysinfo to scan for orphaned children that were spawned by the shell
        // This handles cases where the shell spawns a process and immediately exits
        if let Some(orphaned_child) = find_orphaned_children_by_parent_trace(shell_pid) {
            log::debug!(
                "Found orphaned child PID {} after shell PID {} exited (deep search)",
                orphaned_child,
                shell_pid
            );
            return orphaned_child;
        }
        
        log::debug!(
            "Shell PID {} exited without spawning a detectable child, using shell PID as fallback",
            shell_pid
        );
        return shell_pid; // Return shell PID as fallback (will be handled as crashed by daemon)
    }

    // Fallback: retry with shorter intervals for remaining time
    const ADDITIONAL_RETRIES: u32 = 10;
    const RETRY_DELAY_MS: u64 = 50;
    
    for attempt in 0..ADDITIONAL_RETRIES {
        thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
        
        log::debug!(
            "Looking for actual child of shell PID {} (retry {}/{})",
            shell_pid,
            attempt + 1,
            ADDITIONAL_RETRIES
        );

        if let Some(long_running) = find_first_long_running_child_linux(shell_pid) {
            log::debug!(
                "Found long-running child PID {} for shell PID {} after {} retries",
                long_running,
                shell_pid,
                attempt + 1
            );
            return long_running;
        }

        // Check again if shell exited during retry
        if !crate::process::is_pid_alive(shell_pid) {
            if let Some(orphaned_child) = find_orphaned_children_by_parent_trace(shell_pid) {
                log::debug!(
                    "Found orphaned child PID {} after shell PID {} exited during retry",
                    orphaned_child,
                    shell_pid
                );
                return orphaned_child;
            }
            
            log::debug!(
                "Shell PID {} exited during retry without detectable child",
                shell_pid
            );
            return shell_pid;
        }
    }

    // Ultimate fallback: use shell PID
    log::debug!(
        "No child found after all attempts, using shell PID {} as fallback",
        shell_pid
    );
    shell_pid
}

#[cfg(target_os = "macos")]
fn find_deepest_child_macos(parent_pid: i64) -> Option<i64> {
    use std::collections::HashMap;

    // Build parent-child map from all processes
    let processes = native_processes().ok()?;
    let mut children_map: HashMap<i64, Vec<i64>> = HashMap::new();

    for process in &processes {
        if let Ok(Some(ppid)) = process.ppid() {
            children_map
                .entry(ppid as i64)
                .or_insert_with(Vec::new)
                .push(process.pid() as i64);
        }
    }

    // Recursive helper to find deepest child
    fn find_deepest_recursive(pid: i64, children_map: &HashMap<i64, Vec<i64>>) -> i64 {
        if let Some(children) = children_map.get(&pid) {
            if children.is_empty() {
                return pid;
            }

            if children.len() == 1 {
                log::debug!("Found single child {} of parent {}", children[0], pid);
                return find_deepest_recursive(children[0], children_map);
            }

            // Multiple children: find the deepest among all branches
            log::debug!("Found {} children of parent {}", children.len(), pid);
            let mut deepest_pid = children[0];
            let mut max_depth = 0;

            for &child in children {
                let depth = calculate_depth_macos(child, 0, children_map);
                log::debug!("Child {} has depth {}", child, depth);
                if depth > max_depth {
                    max_depth = depth;
                    deepest_pid = child;
                }
            }

            return find_deepest_recursive(deepest_pid, children_map);
        }

        pid
    }

    let deepest = find_deepest_recursive(parent_pid, &children_map);

    // Only return if we found a different (deeper) PID
    if deepest != parent_pid {
        Some(deepest)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn calculate_depth_macos(
    pid: i64,
    current_depth: usize,
    children_map: &std::collections::HashMap<i64, Vec<i64>>,
) -> usize {
    // Prevent infinite recursion
    if current_depth > 20 {
        return current_depth;
    }

    if let Some(children) = children_map.get(&pid) {
        if children.is_empty() {
            return current_depth;
        }

        // Return depth of deepest child branch
        children
            .iter()
            .map(|&child| calculate_depth_macos(child, current_depth + 1, children_map))
            .max()
            .unwrap_or(current_depth)
    } else {
        current_depth
    }
}

#[cfg(target_os = "macos")]
pub fn get_actual_child_pid(shell_pid: i64) -> i64 {
    // FIX #1: ACCURATE PID ADOPTION with Deep Child Search
    // Initial wait of 500ms to allow shell to spawn children
    thread::sleep(Duration::from_millis(500));

    log::debug!("Looking for actual child of shell PID {} after 500ms stability wait", shell_pid);

    if let Some(deepest) = find_deepest_child_macos(shell_pid) {
        log::debug!(
            "Found deepest child PID {} for shell PID {}",
            deepest,
            shell_pid
        );
        return deepest;
    }

    // Check if shell exited but left children
    if !crate::process::is_pid_alive(shell_pid) {
        log::debug!(
            "Shell PID {} has exited, performing deep child search",
            shell_pid
        );
        
        // For macOS, we can try to find orphaned children
        // This is a best-effort approach since macOS doesn't have /proc
        if let Some(orphaned) = find_orphaned_children_macos(shell_pid) {
            log::debug!(
                "Found orphaned child PID {} after shell PID {} exited",
                orphaned,
                shell_pid
            );
            return orphaned;
        }
    }

    log::debug!("No child found, using shell PID {}", shell_pid);
    shell_pid
}

// Find orphaned children for macOS using process timing
#[cfg(target_os = "macos")]
fn find_orphaned_children_macos(dead_parent_pid: i64) -> Option<i64> {
    use std::time::SystemTime;
    
    let processes = native_processes().ok()?;
    
    let now = SystemTime::now();
    let mut candidates = Vec::new();
    
    for process in &processes {
        let pid = process.pid() as i64;
        
        // Skip the dead parent itself
        if pid == dead_parent_pid {
            continue;
        }
        
        // Check if process was created recently (within orphan detection window)
        let process_age = match now.duration_since(process.create_time) {
            Ok(duration) => duration.as_secs(),
            Err(e) => {
                // Clock skew detected - process start time is in the future
                log::warn!(
                    "Process {} has start time in the future (error: {}), skipping",
                    pid, e
                );
                continue;
            }
        };
            
        if process_age <= ORPHAN_DETECTION_WINDOW_SECS {
            let name = process.name.to_lowercase();
            let is_shell = SHELL_NAMES.iter().any(|s| name.contains(s));
            
            // Prefer non-shell processes
            if !is_shell {
                log::debug!(
                    "Found potential orphaned child: {} (PID {}, age {}s)",
                    process.name,
                    pid,
                    process_age
                );
                candidates.push((pid, false)); // false = not a shell
            } else {
                candidates.push((pid, true)); // true = is a shell
            }
        }
    }
    
    // Sort: prefer non-shell processes first
    candidates.sort_by_key(|(_, is_shell)| *is_shell);
    
    // Return the first (best) candidate
    candidates.first().map(|(pid, _)| *pid)
}
