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
fn find_deepest_child_linux(parent_pid: i64) -> Option<i64> {
    // Read children from /proc
    let proc_path = format!("/proc/{}/task/{}/children", parent_pid, parent_pid);
    let contents = std::fs::read_to_string(&proc_path).ok()?;
    
    let children: Vec<i64> = contents
        .split_whitespace()
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();

    if children.is_empty() {
        return None;
    }

    // If only one child, recurse into it
    if children.len() == 1 {
        let child_pid = children[0];
        log::debug!("Found single child {} of parent {}", child_pid, parent_pid);
        
        // Try to go deeper
        if let Some(deeper) = find_deepest_child_linux(child_pid) {
            return Some(deeper);
        }
        return Some(child_pid);
    }

    // Multiple children: find the deepest among all branches
    log::debug!("Found {} children of parent {}", children.len(), parent_pid);
    let mut deepest_pid = children[0];
    let mut max_depth = 0;

    for &child in &children {
        let depth = calculate_depth_linux(child, 0);
        log::debug!("Child {} has depth {}", child, depth);
        if depth > max_depth {
            max_depth = depth;
            deepest_pid = child;
        }
    }

    // Recurse into the deepest branch
    if let Some(deeper) = find_deepest_child_linux(deepest_pid) {
        return Some(deeper);
    }
    Some(deepest_pid)
}

#[cfg(target_os = "linux")]
fn calculate_depth_linux(pid: i64, current_depth: usize) -> usize {
    // Prevent infinite recursion
    if current_depth > 20 {
        return current_depth;
    }

    let proc_path = format!("/proc/{}/task/{}/children", pid, pid);
    if let Ok(contents) = std::fs::read_to_string(&proc_path) {
        let children: Vec<i64> = contents
            .split_whitespace()
            .filter_map(|s| s.parse::<i64>().ok())
            .collect();

        if children.is_empty() {
            return current_depth;
        }

        // Return depth of deepest child branch
        children
            .iter()
            .map(|&child| calculate_depth_linux(child, current_depth + 1))
            .max()
            .unwrap_or(current_depth)
    } else {
        current_depth
    }
}

#[cfg(target_os = "linux")]
pub fn get_actual_child_pid(shell_pid: i64) -> i64 {
    thread::sleep(Duration::from_millis(PROCESS_OPERATION_DELAY_MS));

    log::debug!("Looking for actual child of shell PID {}", shell_pid);
    
    if let Some(deepest) = find_deepest_child_linux(shell_pid) {
        log::debug!("Found deepest child PID {} for shell PID {}", deepest, shell_pid);
        return deepest;
    }

    log::debug!("No child found, using shell PID {}", shell_pid);
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
    fn find_deepest_recursive(
        pid: i64,
        children_map: &HashMap<i64, Vec<i64>>,
    ) -> i64 {
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
        log::debug!("Found deepest child PID {} for shell PID {}", deepest, shell_pid);
        return deepest;
    }

    log::debug!("No child found, using shell PID {}", shell_pid);
    shell_pid
}
