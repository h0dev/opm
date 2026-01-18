# OPM Role System

## Overview

OPM now supports a role-based system that allows you to configure it as a **server**, **agent**, or **standalone** instance. This enables distributed process management where a central server can monitor and control processes across multiple agent machines.

## Roles

### Standalone (Default)
- The default role for OPM
- Operates independently without any server-agent relationship
- Can manage local processes only
- Suitable for single-machine deployments

### Server
- Acts as a central control point for multiple agents
- Can view and control processes on all connected agents
- Can also manage its own local processes
- Requires API to be enabled

### Agent
- Connects to a server instance
- Exposes its processes to be controlled by the server
- Can only manage its own local processes (cannot access remote servers)
- Automatically starts with API enabled

## Configuration

The role is configured in `~/.opm/config.toml`:

```toml
# Role can be: "standalone", "server", or "agent"
role = "standalone"

[daemon.web]
ui = false
api = false
address = "127.0.0.1"
port = 9876

[daemon.web.secure]
enabled = true
token = "your-secure-token"
```

## Setting Up a Server

1. Configure OPM as a server:
   ```bash
   # Edit ~/.opm/config.toml and set:
   role = "server"
   
   [daemon.web]
   api = true
   ui = true  # optional, for Web UI
   address = "0.0.0.0"  # listen on all interfaces
   port = 9876
   ```

2. Start the daemon:
   ```bash
   opm daemon restore --api --webui
   ```

3. Check the server status:
   ```bash
   opm daemon health
   # Should show: role: server
   ```

## Setting Up an Agent

1. On the agent machine, connect to the server:
   ```bash
   opm agent connect http://192.168.1.100:9876 --name my-agent
   ```

   This will:
   - Set the role to "agent"
   - Configure the agent API endpoint
   - Start the local daemon with API enabled
   - Begin sending heartbeats to the server

2. Check the agent status:
   ```bash
   opm agent status
   ```

3. View agent processes (agent can only see its own):
   ```bash
   opm list
   ```

## Server Operations

From the server machine, you can view all agents:

```bash
# View connected agents (via API)
curl http://localhost:9876/daemon/agents/list

# View processes for a specific agent (via API)
curl http://localhost:9876/daemon/agents/{agent-id}/processes
```

## Agent Restrictions

When running in agent role:
- ✅ Can manage local processes
- ✅ Can view local process information
- ✅ Can start/stop/restart local processes
- ❌ Cannot use `--server` parameter to access remote servers
- ❌ Cannot manage processes on other machines

Attempting restricted operations will result in an error:
```
✘ Agent role cannot perform remote operations. Only local process management is allowed.
```

## Disconnecting an Agent

To disconnect an agent from a server:

```bash
opm agent disconnect
```

This will:
- Restore the role to "standalone"
- Remove the agent configuration
- The local daemon continues running normally

## Architecture

```
┌─────────────────┐
│     Server      │
│  (role=server)  │
│   Port 9876     │ ← HTTP API + WebSocket
└────────┬────────┘
         │
         │ WebSocket Connections
         │ (Registration & Heartbeats)
         │
    ┌────┴────┬────────────┬──────────┐
    │         │            │          │
┌───▼───┐ ┌──▼────┐  ┌───▼────┐ ┌──▼────┐
│Agent 1│ │Agent 2│  │Agent 3 │ │Agent N│
│Port   │ │Port   │  │Port    │ │Port   │
│9877   │ │9877   │  │9877    │ │9877   │
└───────┘ └───────┘  └────────┘ └───────┘
```

Each agent:
1. Connects to the server via WebSocket at `/ws/agent` endpoint on the same port
2. Registers with agent ID, name, hostname, and API endpoint
3. Sends periodic heartbeats through the WebSocket connection
4. Maintains persistent bidirectional connection
5. Exposes API for server to control processes (on its own port)

**Key Benefits of WebSocket:**
- Agents don't need to open ports for server connections
- Real-time bidirectional communication
- More efficient than HTTP polling
- Automatic reconnection on connection loss
- Single port for both HTTP API and WebSocket (simplified deployment)

## Use Cases

### Development Teams
- Server on a central machine
- Agents on developer workstations
- Monitor all development processes from one place

### Production Deployments
- Server on monitoring/management node
- Agents on application servers
- Centralized process monitoring and control

### CI/CD Pipelines
- Server as part of CI/CD infrastructure
- Agents on build/test machines
- Automated process management across environments

## Security Considerations

1. **API Authentication**: Always enable API token authentication:
   ```toml
   [daemon.web.secure]
   enabled = true
   token = "your-secure-random-token"
   ```

2. **Network Security**: 
   - Use firewalls to restrict access to API ports
   - Consider using VPN for agent-server communication
   - Use HTTPS proxy if exposing to public networks

3. **Agent Authorization**:
   - Agents can only control their own processes
   - Server has full control over all agents
   - Consider which machines should have server role

## Troubleshooting

### Agent Not Connecting
1. Check network connectivity: `curl http://server:9876/health`
2. Verify server API is enabled: `opm daemon health` on server
3. Check agent logs: `tail -f ~/.opm/agent.log`
4. Verify WebSocket endpoint is accessible: `curl -i -N -H "Connection: Upgrade" -H "Upgrade: websocket" -H "Sec-WebSocket-Key: test" -H "Sec-WebSocket-Version: 13" http://server:9876/ws/agent`
5. Check firewall settings allow both HTTP and WebSocket connections on the same port

### Permission Denied Errors
- Ensure you're not trying to use `--server` parameter on an agent
- Verify the role is correctly set in config

### Agent Not Showing Up
1. Check server's agent list: `curl http://server:9876/daemon/agents/list`
2. Verify agent is sending heartbeats (check agent logs)
3. Ensure agent API endpoint is reachable from server

## Commands Reference

### Agent Management
- `opm agent connect <server-url>` - Connect to a server as an agent
- `opm agent status` - Show agent connection status
- `opm agent disconnect` - Disconnect from server
- `opm agent list` - Show information about viewing agents

### Daemon Management
- `opm daemon health` - Shows role in output
- `opm daemon restore --api` - Start daemon with API
- `opm daemon stop` - Stop daemon

### Process Management
All standard process management commands work as before:
- `opm start <script>`
- `opm list`
- `opm stop <id>`
- `opm restart <id>`
- etc.

The only difference is that agents cannot use the `--server` parameter.
