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
use crate::process;

/// Duration (in seconds) that daemon will ignore a process after a manual action.
/// This constant documents the timeout used by daemon/mod.rs::has_recent_action_timestamp().
/// Keep this in sync with the actual timeout value in the daemon code.
#[allow(dead_code)]
const ACTION_IGNORE_DURATION_SECS: u64 = 5;

/// Helper function to create action timestamp file for a process
/// This tells the daemon to ignore the process for ACTION_IGNORE_DURATION_SECS seconds
/// Uses fsync to ensure timestamp is durably written before returning
fn create_action_timestamp(id: usize) {
    use crate::process::write_action_timestamp;
    if let Err(e) = write_action_timestamp(id) {
        log::warn!("Failed to create action timestamp file for process {}: {}", id, e);
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
///
/// Takes an optional callback that is invoked once the server is fully ready to accept connections.
pub fn start_socket_server(socket_path: &str) -> Result<()> {
    start_socket_server_with_callback::<fn()>(socket_path, None)
}

/// Start the Unix socket server with an optional readiness callback
///
/// The callback is invoked after the socket is bound and worker threads are spawned,
/// signaling that the server is ready to accept connections.
pub fn start_socket_server_with_callback<F>(socket_path: &str, ready_callback: Option<F>) -> Result<()>
where
    F: FnOnce() + Send + 'static,
{
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
                    // Log with more context to help debugging
                    log::error!("Error handling socket client in worker thread {}: {}", i, e);
                    // Also log the error chain for more detailed debugging
                    if let Some(source) = e.source() {
                        log::debug!("Socket error source: {}", source);
                    }
                }
            }
            log::info!("Socket worker thread {} exiting", i);
        });
    }
    
    // Invoke readiness callback after socket is bound and workers are spawned
    // This signals that the server is ready to accept connections
    if let Some(callback) = ready_callback {
        callback();
    }
    
    // Accept connections and send to worker threads
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // Try to send to worker threads, drop connection if queue is full
                if let Err(e) = tx.send(stream) {
                    log::warn!(
                        "Socket connection queue full (max: {}), dropping connection: {}. \
                        Consider increasing MAX_CONCURRENT_CONNECTIONS or WORKER_THREADS if this happens frequently.",
                        MAX_CONCURRENT_CONNECTIONS, e
                    );
                }
            }
            Err(e) => {
                log::error!(
                    "Error accepting socket connection: {}. This may indicate a system resource issue.",
                    e
                );
            }
        }
    }

    Ok(())
}

