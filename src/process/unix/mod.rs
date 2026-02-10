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
};
pub use process_list::native_processes;

pub const PROCESS_OPERATION_DELAY_MS: u64 = 500;

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

// Find the first long-running child (skip shells, find actual service process)
#[cfg(target_os = "linux")]
fn find_first_long_running_child_linux(parent_pid: i64) -> Option<i64> {
    let shell_names = ["sh", "bash", "zsh", "fish", "dash"];

    let children = find_immediate_children_linux(parent_pid);
    if children.is_empty() {
        return None;
    }

    // First pass: look for non-shell processes
    for &child in &children {
        if let Ok(exe) = get_process_name(child as u32) {
            let exe_lower = exe.to_lowercase();
            let is_shell = shell_names.iter().any(|s| exe_lower.contains(s));

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
    // Store the shell's process group ID before it exits
    // This is critical for backgrounded processes where the shell exits immediately
    let shell_pgid = get_process_group_id(shell_pid);

    // Retry logic with shorter intervals and immediate first check
    // Poll more frequently to catch the child before the shell exits
    const MAX_RETRIES: u32 = 20; // Increased from 6
    const RETRY_DELAY_MS: u64 = 50; // Reduced from 500ms to 50ms

    // Immediate first check (no sleep) to catch fast-spawning children
    log::debug!(
        "Looking for actual child of shell PID {} (immediate check)",
        shell_pid
    );

    for attempt in 0..MAX_RETRIES {
        // Only sleep after the first attempt
        if attempt > 0 {
            thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
        }

        log::debug!(
            "Looking for actual child of shell PID {} (attempt {}/{})",
            shell_pid,
            attempt + 1,
            MAX_RETRIES
        );

        if let Some(long_running) = find_first_long_running_child_linux(shell_pid) {
            log::debug!(
                "Found long-running child PID {} for shell PID {} after {} attempts",
                long_running,
                shell_pid,
                attempt + 1
            );
            return long_running;
        }

        // Check if shell is still alive
        if !crate::process::is_pid_alive(shell_pid) {
            // Shell has exited - try process group fallback
            if let Some(pgid) = shell_pgid {
                log::debug!(
                    "Shell PID {} exited, attempting process group fallback (PGID {})",
                    shell_pid,
                    pgid
                );

                // Try to find an alive non-shell process in the same process group
                if let Some(group_child) = find_alive_process_in_group(pgid, shell_pid) {
                    log::debug!(
                        "Found process group fallback PID {} for shell PID {}",
                        group_child,
                        shell_pid
                    );
                    return group_child;
                }
            }

            log::debug!(
                "Shell PID {} exited and no fallback found, using shell PID as fallback",
                shell_pid
            );
            return shell_pid; // Return shell PID as fallback (will be handled by daemon adoption logic)
        }

        // If this is not the last attempt, continue retrying
        if attempt < MAX_RETRIES - 1 {
            log::debug!("No child found yet, retrying in {}ms...", RETRY_DELAY_MS);
        }
    }

    // After all retries, if still no child found, try process group fallback as last resort
    if let Some(pgid) = shell_pgid {
        if let Some(group_child) = find_alive_process_in_group(pgid, shell_pid) {
            log::debug!(
                "Final process group fallback found PID {} for shell PID {}",
                group_child,
                shell_pid
            );
            return group_child;
        }
    }

    // Ultimate fallback: use shell PID
    log::debug!(
        "No child found after {} attempts, using shell PID {} as fallback",
        MAX_RETRIES,
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
    // Wait for shell to spawn the actual command
    thread::sleep(Duration::from_millis(PROCESS_OPERATION_DELAY_MS));

    log::debug!("Looking for actual child of shell PID {}", shell_pid);

    if let Some(deepest) = find_deepest_child_macos(shell_pid) {
        log::debug!(
            "Found deepest child PID {} for shell PID {}",
            deepest,
            shell_pid
        );
        return deepest;
    }

    log::debug!("No child found, using shell PID {}", shell_pid);
    shell_pid
}
