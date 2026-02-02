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
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;

use crate::process::{dump, Runner};

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
            // where the daemon's stale runner overwrites newly created processes.
            //
            // CRITICAL: Hold the memory cache lock for the entire read-merge-write sequence
            // to prevent concurrent SetState requests from overwriting each other's changes.
            // Without this lock, when multiple CLI commands run simultaneously:
            //   Thread A: read {p0} → merge {p0, p1} → write {p0, p1}
            //   Thread B: read {p0} → merge {p0, p2} → write {p0, p2}  ← OVERWRITES A's p1!
            // With the lock, operations are serialized:
            //   Thread A: LOCK → read {p0} → merge {p0, p1} → write {p0, p1} → UNLOCK
            //   Thread B: LOCK → read {p0, p1} → merge {p0, p1, p2} → write {p0, p1, p2} → UNLOCK
            
            // Acquire exclusive lock on memory cache for atomic read-merge-write
            use crate::process::dump::MEMORY_CACHE;
            let mut cache = MEMORY_CACHE.lock().unwrap();
            
            // Read current memory state while holding lock
            let current_memory = cache.clone();
            
            let merged_runner = match current_memory {
                Some(mut current) => {
                    // Merge strategy: Update/add all processes from the provided runner
                    // while preserving any processes in memory that aren't in the provided runner.
                    // 
                    // CRITICAL: Handle ID collisions when multiple CLIs create processes concurrently.
                    // When concurrent CLIs all start with empty state (counter=0), they all assign ID 0
                    // to their processes. We need to reassign IDs to prevent overwrites.
                    //
                    // Strategy:
                    // 1. For each process in the provided runner:
                    //    - If the ID already exists in current memory AND it's a different process (different PID),
                    //      assign a new unique ID
                    //    - Otherwise, use the provided ID (update existing or add new)
                    // 2. Update the ID counter to ensure future IDs don't collide
                    
                    let mut max_id = current.list.keys().max().copied().unwrap_or(0);
                    
                    // Update/add all processes from the provided runner
                    for (id, mut process) in runner.list {
                        // Check if this ID already exists with a different process
                        let needs_new_id = if let Some(existing) = current.list.get(&id) {
                            // ID exists - check if it's the same process (same PID) or a collision
                            existing.pid != process.pid
                        } else {
                            // ID doesn't exist - no collision
                            false
                        };
                        
                        let final_id = if needs_new_id {
                            // ID collision detected - assign a new unique ID
                            max_id += 1;
                            process.id = max_id;  // Update the process's own ID field
                            max_id
                        } else {
                            // No collision - use the provided ID
                            id
                        };
                        
                        current.list.insert(final_id, process);
                    }
                    
                    // Update the ID counter to max + 1 to prevent future collisions
                    let new_counter = current.list.keys().max().map(|&k| k + 1).unwrap_or(0);
                    current.id.counter.store(new_counter, std::sync::atomic::Ordering::Relaxed);
                    
                    current
                }
                None => {
                    // No existing state, use the provided runner as-is
                    runner
                }
            };
            
            // Write merged state to memory cache while still holding lock
            *cache = Some(merged_runner);
            // Lock is automatically released when `cache` goes out of scope
            
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
                
                // IMPORTANT: Mark process as stopped BEFORE removing from list
                // This prevents race condition where daemon's restart_process() loop
                // detects the process is dead and tries to restart it during removal
                // Safe to call process(id) because we're inside runner.exists(id) check
                runner.process(id).running = false;
                
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
                
                // Mark as stopped
                runner.process(id).running = false;
                
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
