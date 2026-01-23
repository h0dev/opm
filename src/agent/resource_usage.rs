use crate::agent::types::ResourceUsage;

/// Gather current resource usage metrics
pub fn gather_resource_usage() -> Option<ResourceUsage> {
    let mem_info = sys_info::mem_info().ok()?;
    let disk_info = sys_info::disk_info().ok();
    let loadavg = sys_info::loadavg().ok();
    
    // Calculate memory usage - show as usage percentage
    let memory_used = mem_info.total.saturating_sub(mem_info.avail);
    let memory_percent = if mem_info.total > 0 {
        (memory_used as f64 / mem_info.total as f64) * 100.0
    } else {
        0.0
    };
    
    // Calculate disk usage - show as usage percentage
    let (disk_total, disk_free, disk_percent) = if let Some(disk) = disk_info {
        let total = disk.total;
        let free = disk.free;
        let percent = if total > 0 {
            ((total - free) as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        (Some(total), Some(free), Some(percent))
    } else {
        (None, None, None)
    };
    
    // CPU usage based on load average normalized by CPU count
    let cpu_count = num_cpus::get() as f64;
    let cpu_usage = loadavg.as_ref().map(|la| {
        (la.one / cpu_count) * 100.0
    });
    
    Some(ResourceUsage {
        cpu_usage,
        memory_used: Some(memory_used),
        memory_available: Some(mem_info.avail),
        memory_percent: Some(memory_percent),
        disk_total,
        disk_free,
        disk_percent,
        load_avg_1: loadavg.as_ref().map(|la| la.one),
        load_avg_5: loadavg.as_ref().map(|la| la.five),
        load_avg_15: loadavg.as_ref().map(|la| la.fifteen),
    })
}
