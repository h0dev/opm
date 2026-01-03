#[cfg(target_os = "linux")]
pub fn get_cpu_percent(pid: u32) -> f64 {
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant};

    // Take two measurements to calculate CPU usage rate
    let get_cpu_time = |pid: u32| -> Option<(f64, f64)> {
        let stat_path = format!("/proc/{}/stat", pid);
        if let Ok(stat_content) = fs::read_to_string(&stat_path) {
            let parts: Vec<&str> = stat_content.split_whitespace().collect();
            if parts.len() > 16 {
                let utime = parts[13].parse::<u64>().ok()? as f64;
                let stime = parts[14].parse::<u64>().ok()? as f64;
                let total_process_time = (utime + stime) / 100.0; // Convert clock ticks to seconds

                // Get system CPU time
                if let Ok(stat_content) = fs::read_to_string("/proc/stat") {
                    if let Some(cpu_line) = stat_content.lines().next() {
                        let cpu_parts: Vec<&str> = cpu_line.split_whitespace().collect();
                        if cpu_parts.len() > 7 {
                            let user: u64 = cpu_parts[1].parse().ok()?;
                            let nice: u64 = cpu_parts[2].parse().ok()?;
                            let system: u64 = cpu_parts[3].parse().ok()?;
                            let idle: u64 = cpu_parts[4].parse().ok()?;
                            let iowait: u64 = cpu_parts[5].parse().ok()?;
                            let irq: u64 = cpu_parts[6].parse().ok()?;
                            let softirq: u64 = cpu_parts[7].parse().ok()?;

                            let total_system_time = (user + nice + system + idle + iowait + irq + softirq) as f64 / 100.0;
                            return Some((total_process_time, total_system_time));
                        }
                    }
                }
            }
        }

        None
    };

    if let Some((start_process, start_system)) = get_cpu_time(pid) {
        let start_time = Instant::now();
        thread::sleep(Duration::from_millis(super::PROCESS_OPERATION_DELAY_MS));

        if let Some((end_process, end_system)) = get_cpu_time(pid) {
            let elapsed = start_time.elapsed().as_secs_f64();
            let process_diff = end_process - start_process;
            let system_diff = end_system - start_system;

            if system_diff > 0.0 && elapsed > 0.0 {
                let cpu_cores = num_cpus::get() as f64;
                let available_cpu_time = elapsed * cpu_cores;
                let cpu_percent = (process_diff / available_cpu_time) * 100.0;
                return cpu_percent.min(100.0);
            }
        }
    }

    0.0
}

/// Get approximate CPU percentage without delay-based sampling
/// This is much faster but less accurate than get_cpu_percent
/// Returns average CPU usage since process start
#[cfg(target_os = "linux")]
pub fn get_cpu_percent_fast(pid: u32) -> f64 {
    use std::fs;
    
    let stat_path = format!("/proc/{}/stat", pid);
    if let Ok(stat_content) = fs::read_to_string(&stat_path) {
        let parts: Vec<&str> = stat_content.split_whitespace().collect();
        // Indices from /proc/[pid]/stat format (see `man 5 proc`):
        // [13] = utime (CPU time in user mode)
        // [14] = stime (CPU time in kernel mode)
        // [21] = starttime (time process started after system boot)
        const UTIME_INDEX: usize = 13;
        const STIME_INDEX: usize = 14;
        const STARTTIME_INDEX: usize = 21;
        
        if parts.len() > STARTTIME_INDEX {
            // Get process CPU time (utime + stime)
            let utime = parts[UTIME_INDEX].parse::<u64>().unwrap_or(0) as f64;
            let stime = parts[STIME_INDEX].parse::<u64>().unwrap_or(0) as f64;
            let starttime = parts[STARTTIME_INDEX].parse::<u64>().unwrap_or(0) as f64;
            
            // Get system uptime
            if let Ok(uptime_content) = fs::read_to_string("/proc/uptime") {
                if let Some(uptime_str) = uptime_content.split_whitespace().next() {
                    if let Ok(uptime) = uptime_str.parse::<f64>() {
                        // CLK_TCK (clock ticks per second) is typically 100 on Linux
                        // but can vary. Common values are 100 (most Linux), 1000 (some systems)
                        // Using sysconf(_SC_CLK_TCK) would be ideal but requires libc calls
                        // For now, use 100 which is standard for most Linux systems
                        const CLOCK_TICKS_PER_SEC: f64 = 100.0;
                        
                        // Calculate process uptime in seconds
                        let process_uptime = uptime - (starttime / CLOCK_TICKS_PER_SEC);
                        
                        if process_uptime > 0.0 {
                            // Total CPU time used by process in seconds
                            let process_cpu_time = (utime + stime) / CLOCK_TICKS_PER_SEC;
                            
                            // CPU percentage = (CPU time / elapsed time) * 100
                            let cpu_percent = (process_cpu_time / process_uptime) * 100.0;
                            
                            // Clamp to reasonable value
                            return cpu_percent.min(100.0 * num_cpus::get() as f64);
                        }
                    }
                }
            }
        }
    }
    
    0.0
}

