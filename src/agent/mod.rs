/// Agent module for remote process management
///
/// This module provides functionality for OPM agents to connect to a central server
/// and report process information. All communication between agents and the server
/// is handled via WebSocket connections for real-time, bidirectional communication.
///
/// # Architecture
///
/// - **Agent Connection**: Agents use WebSocket (`/ws/agent`) to connect to the server
/// - **Message Protocol**: JSON-based messages defined in `messages.rs`
/// - **Registry**: Server maintains an agent registry to track connected agents
///
/// # WebSocket Communication
///
/// The agent establishes a persistent WebSocket connection to the server and sends:
/// - `AgentMessage::Register`: Initial registration with agent info
/// - `AgentMessage::Heartbeat`: Periodic heartbeat to maintain connection
/// - `AgentMessage::ProcessUpdate`: Real-time process list updates
///
/// The server responds with:
/// - `AgentMessage::Response`: Acknowledgment of received messages
/// - `AgentMessage::Ping`/`Pong`: Connection health checks
///
/// # Migration Note
///
/// Prior versions used HTTP REST endpoints for agent registration and heartbeat.
/// These have been completely replaced with WebSocket communication for improved
/// real-time performance and reduced overhead.
///
pub mod connection;
pub mod messages;
pub mod registry;
pub mod resource_usage;
pub mod types;
