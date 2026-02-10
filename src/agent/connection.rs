use super::messages::AgentMessage;
use super::types::{AgentConfig, AgentInfo, AgentStatus};
use crate::process;
use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use rustls::crypto::ring;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async_tls_with_config, tungstenite::Message, Connector};

pub struct AgentConnection {
    config: AgentConfig,
    status: AgentStatus,
}

impl AgentConnection {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            status: AgentStatus::Offline,
        }
    }

    /// Start the agent connection using WebSocket
    pub async fn run(&mut self) -> Result<()> {
        let _ = ring::default_provider().install_default();
        println!(
            "[Agent] Starting agent '{}' (ID: {})",
            self.config.name, self.config.id
        );
        println!("[Agent] Connecting to server: {}", self.config.server_url);

        loop {
            if let Err(e) = self.websocket_mode().await {
                eprintln!("[Agent] Connection error: {}", e);
                self.status = AgentStatus::Reconnecting;
            }

            // Reconnection backoff
            println!(
                "[Agent] Reconnecting in {} seconds...",
                self.config.reconnect_interval
            );
            sleep(Duration::from_secs(self.config.reconnect_interval)).await;
        }
    }

    async fn websocket_mode(&mut self) -> Result<()> {
        println!("[Agent] Starting WebSocket connection mode");

        // Parse server URL and construct WebSocket URL
        // Expected format: http://host:port or https://host:port
        let server_url = self.config.server_url.trim_end_matches('/');

        // Use the same port as HTTP server (WebSocket is now integrated)
        let ws_url = if server_url.starts_with("https://") {
            let base = server_url.strip_prefix("https://").unwrap();
            let (host, port) = if base.contains(':') {
                let parts: Vec<&str> = base.split(':').collect();
                let port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(443);
                (parts[0], port)
            } else {
                // No port specified, use default HTTPS port
                (base, 443)
            };
            format!("wss://{}:{}/ws/agent", host, port)
        } else {
            let base = server_url.strip_prefix("http://").unwrap_or(server_url);
            let (host, port) = if base.contains(':') {
                let parts: Vec<&str> = base.split(':').collect();
                let port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(80);
                (parts[0], port)
            } else {
                // No port specified, use default HTTP port
                (base, 80)
            };
            format!("ws://{}:{}/ws/agent", host, port)
        };

        println!("[Agent] Connecting to WebSocket: {}", ws_url);

        // Create a TLS connector with native root certificates
        let mut root_store = RootCertStore::empty();
        match rustls_native_certs::load_native_certs() {
            Ok(certs) => {
                for cert in certs {
                    if root_store.add(cert).is_err() {
                        log::warn!("[Agent] Failed to add a native certificate to the trust store");
                    }
                }
            }
            Err(e) => {
                log::error!("[Agent] Could not load native certs: {}", e);
                // Depending on security policy, you might want to stop here
                // For now, we'll proceed with an empty root store, which will likely fail
            }
        }

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let connector = Connector::Rustls(Arc::new(config));

        // Connect to WebSocket server
        let (ws_stream, _) = connect_async_tls_with_config(&ws_url, None, false, Some(connector))
            .await
            .map_err(|e| anyhow!("Failed to connect to WebSocket: {}", e))?;

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        // Construct the API endpoint URL
        // Agents expose an API server so the main server can send action requests
        let api_endpoint = {
            // Determine the external IP address
            // Check if the configured address is a bind-all address (0.0.0.0, ::, 127.0.0.1, ::1)
            const BIND_ALL_IPV4: &str = "0.0.0.0";
            const LOCALHOST_IPV4: &str = "127.0.0.1";

            if self.config.api_address == BIND_ALL_IPV4
                || self.config.api_address == LOCALHOST_IPV4
                || self.config.api_address == "::"
                || self.config.api_address == "::1"
            {
                // Try to get the hostname for better network accessibility
                // In containerized or complex network environments, the hostname
                // might need to be configured explicitly via the agent config
                let detected_hostname = hostname::get().ok().and_then(|h| h.into_string().ok());

                match detected_hostname {
                    Some(hostname) if hostname != "localhost" && !hostname.is_empty() => {
                        Some(format!("http://{}:{}", hostname, self.config.api_port))
                    }
                    _ => {
                        // Could not determine a valid hostname
                        // Don't report localhost as the API endpoint since it won't be reachable
                        // from the server in most network configurations
                        log::warn!(
                            "Could not determine a network-accessible hostname for agent API endpoint. \
                            Process management actions will not be available from the server UI. \
                            To enable actions, configure the agent with an accessible IP address \
                            or hostname using the --api-address option."
                        );
                        eprintln!(
                            "[Agent] WARNING: API endpoint cannot be determined. \
                            Process actions from server will not work."
                        );
                        eprintln!(
                            "[Agent] To fix this, start the agent with: \
                            --api-address <your-ip-or-hostname>"
                        );
                        None // Return None to indicate no API endpoint available
                    }
                }
            } else {
                Some(format!(
                    "http://{}:{}",
                    self.config.api_address, self.config.api_port
                ))
            }
        };

        // Send registration message
        let register_msg = AgentMessage::Register {
            id: self.config.id.clone(),
            name: self.config.name.clone(),
            hostname: hostname::get().ok().and_then(|h| h.into_string().ok()),
            api_endpoint,
        };

        let register_json = serde_json::to_string(&register_msg)
            .map_err(|e| anyhow!("Failed to serialize registration: {}", e))?;

        ws_sender
            .send(Message::Text(register_json))
            .await
            .map_err(|e| anyhow!("Failed to send registration: {}", e))?;

        println!("[Agent] Registration sent");

        // Wait for registration response
        if let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(response) = serde_json::from_str::<AgentMessage>(&text) {
                        if let AgentMessage::Response { success, message } = response {
                            if success {
                                println!("[Agent] Successfully registered with server");
                                self.status = AgentStatus::Online;
                            } else {
                                return Err(anyhow!("Registration failed: {}", message));
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    return Err(anyhow!("Server closed connection"));
                }
                Err(e) => {
                    return Err(anyhow!("WebSocket error: {}", e));
                }
                _ => {}
            }
        }

        // Start heartbeat and process update loop
        let mut heartbeat_interval =
            tokio::time::interval(Duration::from_secs(self.config.heartbeat_interval));
        // Send process updates every 1 second (configurable)
        let mut process_update_interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                // Send heartbeat periodically
                _ = heartbeat_interval.tick() => {
                    let heartbeat_msg = AgentMessage::Heartbeat {
                        id: self.config.id.clone(),
                    };

                    if let Ok(heartbeat_json) = serde_json::to_string(&heartbeat_msg) {
                        if let Err(e) = ws_sender.send(Message::Text(heartbeat_json)).await {
                            eprintln!("[Agent] Failed to send heartbeat: {}", e);
                            return Err(anyhow!("Heartbeat failed: {}", e));
                        }
                        log::debug!("[Agent] Heartbeat sent");
                    }

                    // Also send system info update with heartbeat
                    let system_info = self.collect_system_info();
                    let system_info_msg = AgentMessage::SystemInfoUpdate {
                        id: self.config.id.clone(),
                        system_info,
                    };

                    match serde_json::to_string(&system_info_msg) {
                        Ok(system_info_json) => {
                            if let Err(e) = ws_sender.send(Message::Text(system_info_json)).await {
                                eprintln!("[Agent] Failed to send system info: {}", e);
                                // Don't return error here, just log it
                            } else {
                                log::debug!("[Agent] System info update sent");
                            }
                        }
                        Err(e) => {
                            eprintln!("[Agent] Failed to serialize system info: {}", e);
                        }
                    }
                }

                // Send process updates periodically
                _ = process_update_interval.tick() => {
                    // Fetch current process list
                    use crate::process::Runner;
                    let runner = Runner::new();
                    let processes = runner.fetch();

                    // Convert to JSON values for serialization
                    let process_values: Vec<serde_json::Value> = processes
                        .into_iter()
                        .map(|p| serde_json::to_value(p).unwrap_or(serde_json::Value::Null))
                        .collect();

                    let process_update_msg = AgentMessage::ProcessUpdate {
                        id: self.config.id.clone(),
                        processes: process_values,
                    };

                    if let Ok(update_json) = serde_json::to_string(&process_update_msg) {
                        if let Err(e) = ws_sender.send(Message::Text(update_json)).await {
                            eprintln!("[Agent] Failed to send process update: {}", e);
                            return Err(anyhow!("Process update failed: {}", e));
                        }
                        log::debug!("[Agent] Process update sent");
                    }
                }

                // Receive messages from server
                msg = ws_receiver.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(response) = serde_json::from_str::<AgentMessage>(&text) {
                                match response {
                                    AgentMessage::Response { success, message } => {
                                        if !success {
                                            if message.contains("not found") {
                                                eprintln!("[Agent] Agent has been removed from server. Disconnecting...");
                                                std::process::exit(0);
                                            }
                                            eprintln!("[Agent] Server response: {}", message);
                                            return Err(anyhow!("Server error: {}", message));
                                        }
                                    }
                                    AgentMessage::Ping => {
                                        // Respond to ping with pong
                                        let pong_msg = AgentMessage::Pong;
                                        if let Ok(pong_json) = serde_json::to_string(&pong_msg) {
                                            let _ = ws_sender.send(Message::Text(pong_json)).await;
                                        }
                                    }
                                     AgentMessage::ActionRequest { request_id, process_id, method } => {
                                         log::info!("[Agent] Received action request: {} for process {}", method, process_id);

                                         // Execute the action locally
                                         use crate::process::Runner;
                                         let mut runner = Runner::new();

                                         let (success, message) = if runner.exists(process_id) {
                                              match method.as_str() {
                                                  "start" => {
                                                      let mut item = runner.get(process_id);
                                                      item.restart(false);
                                                      item.get_runner().save();
                                                      // Create timestamp file for this action to prevent daemon from overriding state
                                                      // Write with fsync to ensure timestamp is durably written before daemon checks
                                                      if let Err(e) = process::write_action_timestamp(process_id) {
                                                          ::log::warn!("Failed to create action timestamp file for process {}: {}", process_id, e);
                                                      }
                                                      (true, format!("Process {} started", process_id))
                                                  }
                                                  "restart" => {
                                                      let mut item = runner.get(process_id);
                                                      item.restart(true);
                                                      item.get_runner().save();
                                                      // Create timestamp file for this action to prevent daemon from overriding state
                                                      // Write with fsync to ensure timestamp is durably written before daemon checks
                                                      if let Err(e) = process::write_action_timestamp(process_id) {
                                                          ::log::warn!("Failed to create action timestamp file for process {}: {}", process_id, e);
                                                      }
                                                      (true, format!("Process {} restarted", process_id))
                                                  }
                                                  "reload" => {
                                                      let mut item = runner.get(process_id);
                                                      item.reload(true);
                                                      item.get_runner().save();
                                                      // Create timestamp file for this action to prevent daemon from overriding state
                                                      // Write with fsync to ensure timestamp is durably written before daemon checks
                                                      if let Err(e) = process::write_action_timestamp(process_id) {
                                                          ::log::warn!("Failed to create action timestamp file for process {}: {}", process_id, e);
                                                      }
                                                      (true, format!("Process {} reloaded", process_id))
                                                  }
                                                  "stop" | "kill" => {
                                                      let mut item = runner.get(process_id);
                                                      item.stop();
                                                      item.get_runner().save();
                                                      // Create timestamp file for this action to prevent daemon from overriding state
                                                      // Write with fsync to ensure timestamp is durably written before daemon checks
                                                      if let Err(e) = process::write_action_timestamp(process_id) {
                                                          ::log::warn!("Failed to create action timestamp file for process {}: {}", process_id, e);
                                                      }
                                                      (true, format!("Process {} stopped", process_id))
                                                  }
                                                  "reset_env" | "clear_env" => {
                                                      let mut item = runner.get(process_id);
                                                      item.clear_env();
                                                      item.get_runner().save();
                                                      // Create timestamp file for this action to prevent daemon from overriding state
                                                      // Write with fsync to ensure timestamp is durably written before daemon checks
                                                      if let Err(e) = process::write_action_timestamp(process_id) {
                                                          ::log::warn!("Failed to create action timestamp file for process {}: {}", process_id, e);
                                                      }
                                                      (true, format!("Process {} environment cleared", process_id))
                                                  }
                                                  "remove" | "delete" => {
                                                      runner.remove(process_id);
                                                      // Clean up any action timestamp file after removal
                                                      if let Some(home_dir) = home::home_dir() {
                                                          let action_file = format!("{}/.opm/last_action_{}.timestamp", home_dir.display(), process_id);
                                                          let _ = std::fs::remove_file(action_file);
                                                      }
                                                      (true, format!("Process {} removed", process_id))
                                                  }
                                                  "flush" | "clean" => {
                                                      runner.flush(process_id);
                                                      (true, format!("Process {} logs flushed", process_id))
                                                  }
                                                  _ => {
                                                      (false, format!("Unknown action: {}", method))
                                                  }
                                              }
                                         } else {
                                             (false, format!("Process {} not found", process_id))
                                         };

                                         // Send response back to server
                                         let response_msg = AgentMessage::ActionResponse {
                                             request_id,
                                             success,
                                             message,
                                         };

                                         if let Ok(response_json) = serde_json::to_string(&response_msg) {
                                             if let Err(e) = ws_sender.send(Message::Text(response_json)).await {
                                                 log::error!("[Agent] Failed to send action response: {}", e);
                                             }
                                         }

                                          // Immediately send process update after action to ensure UI reflects changes quickly
                                          // Only send immediate updates for actions that modify process state
                                          if success && matches!(method.as_str(), "start" | "restart" | "reload" | "stop" | "kill" | "remove" | "delete") {
                                              // Small delay to ensure action completion before fetching process list
                                              tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                                              // Fetch current process list after action has been processed
                                              let updated_runner = Runner::new();
                                              let processes = updated_runner.fetch();

                                              let process_values: Vec<serde_json::Value> = processes
                                                  .into_iter()
                                                  .filter_map(|p| {
                                                      serde_json::to_value(p).map_err(|e| {
                                                          log::warn!("[Agent] Failed to serialize process: {}", e);
                                                          e
                                                      }).ok()
                                                  })
                                                  .collect();

                                              let process_update_msg = AgentMessage::ProcessUpdate {
                                                  id: self.config.id.clone(),
                                                  processes: process_values,
                                              };

                                              if let Ok(update_json) = serde_json::to_string(&process_update_msg) {
                                                  if let Err(e) = ws_sender.send(Message::Text(update_json)).await {
                                                      log::error!("[Agent] Failed to send immediate process update: {}", e);
                                                  } else {
                                                      log::info!("[Agent] Immediate process update sent to server after action");
                                                  }
                                              }
                                          }
                                    }
                                    AgentMessage::SaveRequest { request_id } => {
                                        log::info!("[Agent] Received save request");

                                        // Save all processes locally
                                        use crate::process::Runner;

                                        let (success, message) = match std::panic::catch_unwind(|| {
                                            let runner = Runner::new();
                                            runner.save_permanent();
                                        }) {
                                            Ok(_) => {
                                                log::info!("[Agent] Processes saved successfully");
                                                (true, "Processes saved successfully".to_string())
                                            }
                                            Err(_) => {
                                                log::error!("[Agent] Failed to save processes - operation panicked");
                                                (false, "Failed to save processes".to_string())
                                            }
                                        };

                                        // Send response back to server
                                        let response_msg = AgentMessage::ActionResponse {
                                            request_id,
                                            success,
                                            message,
                                        };

                                        if let Ok(response_json) = serde_json::to_string(&response_msg) {
                                            if let Err(e) = ws_sender.send(Message::Text(response_json)).await {
                                                log::error!("[Agent] Failed to send save response: {}", e);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            // Respond to WebSocket ping with pong
                            let _ = ws_sender.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => {
                            println!("[Agent] Server closed connection");
                            return Err(anyhow!("Server closed connection"));
                        }
                        Some(Err(e)) => {
                            eprintln!("[Agent] WebSocket error: {}", e);
                            return Err(anyhow!("WebSocket error: {}", e));
                        }
                        None => {
                            println!("[Agent] WebSocket stream ended");
                            return Err(anyhow!("WebSocket stream ended"));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn collect_system_info(&self) -> super::types::SystemInfo {
        let os_info = os_info::get();
        let mem_info = sys_info::mem_info().ok();

        super::types::SystemInfo {
            os_name: format!("{:?}", os_info.os_type()),
            os_version: os_info.version().to_string(),
            arch: os_info.architecture().unwrap_or("unknown").to_string(),
            cpu_count: Some(num_cpus::get()),
            total_memory: mem_info.map(|m| m.total),
            resource_usage: super::resource_usage::gather_resource_usage(),
        }
    }

    pub fn get_info(&self) -> AgentInfo {
        use super::types::ConnectionType;
        let api_endpoint = format!(
            "http://{}:{}",
            self.config.api_address, self.config.api_port
        );

        AgentInfo {
            id: self.config.id.clone(),
            name: self.config.name.clone(),
            hostname: hostname::get().ok().and_then(|h| h.into_string().ok()),
            status: self.status.clone(),
            connection_type: ConnectionType::In,
            last_seen: std::time::SystemTime::now(),
            connected_at: std::time::SystemTime::now(),
            api_endpoint: Some(api_endpoint),
            system_info: Some(self.collect_system_info()),
        }
    }
}