/// Handle a single client connection
fn handle_client(mut stream: UnixStream) -> Result<()> {
    // Set read timeout to prevent hanging on malicious clients
    // Increased from 5s to 30s to allow for large state transfers
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    
    // Set write timeout to prevent hanging on socket writes
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    
    // Read request from stream
    // IMPORTANT: We create the BufReader in a limited scope so that it gets dropped
    // before we write to the stream. This prevents buffering synchronization issues.
    // The BufReader borrows the stream mutably, but when the scope ends (line 200),
    // the BufReader is dropped and the borrow ends, allowing us to use stream for writing.
    let request = {
        let mut line = String::new();
        
        // Limit input size to 50MB to allow for larger state objects while preventing memory exhaustion
        // Increased from 10MB to accommodate larger process lists
        const MAX_REQUEST_SIZE: usize = 50 * 1024 * 1024;
        let reader = BufReader::new(&mut stream);
        let mut limited_reader = reader.take(MAX_REQUEST_SIZE as u64);
        
        // Read the request line
        match limited_reader.read_line(&mut line) {
            Ok(0) => {
                // Connection closed before receiving data
                return Err(anyhow!("Client closed connection before sending request"));
            }
            Ok(bytes_read) => {
                log::debug!("[socket] Read {} bytes from client", bytes_read);
            }
            Err(e) => {
                log::error!("[socket] Failed to read from client: {}", e);
                return Err(anyhow!("Failed to read request: {}", e));
            }
        }
        
        // Parse request before reader is dropped
        serde_json::from_str::<SocketRequest>(&line).map_err(|e| {
            log::error!("[socket] Failed to parse request (len={}): {}", line.len(), e);
            anyhow!("Invalid request format: {}", e)
        })?
    }; // BufReader is dropped here, releasing the mutable borrow on stream

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
                    // is fundamentally different (race condition from concurrent creates with stale counter),
                    // reassign a new ID to prevent overwriting existing processes
                    for (id, mut process) in runner.list {
                        if let Some(existing) = current.list.get(&id) {
                            // Check if this is actually a different process (same ID but different identity)
                            // Compare by immutable process properties (name, script, path) rather than
                            // runtime state (PID, started timestamp) which change during normal restarts.
                            // This fixes the bug where process restarts were incorrectly detected as ID conflicts,
                            // causing duplicate process entries to be created.
                            let is_same_process = existing.name == process.name
                                && existing.script == process.script
                                && existing.path == process.path;
                            
                            if !is_same_process {
                                // True ID conflict detected! Two genuinely different processes claim the same ID.
                                // This happens in race conditions during concurrent process creation with stale counters.
                                // Allocate a new ID for the incoming process to prevent overwriting.
                                let mut new_id = current.id.counter.load(std::sync::atomic::Ordering::Relaxed);
                                while current.list.contains_key(&new_id) {
                                    new_id += 1;
                                }
                                
                                // Log before moving process (avoids unnecessary clones)
                                log::warn!(
                                    "[socket] True ID conflict detected for id={} (existing process '{}' vs incoming process '{}'). Reassigned incoming to id={}.",
                                    id, existing.name, process.name, new_id
                                );
                                
                                // Update process with new ID and insert
                                process.id = new_id;
                                current.list.insert(new_id, process);
                                
                                // Update counter to be at least new_id + 1
                                let next_counter = new_id + 1;
                                current.id.counter.store(next_counter, std::sync::atomic::Ordering::Relaxed);
                                
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
                // Mark as running - daemon will spawn the process
                runner.process(id).running = true;
                runner.process(id).crash.crashed = false;
                runner.process(id).pid = 0; // Reset PID so daemon knows to spawn
                runner.process(id).shell_pid = None;
                runner.process(id).started = chrono::Utc::now();
                
                // Write to memory cache only
                dump::write_memory_direct(&runner);
                
                SocketResponse::Success
            } else {
                SocketResponse::Error(format!("Process {} not found", id))
            }
        }
        SocketRequest::RestartProcess(id) => {
            // Restart a process by stopping the old one and marking for daemon to start new one
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let mut runner = dump::merge_runners_public(permanent, memory);
            
            if runner.exists(id) {
                let pid = runner.info(id).map(|p| p.pid).unwrap_or(0);
                let shell_pid = runner.info(id).and_then(|p| p.shell_pid);
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
                    
                    // Remove process handle if it exists
                    let handle_pid = shell_pid.unwrap_or(pid);
                    if let Some((_, handle)) = process::PROCESS_HANDLES.remove(&handle_pid) {
                        if let Ok(mut child) = handle.lock() {
                            let _ = child.wait();
                        }
                    }
                    
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                
                // Mark for restart - daemon will spawn the process
                runner.process(id).running = true;
                runner.process(id).crash.crashed = false;
                runner.process(id).restarts += 1;
                runner.process(id).pid = 0; // Reset PID so daemon knows to spawn
                runner.process(id).shell_pid = None;
                runner.process(id).children = vec![];
                runner.process(id).started = chrono::Utc::now();
                
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
    let response_json = serde_json::to_string(&response).map_err(|e| {
        log::error!("[socket] Failed to serialize response: {}", e);
        anyhow!("Failed to serialize response: {}", e)
    })?;
    
    let response_len = response_json.len();
    log::debug!("[socket] Sending response ({} bytes)", response_len);
    
    // Write response with error handling
    if let Err(e) = stream.write_all(response_json.as_bytes()) {
        log::error!("[socket] Failed to write response body: {}", e);
        return Err(anyhow!("Failed to write response: {}", e));
    }
    
    if let Err(e) = stream.write_all(b"\n") {
        log::error!("[socket] Failed to write response newline: {}", e);
        return Err(anyhow!("Failed to write newline: {}", e));
    }
    
    if let Err(e) = stream.flush() {
        log::error!("[socket] Failed to flush response: {}", e);
        return Err(anyhow!("Failed to flush: {}", e));
    }
    
    // Shutdown write side to signal completion
    // This ensures the client knows the response is complete
    // Expected errors: ENOTCONN (client closed connection early), EPIPE (broken pipe)
    // These are normal when a client disconnects or crashes, so we only log at debug level
    if let Err(e) = stream.shutdown(std::net::Shutdown::Write) {
        log::debug!("[socket] Error during write shutdown (client may have disconnected): {}", e);
    }

    log::debug!("[socket] Successfully sent response");
    Ok(())
}

/// Client function to send a request to the daemon via socket
/// Implements retry logic with exponential backoff for transient failures
pub fn send_request(socket_path: &str, request: SocketRequest) -> Result<SocketResponse> {
    const MAX_RETRIES: u32 = 3;
    const INITIAL_BACKOFF_MS: u64 = 50;
    
    let mut last_error = None;
    
    for attempt in 0..MAX_RETRIES {
        match send_request_once(socket_path, &request) {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_error = Some(e);
                
                // Don't retry on the last attempt
                if attempt < MAX_RETRIES - 1 {
                    // Exponential backoff: 50ms, 100ms, 200ms
                    let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow(attempt);
                    std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                    log::debug!("Socket request failed (attempt {}/{}), retrying after {}ms", 
                               attempt + 1, MAX_RETRIES, backoff_ms);
                }
            }
        }
    }
    
    // All retries exhausted, return the last error
    Err(last_error.unwrap())
}

/// Internal function to attempt a single socket request without retry
pub(crate) fn send_request_once(socket_path: &str, request: &SocketRequest) -> Result<SocketResponse> {
    let mut stream = UnixStream::connect(socket_path).map_err(|e| {
        log::debug!("[socket client] Failed to connect to {}: {}", socket_path, e);
        anyhow!(
            "Failed to connect to daemon socket: {}. Is the daemon running?",
            e
        )
    })?;
    
    // Set timeouts for client connections as well
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    // Send request
    let request_json = serde_json::to_string(&request).map_err(|e| {
        log::error!("[socket client] Failed to serialize request: {}", e);
        anyhow!("Failed to serialize request: {}", e)
    })?;
    
    log::debug!("[socket client] Sending request ({} bytes)", request_json.len());
    
    stream.write_all(request_json.as_bytes()).map_err(|e| {
        log::error!("[socket client] Failed to write request: {}", e);
        anyhow!("Failed to write request: {}", e)
    })?;
    
    stream.write_all(b"\n").map_err(|e| {
        log::error!("[socket client] Failed to write newline: {}", e);
        anyhow!("Failed to write newline: {}", e)
    })?;
    
    stream.flush().map_err(|e| {
        log::error!("[socket client] Failed to flush request: {}", e);
        anyhow!("Failed to flush: {}", e)
    })?;
    
    // Shutdown the write side to signal we're done sending
    // This ensures the server knows no more data is coming
    // Note: In normal operation this should succeed. If it fails, it indicates
    // a connection problem and we should report it to the caller.
    stream.shutdown(std::net::Shutdown::Write).map_err(|e| {
        log::debug!("[socket client] Failed to shutdown write (connection issue): {}", e);
        anyhow!("Failed to shutdown write: {}", e)
    })?;

    log::debug!("[socket client] Request sent, waiting for response");

    // Read response
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| {
        log::error!("[socket client] Failed to read response: {}", e);
        anyhow!("Failed to read response: {}", e)
    })?;
    
    log::debug!("[socket client] Received response ({} bytes)", line.len());

    // Parse response
    let response: SocketResponse = serde_json::from_str(&line).map_err(|e| {
        log::error!("[socket client] Failed to parse response (len={}): {}", line.len(), e);
        anyhow!("Invalid response format: {}", e)
    })?;

    Ok(response)
}

/// Check if daemon is running by attempting to ping via socket
pub fn is_daemon_running(socket_path: &str) -> bool {
    match send_request(socket_path, SocketRequest::Ping) {
        Ok(SocketResponse::Pong) => true,
        _ => false,
    }
}
