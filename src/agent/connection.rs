use super::messages::AgentMessage;
use super::types::{AgentConfig, AgentInfo, AgentStatus};
use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};

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

        // Connect to WebSocket server
        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .map_err(|e| anyhow!("Failed to connect to WebSocket: {}", e))?;

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        // Construct the API endpoint URL (optional - for backward compatibility)
        let api_endpoint = None; // Agents no longer need to expose API endpoint

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
        // Send process updates every 10 seconds (configurable)
        let mut process_update_interval = tokio::time::interval(Duration::from_secs(10));

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
        }
    }
}
