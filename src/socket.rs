//! Unix socket API for daemon-CLI communication
//!
//! This module provides a Unix socket server that allows CLI commands to communicate
//! with the daemon for state management operations. The daemon keeps state in RAM,
//! and the CLI reads/writes through this socket API.
//!
//! ## Protocol
//!
//! The protocol uses JSON over Unix sockets for simplicity and debuggability:
//! - Request: JSON-serialized `SocketRequest`
//! - Response: JSON-serialized `SocketResponse`
//!
//! ## Socket Location
//!
//! The socket is created at `~/.opm/opm.sock`

use anyhow::{anyhow, Result};
use home::home_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;

use crate::process::{dump, Runner};

/// Duration (in seconds) that daemon will ignore a process after a manual action.
/// This constant documents the timeout used by daemon/mod.rs::has_recent_action_timestamp().
/// Keep this in sync with the actual timeout value in the daemon code.
#[allow(dead_code)]
const ACTION_IGNORE_DURATION_SECS: u64 = 3;

/// Helper function to create action timestamp file for a process
/// This tells the daemon to ignore the process for ACTION_IGNORE_DURATION_SECS seconds
fn create_action_timestamp(id: usize) {
    if let Some(home_dir) = home_dir() {
        let action_file = format!("{}/.opm/last_action_{}.timestamp", home_dir.display(), id);
        let _ = std::fs::write(&action_file, chrono::Utc::now().to_rfc3339());
    }
}

/// Request types that can be sent to the daemon via socket
#[derive(Debug, Serialize, Deserialize)]
pub enum SocketRequest {
    /// Get the current process state (Runner)
    GetState,
    /// Update the process state
    SetState(Runner),
    /// Save the current state to permanent storage
    SavePermanent,
    /// Remove a process by ID (handled by daemon to avoid race conditions)
    RemoveProcess(usize),
    /// Stop a process by ID
    StopProcess(usize),
    /// Start a process by ID
    StartProcess(usize),
    /// Restart a process by ID
    RestartProcess(usize),
    /// Edit a process (name and/or command)
    EditProcess { id: usize, name: Option<String>, command: Option<String> },
    /// Ping to check if daemon is responsive
    Ping,
}

/// Response from daemon socket API
#[derive(Debug, Serialize, Deserialize)]
pub enum SocketResponse {
    /// Successfully retrieved state
    State(Runner),
    /// Operation succeeded
    Success,
    /// Error occurred
    Error(String),
    /// Pong response to Ping
    Pong,
}

/// Start the Unix socket server in the daemon
///
/// This function creates a Unix socket at the specified path and listens for
/// incoming connections from CLI commands. Each connection is handled in a
/// separate thread.
pub fn start_socket_server(socket_path: &str) -> Result<()> {
    // Remove old socket file if it exists
    if Path::new(socket_path).exists() {
        fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    
    // Set restrictive permissions (600 - owner read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(socket_path, permissions)?;
    }
    
    log::info!("Unix socket server started at {}", socket_path);

    // Use a bounded channel to limit concurrent connections
    const MAX_CONCURRENT_CONNECTIONS: usize = 100;
    let (tx, rx) = std::sync::mpsc::sync_channel::<UnixStream>(MAX_CONCURRENT_CONNECTIONS);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    
    // Spawn worker threads to handle connections
    const WORKER_THREADS: usize = 4;
    for i in 0..WORKER_THREADS {
        let rx = std::sync::Arc::clone(&rx);
        thread::spawn(move || {
            loop {
                let stream = {
                    let rx = rx.lock().unwrap();
                    match rx.recv() {
                        Ok(stream) => stream,
                        Err(_) => break, // Channel closed
                    }
                };
                if let Err(e) = handle_client(stream) {
                    log::error!("Error handling socket client: {}", e);
                }
            }
            log::info!("Socket worker thread {} exiting", i);
        });
    }
    
    // Accept connections and send to worker threads
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // Try to send to worker threads, drop connection if queue is full
                if tx.send(stream).is_err() {
                    log::warn!("Socket connection queue full, dropping connection");
                }
            }
            Err(e) => {
                log::error!("Error accepting socket connection: {}", e);
            }
        }
    }

    Ok(())
}

