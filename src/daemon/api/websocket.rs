use opm::agent::messages::AgentMessage;
use opm::agent::registry::AgentRegistry;
use opm::agent::types::{AgentInfo, AgentStatus, ConnectionType};
use opm::notifications::{NotificationManager, NotificationEvent};
use opm::process::ProcessItem;
use rocket::{State, get};
use rocket_ws::{Message, Stream, WebSocket};
use tokio::sync::mpsc;

/// WebSocket route handler for agent connections
/// 
/// This is the primary communication channel between agents and the server.
/// Agents connect to this endpoint and send/receive messages for:
/// - Registration (AgentMessage::Register)
/// - Heartbeat (AgentMessage::Heartbeat)
/// - Process updates (AgentMessage::ProcessUpdate)
/// - Action requests (AgentMessage::ActionRequest) - server to agent
/// - Action responses (AgentMessage::ActionResponse) - agent to server
/// - Ping/Pong for connection health checks
/// 
/// All agent communication including process actions is now handled via WebSocket.
#[get("/ws/agent")]
pub fn websocket_handler(
    ws: WebSocket, 
    registry: &State<AgentRegistry>,
    notif_mgr: &State<std::sync::Arc<NotificationManager>>,
    event_log: &State<std::sync::Arc<opm::events::EventLog>>,
) -> Stream!['static] {
    let registry = registry.inner().clone();
    let notif_mgr = notif_mgr.inner().clone();
    let event_log = event_log.inner().clone();

    Stream! { ws =>
        let mut agent_id: Option<String> = None;
        
        // Create a channel for sending messages to the agent
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        for await message in ws {
            // First check if there are any outgoing messages to send
            while let Ok(outgoing_msg) = rx.try_recv() {
                yield Message::Text(outgoing_msg);
            }
            
            match message {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<AgentMessage>(&text) {
                        Ok(agent_msg) => {
                            match agent_msg {
                                AgentMessage::Register { id, name, hostname, api_endpoint } => {
                                    log::info!("[WebSocket] Agent registration: {} ({})", name, id);

                                    // Clone hostname once for notification
                                    let hostname_for_notif = hostname.clone();

                                    let agent_info = AgentInfo {
                                        id: id.clone(),
                                        name: name.clone(),
                                        hostname,
                                        status: AgentStatus::Online,
                                        connection_type: ConnectionType::In,
                                        last_seen: std::time::SystemTime::now(),
                                        connected_at: std::time::SystemTime::now(),
                                        api_endpoint,
                                        system_info: None,
                                    };

                                    // Register agent with sender channel for bidirectional communication
                                    registry.register_with_sender(agent_info, tx.clone());
                                    agent_id = Some(id.clone());

                                    // Send notification about agent connection
                                    let notif_title = "Agent Connected";
                                    let notif_message = format!(
                                        "Agent '{}' (ID: {}) has connected{}",
                                        name,
                                        id,
                                        hostname_for_notif.map(|h| format!(" from {}", h)).unwrap_or_default()
                                    );
                                    let nm = notif_mgr.clone();
                                    let el = event_log.clone();
                                    tokio::spawn(async move {
                                        nm.send(NotificationEvent::AgentConnect, &notif_title, &notif_message).await;
                                        el.log(
                                            opm::events::EventType::AgentConnect,
                                            notif_title.to_string(),
                                            notif_message
                                        ).await;
                                    });

                                    // Send success response
                                    let response = AgentMessage::Response {
                                        success: true,
                                        message: "Agent registered successfully".to_string(),
                                    };

                                    if let Ok(response_json) = serde_json::to_string(&response) {
                                        yield Message::Text(response_json);
                                    }
                                }
                                AgentMessage::Heartbeat { id } => {
                                    log::debug!("[WebSocket] Heartbeat from agent {}", id);

                                    if registry.update_heartbeat(&id) {
                                        // Send pong response
                                        let response = AgentMessage::Response {
                                            success: true,
                                            message: "Heartbeat received".to_string(),
                                        };

                                        if let Ok(response_json) = serde_json::to_string(&response) {
                                            yield Message::Text(response_json);
                                        }
                                    } else {
                                        // Agent not found in registry
                                        let response = AgentMessage::Response {
                                            success: false,
                                            message: "Agent not found".to_string(),
                                        };

                                        if let Ok(response_json) = serde_json::to_string(&response) {
                                            yield Message::Text(response_json);
                                        }

                                        // Close connection
                                        break;
                                    }
                                }
                                AgentMessage::SystemInfoUpdate { id, system_info } => {
                                    log::debug!("[WebSocket] System info update from agent {}", id);

                                    if registry.update_system_info(&id, system_info) {
                                        log::debug!("[WebSocket] System info updated for agent {}", id);
                                    } else {
                                        log::warn!("[WebSocket] Failed to update system info for agent {}", id);
                                    }
                                }
                                AgentMessage::ProcessUpdate { id, processes } => {
                                    log::debug!("[WebSocket] Process update from agent {}", id);

                                    // Parse processes from JSON values
                                    let parsed_processes: Vec<ProcessItem> = processes
                                        .into_iter()
                                        .filter_map(|p| serde_json::from_value(p).ok())
                                        .collect();

                                    registry.update_processes(&id, parsed_processes);

                                    // Send acknowledgment
                                    let response = AgentMessage::Response {
                                        success: true,
                                        message: "Process update received".to_string(),
                                    };

                                    if let Ok(response_json) = serde_json::to_string(&response) {
                                        yield Message::Text(response_json);
                                    }
                                }
                                AgentMessage::ActionResponse { request_id, success, message } => {
                                    log::info!("[WebSocket] Action response: request_id={}, success={}, message={}", 
                                        request_id, success, message);
                                    // Action responses are logged
                                    // In a full implementation, this would resolve a pending Future
                                }
                                AgentMessage::Pong => {
                                    log::debug!("[WebSocket] Pong received from agent");
                                    // Update last_seen time
                                    if let Some(ref id) = agent_id {
                                        registry.update_heartbeat(id);
                                    }
                                }
                                AgentMessage::Ping => {
                                    // Respond to ping with pong
                                    let pong_msg = AgentMessage::Pong;
                                    if let Ok(pong_json) = serde_json::to_string(&pong_msg) {
                                        yield Message::Text(pong_json);
                                    }
                                }
                                _ => {
                                    log::warn!("[WebSocket] Unexpected message type");
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("[WebSocket] Failed to parse message: {}", e);
                        }
                    }
                }
                Ok(Message::Ping(data)) => {
                    // Respond to WebSocket ping with pong
                    yield Message::Pong(data);
                }
                Ok(Message::Pong(_)) => {
                    // Update heartbeat on pong
                    if let Some(ref id) = agent_id {
                        registry.update_heartbeat(id);
                    }
                }
                Ok(Message::Close(_)) => {
                    log::info!("[WebSocket] Agent disconnected");
                    break;
                }
                Err(e) => {
                    log::error!("[WebSocket] Error receiving message: {}", e);
                    break;
                }
                _ => {}
            }
        }

        // Cleanup: unregister agent on disconnect
        if let Some(id) = agent_id {
            log::info!("[WebSocket] Unregistering agent {}", id);
            
            // Get agent info before unregistering for notification
            let agent_name = registry.get(&id).map(|a| a.name.clone());
            
            registry.unregister(&id);
            
            // Send notification about agent disconnection
            if let Some(name) = agent_name {
                let notif_title = "Agent Disconnected";
                let notif_message = format!("Agent '{}' (ID: {}) has disconnected", name, id);
                tokio::spawn(async move {
                    notif_mgr.send(NotificationEvent::AgentDisconnect, &notif_title, &notif_message).await;
                    event_log.log(
                        opm::events::EventType::AgentDisconnect,
                        notif_title.to_string(),
                        notif_message
                    ).await;
                });
            }
        }
    }
}
