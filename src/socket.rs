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

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;

use crate::process::{Runner, dump};

/// Request types that can be sent to the daemon via socket
#[derive(Debug, Serialize, Deserialize)]
pub enum SocketRequest {
    /// Get the current process state (Runner)
    GetState,
    /// Update the process state
    SetState(Runner),
    /// Save the current state to permanent storage
    SavePermanent,
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
    log::info!("Unix socket server started at {}", socket_path);

    // Handle connections in a loop
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || {
                    if let Err(e) = handle_client(stream) {
                        log::error!("Error handling socket client: {}", e);
                    }
                });
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
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    
    // Read request line
    reader.read_line(&mut line)?;
    
    // Parse request
    let request: SocketRequest = serde_json::from_str(&line)?;
    
    // Process request
    let response = match request {
        SocketRequest::GetState => {
            // Read merged state (permanent + memory cache)
            let runner = dump::read_merged();
            SocketResponse::State(runner)
        }
        SocketRequest::SetState(runner) => {
            // Write to memory cache
            dump::write_memory(&runner);
            SocketResponse::Success
        }
        SocketRequest::SavePermanent => {
            // Commit memory cache to permanent storage
            dump::commit_memory();
            SocketResponse::Success
        }
        SocketRequest::Ping => {
            SocketResponse::Pong
        }
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
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| anyhow!("Failed to connect to daemon socket: {}. Is the daemon running?", e))?;
    
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