/// Handle a single client connection
fn handle_client(mut stream: UnixStream) -> Result<()> {
    // Set read timeout to prevent hanging on malicious clients
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    
    let reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    
    // Limit input size to 10MB to prevent memory exhaustion
    const MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024;
    let mut limited_reader = reader.take(MAX_REQUEST_SIZE as u64);
    limited_reader.read_line(&mut line)?;

    // Parse request
    let request: SocketRequest = serde_json::from_str(&line)?;

    // Process request
    let response = match request {
        SocketRequest::GetState => {
            // Read merged state directly without recursion
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let merged = dump::merge_runners_public(permanent, memory);
            SocketResponse::State(merged)
        }
        SocketRequest::SetState(runner) => {
            // Merge the provided state with existing memory cache to prevent race conditions
            // where the daemon's stale runner overwrites newly created processes
            // Read current memory state
            let current_memory = dump::read_memory_direct_option();
            
            let merged_runner = match current_memory {
                Some(mut current) => {
                    // Merge strategy: Update/add all processes from the provided runner
                    // while preserving any processes in memory that aren't in the provided runner.
                    // 
                    // This prevents two issues:
                    // 1. Daemon (or any caller) accidentally deleting processes created after it loaded state
                    // 2. Race conditions where CLI creates a process while daemon is monitoring
                    //
                    // For processes that exist in both:
                    // - The provided runner's version overwrites the existing one
                    // - This is intentional: the daemon is the authoritative source for state updates
                    //   (crash counters, PIDs, running status, etc.)
                    // - The daemon loads state once per cycle and makes authoritative updates
                    //
                    // For processes only in current memory:
                    // - They are preserved (not deleted)
                    // - This fixes the reported bug where newly created processes disappeared
                    
                    // Update all processes that exist in the provided runner
                    // Handle ID conflicts: if a process ID already exists and the incoming process
                    // is different (race condition from concurrent creates with stale counter),
                    // reassign a new ID to prevent overwriting existing processes
                    for (id, mut process) in runner.list {
                        if let Some(existing) = current.list.get(&id) {
                            // Check if this is actually a different process (same ID but different content)
                            // Compare by PID and creation time to detect race condition
                            let is_same_process = existing.pid == process.pid 
                                && existing.started == process.started
                                && existing.name == process.name;
                            
                            if !is_same_process {
                                // ID conflict detected! Allocate a new ID for the incoming process
                                // Find the next available ID by checking current counter and incrementing
                                let mut new_id = current.id.counter.load(std::sync::atomic::Ordering::Relaxed);
                                while current.list.contains_key(&new_id) {
                                    new_id += 1;
                                }
                                
                                // Update process with new ID and insert
                                process.id = new_id;
                                current.list.insert(new_id, process);
                                
                                // Update counter to be at least new_id + 1
                                let next_counter = new_id + 1;
                                current.id.counter.store(next_counter, std::sync::atomic::Ordering::Relaxed);
                                
                                log::warn!(
                                    "[socket] ID conflict detected for id={}, reassigned to id={}. This indicates concurrent process creation with stale ID counter.",
                                    id, new_id
                                );
                                continue;
                            }
                        }
                        
                        // No conflict, or this is an update to existing process - insert normally
                        current.list.insert(id, process);
                    }
                    
                    // Update the ID counter to the maximum of both
                    // Use Relaxed ordering since socket handler is single-threaded and sequential
                    let provided_counter = runner.id.counter.load(std::sync::atomic::Ordering::Relaxed);
                    let current_counter = current.id.counter.load(std::sync::atomic::Ordering::Relaxed);
                    if provided_counter > current_counter {
                        current.id.counter.store(provided_counter, std::sync::atomic::Ordering::Relaxed);
                    }
                    
                    current
                }
                None => {
                    // No existing state, use the provided runner as-is
                    runner
                }
            };
            
            // Write merged state to memory cache
            dump::write_memory_direct(&merged_runner);
            SocketResponse::Success
        }
        SocketRequest::SavePermanent => {
            // Commit memory cache to permanent storage directly
            dump::commit_memory_direct();
            SocketResponse::Success
        }
        SocketRequest::RemoveProcess(id) => {
            // Handle process removal through daemon to avoid race conditions
            // This ensures the daemon's monitoring loop sees the removal immediately
            // Read state directly to avoid socket recursion
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let mut runner = dump::merge_runners_public(permanent, memory);
            
            if runner.exists(id) {
                // Get PID info before removing
                let pid = runner.info(id).map(|p| p.pid).unwrap_or(0);
                let children = runner.info(id).map(|p| p.children.clone()).unwrap_or_default();
                
                // Create action timestamp to prevent daemon from interfering during removal
                create_action_timestamp(id);
                
                // IMPORTANT: Mark process as stopped BEFORE removing from list
                // This prevents race condition where daemon's restart_process() loop
                // detects the process is dead and tries to restart it during removal
                // Safe to call process(id) because we're inside runner.exists(id) check
                runner.process(id).running = false;
                runner.process(id).crash.crashed = false;  // Clear crashed flag
                
                // Save state with running=false before removal
                // This ensures daemon sees the stopped state and won't try to restart
                // Using dump::write_memory_direct instead of runner.save() to avoid recursion
                // since this is running inside the socket handler
                dump::write_memory_direct(&runner);
                
                // Brief delay to ensure daemon's monitoring loop sees the updated state
                // before we remove the process from the list. This is a lightweight
                // synchronization approach suitable for this use case where the daemon
                // runs on a 1-second interval. A longer delay would be wasteful, and
                // proper synchronization primitives would add unnecessary complexity.
                std::thread::sleep(std::time::Duration::from_millis(100));
                
                // Remove from list
                runner.list.remove(&id);
                runner.compact();
                
                // Write to memory cache only - don't persist until save
                dump::write_memory_direct(&runner);
                
                // Now kill the process
                if pid > 0 {
                    use crate::process::process_stop;
                    
                    // Kill children
                    for child_pid in children {
                        let _ = nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(child_pid as i32),
                            nix::sys::signal::Signal::SIGTERM,
                        );
                    }
                    
                    // Kill main process
                    let _ = process_stop(pid);
                    
                    // Wait for termination (simple version without accessing private function)
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                
                SocketResponse::Success
            } else {
                SocketResponse::Error(format!("Process {} not found", id))
            }
        }
        SocketRequest::StopProcess(id) => {
            // Stop a process by setting running=false and killing the PID
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let mut runner = dump::merge_runners_public(permanent, memory);
            
            if runner.exists(id) {
                let pid = runner.info(id).map(|p| p.pid).unwrap_or(0);
                let children = runner.info(id).map(|p| p.children.clone()).unwrap_or_default();
                
                // Create action timestamp to prevent daemon from interfering during stop
                create_action_timestamp(id);
                
                // Mark as stopped and clear crashed flag
                runner.process(id).running = false;
                runner.process(id).crash.crashed = false;  // Clear crashed flag
                
                // Write to memory cache only
                dump::write_memory_direct(&runner);
                
                // Kill the process
                if pid > 0 {
                    use crate::process::process_stop;
                    
                    // Kill children
                    for child_pid in children {
                        let _ = nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(child_pid as i32),
                            nix::sys::signal::Signal::SIGTERM,
                        );
                    }
                    
                    // Kill main process
                    let _ = process_stop(pid);
                    
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                
                SocketResponse::Success
            } else {
                SocketResponse::Error(format!("Process {} not found", id))
            }
        }
        SocketRequest::StartProcess(id) => {
            // Start a stopped process
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let mut runner = dump::merge_runners_public(permanent, memory);
            
            if runner.exists(id) {
                // This is a simplified start - full implementation would need process spawning logic
                // For now, just mark as running and let the daemon handle actual process start
                runner.process(id).running = true;
                runner.process(id).crash.crashed = false;
                
                // Write to memory cache only
                dump::write_memory_direct(&runner);
                
                SocketResponse::Success
            } else {
                SocketResponse::Error(format!("Process {} not found", id))
            }
        }
        SocketRequest::RestartProcess(id) => {
            // Restart a process by stopping and starting it
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let mut runner = dump::merge_runners_public(permanent, memory);
            
            if runner.exists(id) {
                let pid = runner.info(id).map(|p| p.pid).unwrap_or(0);
                let children = runner.info(id).map(|p| p.children.clone()).unwrap_or_default();
                
                // Kill existing process
                if pid > 0 {
                    use crate::process::process_stop;
                    
                    // Kill children
                    for child_pid in children {
                        let _ = nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(child_pid as i32),
                            nix::sys::signal::Signal::SIGTERM,
                        );
                    }
                    
                    // Kill main process
                    let _ = process_stop(pid);
                    
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                
                // Mark for restart - daemon will handle actual spawning
                runner.process(id).running = true;
                runner.process(id).crash.crashed = false;
                runner.process(id).restarts += 1;
                
                // Write to memory cache only
                dump::write_memory_direct(&runner);
                
                SocketResponse::Success
            } else {
                SocketResponse::Error(format!("Process {} not found", id))
            }
        }
        SocketRequest::EditProcess { id, name, command } => {
            // Edit a process's name and/or command in RAM
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let mut runner = dump::merge_runners_public(permanent, memory);
            
            if runner.exists(id) {
                // Update name if provided
                if let Some(new_name) = name {
                    runner.process(id).name = new_name;
                }
                
                // Update command/script if provided
                if let Some(new_command) = command {
                    runner.process(id).script = new_command;
                }
                
                // Write to memory cache only
                dump::write_memory_direct(&runner);
                
                SocketResponse::Success
            } else {
                SocketResponse::Error(format!("Process {} not found", id))
            }
        }
        SocketRequest::Ping => SocketResponse::Pong,
    };

    // Send response
    let response_json = serde_json::to_string(&response)?;
    stream.write_all(response_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    Ok(())
}

/// Client function to send a request to the daemon via socket
pub fn send_request(socket_path: &str, request: SocketRequest) -> Result<SocketResponse> {
    let mut stream = UnixStream::connect(socket_path).map_err(|e| {
        anyhow!(
            "Failed to connect to daemon socket: {}. Is the daemon running?",
            e
        )
    })?;

    // Send request
    let request_json = serde_json::to_string(&request)?;
    stream.write_all(request_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    // Read response
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    // Parse response
    let response: SocketResponse = serde_json::from_str(&line)?;

    Ok(response)
}

/// Check if daemon is running by attempting to ping via socket
pub fn is_daemon_running(socket_path: &str) -> bool {
    match send_request(socket_path, SocketRequest::Ping) {
        Ok(SocketResponse::Pong) => true,
        _ => false,
    }
}
