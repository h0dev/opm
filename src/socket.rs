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

use crate::process;
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
    LoadPermanent,
    /// Remove a process by ID (handled by daemon to avoid race conditions)
    RemoveProcess(usize),
    /// Stop a process by ID
    StopProcess(usize),
    /// Start a process by ID
    StartProcess(usize),
    /// Restart a process by ID
    RestartProcess(usize),
    /// Edit a process (name and/or command)
    EditProcess {
        id: usize,
        name: Option<String>,
        command: Option<String>,
    },
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
pub fn start_socket_server_with_callback<F>(
    socket_path: &str,
    ready_callback: Option<F>,
) -> Result<()>
where
    F: FnOnce() + Send + 'static,
{
    // Remove old socket file if it exists
    if Path::new(socket_path).exists() {
        fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;

    // Set permissions to allow cross-user CLI access when daemon runs under a different user
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o666);
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
            log::error!(
                "[socket] Failed to parse request (len={}): {}",
                line.len(),
                e
            );
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
            // Merge the caller's snapshot with the authoritative memory cache.
            // See `merge_setstate` for the identity rules.
            let merged_runner = match dump::read_memory_direct_option() {
                Some(current) => merge_setstate(current, runner),
                None => runner,
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
        SocketRequest::LoadPermanent => {
            dump::load_permanent_into_memory();
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
                let children = runner
                    .info(id)
                    .map(|p| p.children.clone())
                    .unwrap_or_default();

                // IMPORTANT: Mark process as stopped BEFORE removing from list
                // This prevents race condition where daemon's restart_process() loop
                // detects the process is dead and tries to restart it during removal
                // Safe to call process(id) because we're inside runner.exists(id) check
                runner.process(id).running = false;
                runner.process(id).crash.crashed = false; // Clear crashed flag

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
                let children = runner
                    .info(id)
                    .map(|p| p.children.clone())
                    .unwrap_or_default();

                // Mark as stopped and clear crashed flag
                runner.process(id).running = false;
                runner.process(id).crash.crashed = false; // Clear crashed flag

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
                let children = runner
                    .info(id)
                    .map(|p| p.children.clone())
                    .unwrap_or_default();

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
            // Edit a process's name and/or command in RAM. `Runner::edit` is the
            // one implementation of a rename, shared with the CLI paths, so the
            // daemon and a daemon-less cache behave identically.
            let permanent = dump::read_permanent_direct();
            let memory = dump::read_memory_direct_option();
            let mut runner = dump::merge_runners_public(permanent, memory);

            if runner.edit(id, name, command) {
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
        log::debug!(
            "[socket] Error during write shutdown (client may have disconnected): {}",
            e
        );
    }

    log::debug!("[socket] Successfully sent response");
    Ok(())
}

/// Identifies a process across renumbering: the properties a user picks it by.
type Identity = process::Identity;

/// Merge a caller's `SetState` snapshot into the authoritative memory cache.
///
/// Snapshot callers read the state, mutate it and send it back, so their ids can
/// be stale: the daemon compacts ids on every removal, which shifts every id
/// above the removed one. Keying on ids therefore confused identity with
/// position — it duplicated processes, moved them to other rows and resurrected
/// deleted ones. Processes are matched by identity `(name, script, path)` instead:
///
/// - An identity present in `current` is updated **in place**: it keeps the
///   current id (the incoming id is stale) and its derived state (log paths).
/// - An incoming process whose identity is unknown is new; it is inserted under
///   its own id, or under a freshly allocated id when that id is taken.
/// - A process in `current` that no incoming process claims is preserved: only
///   an explicit deletion (`RemoveProcess`) removes a process.
/// - A rename never travels in a snapshot: the new name is a new identity, so
///   it would be inserted as a second process and the old name would stay.
///   Renames are edits by id (`SocketRequest::EditProcess`), applied to the
///   cache in place without reindexing.
/// - The id counter never moves backwards.
fn merge_setstate(mut current: Runner, incoming: Runner) -> Runner {
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

    let incoming_counter = incoming.id.counter.load(Ordering::SeqCst);
    let mut incoming_by_identity: HashMap<Identity, process::Process> = incoming
        .list
        .into_values()
        .map(|process| (process::identity(&process), process))
        .collect();

    {
        for (id, existing) in current.list.iter_mut() {
            let Some(mut process) = incoming_by_identity.remove(&process::identity(existing)) else {
                continue;
            };

            process.name = existing.name.clone();

            process.id = *id;
            process.log_out = existing.log_out.clone();
            process.log_error = existing.log_error.clone();

            // A manual stop is an instruction, not runtime state: it must survive
            // the merge whatever the snapshot says, or the daemon sees a process
            // that is supposed to be running, treats the exit as a crash and
            // counts a restart the user never asked for.
            if existing.manual_stop {
                process.manual_stop = true;
                process.running = false;
            }

            // A snapshot with pid=0 (old dump, daemon restart, restore clearing
            // pids) must not wipe the live runtime state of a running process.
            //
            // The exception is a stop: the caller cleared the pid on purpose, and
            // putting it back would leave a stopped process looking like one that
            // died while running, which the daemon restarts.
            let stopping = process.manual_stop || existing.manual_stop;
            if existing.pid > 0 && process.pid == 0 && !stopping {
                process.pid = existing.pid;
                process.shell_pid = existing.shell_pid;
                process.children = existing.children.clone();
                process.running = existing.running;
                process.started = existing.started;
                log::debug!(
                    "[socket] Preserved PID for process '{}' (id={}): pid={}, running={}",
                    process.name,
                    id,
                    process.pid,
                    process.running
                );
            }

            *existing = process;
        }
    }

    // Whatever no identity claimed is new to this daemon.
    for (_, mut process) in incoming_by_identity {
        let snapshot_id = process.id;
        let id = if current.list.contains_key(&snapshot_id) {
            current.alloc_id()
        } else {
            snapshot_id
        };

        log::debug!(
            "[socket] Accepted new process '{}' as id={} (snapshot id was {})",
            process.name,
            id,
            snapshot_id
        );
        process.id = id;
        current.list.insert(id, process);
    }

    // The counter follows the ids that exist. Keeping the larger of the two
    // counters preserved history that no longer exists: after a removal the
    // daemon could hold a counter past the end of a contiguous list, and the
    // next process got an id with a gap in front of it.
    //
    // `incoming_counter` is deliberately ignored: ids beyond this list are
    // history, and `alloc_id` skips any id that is already taken.
    let _ = incoming_counter;
    if let Some(highest) = current.list.keys().copied().max() {
        current
            .id
            .counter
            .store(highest.saturating_add(1), Ordering::SeqCst);
    } else {
        current.id.counter.store(0, Ordering::SeqCst);
    }

    current
}

/// Client function to send a request to the daemon via socket
/// Implements retry logic with exponential backoff for transient failures
pub fn send_request(socket_path: &str, request: SocketRequest) -> Result<SocketResponse> {
    const MAX_RETRIES: u32 = 3;
    const INITIAL_BACKOFF_MS: u64 = 50;

    let mut last_error = None;
    let candidates = collect_socket_paths(socket_path);

    for path in candidates {
        for attempt in 0..MAX_RETRIES {
            match send_request_once(&path, &request) {
                Ok(response) => return Ok(response),
                Err(e) => {
                    last_error = Some(e);

                    // Don't retry on the last attempt
                    if attempt < MAX_RETRIES - 1 {
                        // Exponential backoff: 50ms, 100ms, 200ms
                        let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow(attempt);
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                        log::debug!(
                            "Socket request failed (attempt {}/{} for {}), retrying after {}ms",
                            attempt + 1,
                            MAX_RETRIES,
                            path,
                            backoff_ms
                        );
                    }
                }
            }
        }
    }

    Err(last_error.unwrap())
}

fn collect_socket_paths(primary: &str) -> Vec<String> {
    use std::collections::HashSet;
    use std::path::Path;

    let mut paths: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    if let Ok(env_path) = std::env::var("OPM_SOCKET") {
        if !env_path.trim().is_empty() && seen.insert(env_path.clone()) {
            paths.push(env_path);
        }
    }

    if seen.insert(primary.to_string()) {
        paths.push(primary.to_string());
    }

    if Path::new("/root/.opm/opm.sock").exists() && seen.insert("/root/.opm/opm.sock".to_string()) {
        paths.push("/root/.opm/opm.sock".to_string());
    }

    if let Ok(entries) = std::fs::read_dir("/home") {
        for entry in entries.flatten() {
            let candidate = entry.path().join(".opm/opm.sock");
            if candidate.exists() {
                let candidate = candidate.to_string_lossy().to_string();
                if seen.insert(candidate.clone()) {
                    paths.push(candidate);
                }
            }
        }
    }

    paths
}

/// Internal function to attempt a single socket request without retry
pub(crate) fn send_request_once(
    socket_path: &str,
    request: &SocketRequest,
) -> Result<SocketResponse> {
    let mut stream = UnixStream::connect(socket_path).map_err(|e| {
        log::debug!(
            "[socket client] Failed to connect to {}: {}",
            socket_path,
            e
        );
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

    log::debug!(
        "[socket client] Sending request ({} bytes)",
        request_json.len()
    );

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
        log::debug!(
            "[socket client] Failed to shutdown write (connection issue): {}",
            e
        );
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
        log::error!(
            "[socket client] Failed to parse response (len={}): {}",
            line.len(),
            e
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Crash, Process, Watch};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn process(id: usize, name: &str, pid: i64, started_secs: i64) -> Process {
        Process {
            id,
            pid,
            shell_pid: None,
            env: BTreeMap::new(),
            name: name.to_string(),
            path: PathBuf::from("/tmp/opm-e2e"),
            script: "node /tmp/probe.js".to_string(),
            restarts: 0,
            running: pid > 0,
            crash: Crash { crashed: false },
            watch: Watch {
                enabled: false,
                path: String::new(),
                hash: String::new(),
            },
            children: vec![],
            started: chrono::DateTime::from_timestamp(started_secs, 0)
                .expect("timestamp is valid"),
            max_memory: 0,
            agent_id: None,
            frozen_until: None,
            last_action_at: chrono::DateTime::from_timestamp(started_secs, 0)
                .expect("timestamp is valid"),
            manual_stop: false,
            errored: false,
            last_restart_attempt: None,
            failed_restart_attempts: 0,
            session_id: None,
            process_start_time: None,
            spawned_pid: None,
            is_process_tree: false,
            log_out: Some(format!("/root/.opm/logs/{name}-out.log")),
            log_error: Some(format!("/root/.opm/logs/{name}-error.log")),
        }
    }

    fn runner(counter: usize, processes: Vec<Process>) -> Runner {
        let mut list = BTreeMap::new();
        for item in processes {
            list.insert(item.id, item);
        }

        let mut runner = Runner::default();
        runner.id = crate::process::id::Id::new(counter);
        runner.list = list;
        runner
    }

    fn names_and_pids(runner: &Runner) -> Vec<(usize, String, i64)> {
        runner
            .list
            .iter()
            .map(|(id, process)| (*id, process.name.clone(), process.pid))
            .collect()
    }

    // ---------------------------------------------------------------------
    // Regression suite for the reported defects (fix-process-id-integrity).
    // The stale-snapshot merge that used to duplicate the survivors and
    // resurrect removed rows fails on the pre-fix (id-keyed) merge.
    // ---------------------------------------------------------------------

    pub(super) fn reported_process(id: usize, name: &str, pid: i64) -> Process {
        // Identity is (name, script, path): the fixture keeps one script and
        // path and lets the name, like the reported apps, tell them apart.
        let mut process = process(id, name, pid, 1 + id as i64);
        process.script = "node /tmp/probe.js".to_string();
        process.path = std::path::PathBuf::from("/tmp/opm-e2e");
        process.running = pid > 0;
        process
    }

    pub(super) fn reported_runner(counter: usize, processes: Vec<Process>) -> Runner {
        runner(counter, processes)
    }

    pub(super) fn reported_names(runner: &Runner) -> Vec<(usize, String)> {
        runner
            .list
            .iter()
            .map(|(id, process)| (*id, process.name.clone()))
            .collect()
    }


    #[test]
    fn regression_reported_stale_snapshot_merge_does_not_duplicate() {
        // Reported: a stale snapshot (captured before `opm remove 3,4`, carrying
        // pre-compaction ids) written through the merge path produced 8 rows for
        // 6 real processes — app6 and app7 twice, each copy with the same PID —
        // and moved app6/app7 onto other rows.
        //
        // A process absent from the current cache is new to the daemon (only
        // RemoveProcess deletes), so the merge may accept it under a spare id.
        // What it must never do is put an identity in the list twice or move a
        // survivor off the id it already has.
        let current = reported_runner(
            9,
            vec![
                reported_process(0, "app1", 11301),
                reported_process(1, "app4", 11304),
                reported_process(2, "app5", 11305),
                reported_process(3, "app6", 11306),
                reported_process(4, "app7", 11307),
            ],
        );
        let stale = reported_runner(
            9,
            vec![
                reported_process(0, "app1", 11301),
                reported_process(1, "app2", 11302),
                reported_process(2, "app3", 11303),
                reported_process(3, "app4", 11304),
                reported_process(4, "app5", 11305),
                reported_process(5, "app6", 11306),
                reported_process(6, "app7", 11307),
            ],
        );

        let merged = merge_setstate(current, stale);

        for (id, name) in [
            (0, "app1"),
            (1, "app4"),
            (2, "app5"),
            (3, "app6"),
            (4, "app7"),
        ] {
            let entry = merged
                .list
                .get(&id)
                .unwrap_or_else(|| panic!("id {id} must still hold {name}: {:?}", reported_names(&merged)));
            assert_eq!(entry.name, name, "id {id} must keep {name}: {:?}", reported_names(&merged));
        }
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for process in merged.list.values() {
            *counts.entry(process.name.as_str()).or_default() += 1;
        }
        let duplicated: Vec<(&str, usize)> = counts
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(name, count)| (*name, *count))
            .collect();
        assert!(
            duplicated.is_empty(),
            "no identity may appear twice (the report: 8 rows for 6 processes); duplicated {duplicated:?} in {:?}",
            reported_names(&merged)
        );
        assert_eq!(
            merged.list.len(),
            7,
            "5 survivors plus the two rows a plain SetState may re-accept: {:?}",
            reported_names(&merged)
        );
    }

    #[test]
    fn regression_reported_stale_snapshot_never_duplicates_a_survivor() {
        // The rows that survive a removal must be matched by identity even though
        // the stale snapshot pairs them with their pre-removal ids. There must be
        // exactly one row per identity afterwards, whatever id the extra entries
        // take.
        let current = reported_runner(
            9,
            vec![
                reported_process(0, "app1", 11301),
                reported_process(1, "app5", 11305),
                reported_process(2, "app6", 11306),
                reported_process(3, "app7", 11307),
            ],
        );
        let stale = reported_runner(
            9,
            vec![
                reported_process(0, "app1", 11301),
                reported_process(1, "app2", 11302),
                reported_process(2, "app3", 11303),
                reported_process(3, "app4", 11304),
                reported_process(4, "app5", 11305),
                reported_process(5, "app6", 11306),
                reported_process(6, "app7", 11307),
            ],
        );

        let merged = merge_setstate(current, stale);

        for (id, name) in [
            (0, "app1"),
            (1, "app5"),
            (2, "app6"),
            (3, "app7"),
        ] {
            let entry = merged.list.get(&id).unwrap_or_else(|| {
                panic!("id {id} must still hold {name}: {:?}", reported_names(&merged))
            });
            assert_eq!(entry.name, name, "stale ids must not move {name}: {:?}", reported_names(&merged));
        }

        // One row per identity: a duplicated identity is what the report saw as
        // two rows with the same live PID.
        for name in ["app1", "app5", "app6", "app7"] {
            assert_eq!(
                merged.list.values().filter(|p| p.name == name).count(),
                1,
                "{name} must appear exactly once: {:?}",
                reported_names(&merged)
            );
        }
        // The rows the plan allows a plain SetState to re-accept (absent from
        // current; only RemoveProcess deletes) must each be inserted once.
        for name in ["app2", "app3", "app4"] {
            assert!(
                merged.list.values().filter(|p| p.name == name).count() <= 1,
                "{name} may be accepted at most once: {:?}",
                reported_names(&merged)
            );
        }
        for id in [0usize, 1, 2, 3] {
            assert!(
                merged.list.get(&id).map(|p| ["app1", "app5", "app6", "app7"].contains(&p.name.as_str())) == Some(true),
                "survivor id {id} was overwritten: {:?}",
                reported_names(&merged)
            );
        }
    }

    #[test]
    fn test_setstate_stale_snapshot_does_not_duplicate_processes() {
        // Real reproduction (base commit): seven processes, then `opm remove 3,4`.
        // The first removal compacted ids, so the second call's snapshot (taken
        // before the first removal) still described ids 3 and 4 with the pre-remove
        // identities. Merging that snapshot by numeric id added app6/app7 a second
        // time at ids 7 and 8, both carrying the same live PID.
        let current = runner(
            9,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
                process(3, "app4", 0, 4),
                process(4, "app6", 114227, 5),
                process(5, "app7", 114241, 6),
                process(6, "app5", 114280, 7),
            ],
        );
        let incoming = runner(
            9,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
                process(3, "app4", 114189, 4),
                process(4, "app5", 114280, 7),
                process(5, "app6", 114227, 5),
                process(6, "app7", 114241, 6),
            ],
        );

        let merged = merge_setstate(current, incoming);

        assert_eq!(
            names_and_pids(&merged),
            vec![
                (0, "app1".to_string(), 114130),
                (1, "app2".to_string(), 114157),
                (2, "app3".to_string(), 114185),
                (3, "app4".to_string(), 114189),
                (4, "app6".to_string(), 114227),
                (5, "app7".to_string(), 114241),
                (6, "app5".to_string(), 114280),
            ],
            "a stale snapshot must not duplicate processes or move an existing one to another id"
        );
    }

    #[test]
    fn test_setstate_does_not_resurrect_a_deleted_process_from_a_contaminated_snapshot() {
        // The resurrection seen live at base: `opm remove 3,4` deleted app5 too,
        // but the contaminated memory cache still listed it, so the second
        // removal's snapshot carried app5's identity back in. A stale snapshot can
        // only lay claim to identities the current list really has; a process the
        // caller never asked to delete must stay deleted.
        let current = runner(
            9,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
                process(3, "app6", 114227, 5),
            ],
        );
        let incoming = runner(
            9,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
                process(3, "app4", 114189, 4),
                process(4, "app5", 114280, 6),
                process(5, "app6", 114227, 5),
            ],
        );

        let merged = merge_setstate(current, incoming);

        // app4 and app5 are absent from the current list, so they exist nowhere
        // else; they are new to this daemon and are accepted under fresh ids — but
        // never in place of a process that is already there.
        assert_eq!(merged.list.len(), 6, "no identity may be inserted twice: {:?}", names_and_pids(&merged));
        for name in ["app1", "app2", "app3", "app4", "app5", "app6"] {
            assert_eq!(
                merged.list.values().filter(|p| p.name == name).count(),
                1,
                "{name} must appear exactly once"
            );
        }
        assert_eq!(
            names_and_pids(&merged)[..4],
            [
                (0, "app1".to_string(), 114130),
                (1, "app2".to_string(), 114157),
                (2, "app3".to_string(), 114185),
                (3, "app6".to_string(), 114227),
            ],
            "the processes already in the list must keep their ids and slots"
        );
        assert!(
            merged.list[&4].name == "app4" || merged.list[&4].name == "app5",
            "id 4 is free, so one of the new processes takes it"
        );
    }

    #[test]
    fn test_setstate_deleted_process_stays_deleted_when_absent_from_current() {
        // Explicit deletion is `RemoveProcess`; a snapshot that no longer contains
        // app5 must not delete it either, and a snapshot that carries an identity
        // which no longer exists anywhere is the only way it can return. Once the
        // contaminating entry is gone from the current list, nothing can claim it.
        let current = runner(
            7,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app6", 114227, 5),
            ],
        );
        let incoming = runner(
            7,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app6", 114227, 5),
            ],
        );

        let merged = merge_setstate(current, incoming);

        assert_eq!(
            names_and_pids(&merged),
            vec![
                (0, "app1".to_string(), 114130),
                (1, "app6".to_string(), 114227),
            ],
            "a process missing from the snapshot must survive the merge"
        );
    }

    #[test]
    fn test_setstate_matches_by_identity_when_ids_shifted() {
        // Compaction renumbers ids; a stale snapshot pairs an identity with the id
        // the process had before the shift. The identity match must win, and the
        // CURRENT id must win too (`opm <cmd> 6` resolves against the current list).
        let current = runner(
            7,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app5", 114280, 7),
                process(2, "app6", 114227, 5),
            ],
        );
        let incoming = runner(
            7,
            vec![
                process(0, "app1", 114130, 1),
                process(3, "app6", 114227, 5),
                process(6, "app5", 114280, 7),
            ],
        );

        let merged = merge_setstate(current, incoming);

        assert_eq!(
            names_and_pids(&merged),
            vec![
                (0, "app1".to_string(), 114130),
                (1, "app5".to_string(), 114280),
                (2, "app6".to_string(), 114227),
            ],
            "identity is (name, script, path), so stale ids must not move a process"
        );
    }

    #[test]
    fn test_setstate_accepts_new_process_and_reallocates_colliding_id() {
        let current = runner(
            3,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
            ],
        );
        // A concurrent `opm start` allocated id 3 while the caller held a stale
        // counter, so the incoming new process claims an id that is already taken
        // by a different identity.
        let incoming = runner(
            4,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
                process(3, "app9", 114999, 9),
            ],
        );

        let merged = merge_setstate(current, incoming);

        assert_eq!(
            names_and_pids(&merged),
            vec![
                (0, "app1".to_string(), 114130),
                (1, "app2".to_string(), 114157),
                (2, "app3".to_string(), 114185),
                (3, "app9".to_string(), 114999),
            ],
            "a genuinely new process must be accepted under a free id"
        );
    }

    #[test]
    fn test_setstate_new_process_colliding_on_id_gets_a_free_id() {
        let current = runner(
            3,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
            ],
        );
        let incoming = runner(
            4,
            vec![
                process(0, "app1", 114130, 1),
                process(1, "app2", 114157, 2),
                process(2, "app3", 114185, 3),
                process(1, "app9", 114999, 9),
            ],
        );

        let merged = merge_setstate(current, incoming);

        assert_eq!(
            names_and_pids(&merged),
            vec![
                (0, "app1".to_string(), 114130),
                (1, "app2".to_string(), 114157),
                (2, "app3".to_string(), 114185),
                (3, "app9".to_string(), 114999),
            ],
            "a new process must never overwrite the process already at that id"
        );
    }

    #[test]
    fn test_setstate_preserves_current_state_when_incoming_pid_is_zero() {
        // Existing rule: a snapshot with pid=0 (daemon restart, old dump, restore
        // clearing pids) must not wipe the live pid/children of a running process.
        let mut current_process = process(0, "app1", 114130, 1);
        current_process.children = vec![114131, 114132];
        let current = runner(1, vec![current_process]);
        let incoming = runner(1, vec![process(0, "app1", 0, 1)]);

        let merged = merge_setstate(current, incoming);

        let entry = merged.list.get(&0).expect("process must survive the merge");
        assert_eq!(entry.pid, 114130, "live pid must be preserved");
        assert_eq!(
            entry.children,
            vec![114131, 114132],
            "live children must be preserved"
        );
        assert!(entry.running, "running state must be preserved");
    }

    #[test]
    fn rename_through_editprocess_lands_in_place() {
        // The rename path is an edit by id, applied to the cache in place. The
        // id is the address and does not move, and the live runtime state stays
        // attached to the process: a rename must never be a `SetState` snapshot,
        // because the merge matches by identity `(name, script, path)` and the
        // new name matches nothing (see the test below for that defect).
        let mut runner = runner(3, vec![reported_process(0, "app1", 4242)]);

        assert!(
            runner.edit(0, Some("renamed_app".to_string()), None),
            "EditProcess must find the process by id"
        );
        assert_eq!(
            reported_names(&runner),
            vec![(0, "renamed_app".to_string())],
            "an edit by id must rename in place: {:?}",
            reported_names(&runner)
        );
        assert_eq!(
            runner.list.get(&0).map(|p| p.pid),
            Some(4242),
            "the live pid must survive the rename"
        );
    }

    #[test]
    fn edit_process_reports_a_missing_id_instead_of_creating_one() {
        // The `EditProcess` handler answers `Error("Process N not found")` when
        // `edit` returns false, so an edit of a removed process must not insert
        // anything.
        let mut runner = runner(3, vec![reported_process(0, "app1", 4242)]);

        assert!(!runner.edit(9, Some("ghost".to_string()), None));
        assert_eq!(
            reported_names(&runner),
            vec![(0, "app1".to_string())],
            "an edit of a missing id must not add a row"
        );
    }

    #[test]
    fn edit_process_applies_a_command_change_without_touching_the_name() {
        let mut runner = runner(3, vec![reported_process(0, "app1", 4242)]);

        assert!(runner.edit(0, None, Some("node other.js".to_string())));

        let process = runner.list.get(&0).expect("process must survive");
        assert_eq!(process.script, "node other.js");
        assert_eq!(process.name, "app1", "a command change must not rename");
    }

    #[test]
    fn rename_that_travels_as_a_snapshot_is_lost() {
        // Pins the defect that routed renames through `EditProcess`: a rename
        // carried by a `SetState` snapshot is a new identity to the merge, so it
        // lands as a second process and the old name keeps the live pid.
        let current = runner(3, vec![reported_process(0, "app1", 4242)]);
        let mut renamed = reported_process(0, "renamed_app", 4242);
        renamed.pid = 0;
        let incoming = runner(3, vec![renamed]);

        let merged = merge_setstate(current, incoming);
        let names: Vec<String> = merged
            .list
            .values()
            .map(|process| process.name.clone())
            .collect();

        assert_eq!(
            names.len(),
            2,
            "a snapshot rename is inserted as a second process: {names:?}"
        );
        assert_eq!(
            merged.list.get(&0).map(|p| (p.name.as_str(), p.pid)),
            Some(("app1", 4242)),
            "the old name keeps the live process: {names:?}"
        );
        assert!(
            names.contains(&"renamed_app".to_string()),
            "the renamed copy is a separate row: {names:?}"
        );
    }

    #[test]
    fn test_setstate_never_lowers_the_id_counter() {
        // A stale snapshot carries an old counter. Lowering it would let the next
        // allocation reuse ids that the current list still needs.
        let current = runner(
            9,
            vec![
                process(0, "app1", 114130, 1),
                process(8, "app7", 114241, 6),
            ],
        );
        let incoming = runner(3, vec![process(0, "app1", 114130, 1)]);

        let merged = merge_setstate(current, incoming);

        assert!(
            merged.id.counter.load(std::sync::atomic::Ordering::SeqCst) >= 9,
            "counter must never move backwards, got {}",
            merged.id.counter.load(std::sync::atomic::Ordering::SeqCst)
        );
    }
}