#[cfg(target_os = "macos")]
pub fn get_cpu_percent_fast(pid: u32) -> f64 {
    // For macOS, we'll use ps command as a fast approximation
    if let Some(percent) = get_cpu_percent_ps(pid) {
        return percent;
    }
    0.0
}

#[cfg(target_os = "macos")]
pub fn get_cpu_percent(pid: u32) -> f64 {
    // Try mach task info first
    if let Some(percent) = get_cpu_percent_mach(pid) {
        return percent;
    }

    // Fallback to ps command
    if let Some(percent) = get_cpu_percent_ps(pid) {
        return percent;
    }

    0.0
}

#[cfg(target_os = "macos")]
fn get_cpu_percent_mach(pid: u32) -> Option<f64> {
    use std::mem;
    use std::thread;
    use std::time::{Duration, Instant};

    #[repr(C)]
    struct TaskBasicInfo {
        virtual_size: u32,
        resident_size: u32,
        resident_size_max: u32,
        user_time: TimeValue,
        system_time: TimeValue,
        policy: i32,
        suspend_count: i32,
    }

    #[repr(C)]
    struct TimeValue {
        seconds: i32,
        microseconds: i32,
    }

    const TASK_BASIC_INFO: u32 = 5;
    const TASK_BASIC_INFO_COUNT: u32 = 10;

    unsafe extern "C" {
        fn task_for_pid(target_tport: u32, pid: i32, task: *mut u32) -> i32;
        fn task_info(target_task: u32, flavor: u32, task_info_out: *mut libc::c_void, task_info_outCnt: *mut u32) -> i32;
        fn mach_task_self() -> u32;
    }

    // Helper to convert TimeValue to seconds
    let time_to_seconds = |tv: &TimeValue| -> f64 {
        tv.seconds as f64 + tv.microseconds as f64 / 1_000_000.0
    };

    // Get task port for the process
    let mut task: u32 = 0;
    if unsafe { task_for_pid(mach_task_self(), pid as i32, &mut task) } != 0 {
        return None;
    }

    // Get first measurement
    let mut info: TaskBasicInfo = unsafe { mem::zeroed() };
    let mut count = TASK_BASIC_INFO_COUNT;
    if unsafe { task_info(task, TASK_BASIC_INFO, &mut info as *mut _ as *mut libc::c_void, &mut count) } != 0 {
        return None;
    }

    let start_time = Instant::now();
    let start_cpu_time = time_to_seconds(&info.user_time) + time_to_seconds(&info.system_time);

    // Wait for measurement interval
    thread::sleep(Duration::from_millis(super::PROCESS_OPERATION_DELAY_MS));

    // Get second measurement
    let mut info2: TaskBasicInfo = unsafe { mem::zeroed() };
    let mut count2 = TASK_BASIC_INFO_COUNT;
    if unsafe { task_info(task, TASK_BASIC_INFO, &mut info2 as *mut _ as *mut libc::c_void, &mut count2) } != 0 {
        return None;
    }

    let elapsed_real = start_time.elapsed().as_secs_f64();
    if elapsed_real <= 0.0 {
        return None;
    }

    let end_cpu_time = time_to_seconds(&info2.user_time) + time_to_seconds(&info2.system_time);
    let cpu_time_used = end_cpu_time - start_cpu_time;

    let cpu_cores = num_cpus::get() as f64;
    let available_cpu_time = elapsed_real * cpu_cores;
    let cpu_percent = (cpu_time_used / available_cpu_time) * 100.0 * cpu_cores;

    Some(cpu_percent.min(100.0))
}

#[cfg(target_os = "macos")]
fn get_cpu_percent_ps(pid: u32) -> Option<f64> {
    let output = std::process::Command::new("ps")
        .args(&["-p", &pid.to_string(), "-o", "pcpu="])
        .output()
        .ok()?;

    let cpu_str = String::from_utf8(output.stdout).ok()?;
    cpu_str.trim().parse::<f64>().ok()
} 