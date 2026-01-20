# Open Process Management (OPM) Documentation

## Overview

OPM (Open Process Management) is a simple PM2 alternative written in Rust. It provides a command-line/API interface to start, stop, restart, and manage fork processes, with a web UI for easier management.

## Key Features

- Start, stop, restart, and reload processes
- Watch for file changes and auto-reload processes
- Set memory limits for processes
- List all running processes with customizable output formats
- HTTP/Rust API for remote process control
- Web UI for visual process management
- Desktop notifications for process events
- Configuration import/export with HCL files

## Quick Start

### Starting the Daemon

```bash
# Start daemon with API only
opm daemon restore --api

# Start daemon with both API and Web UI
opm daemon restore --api --webui
```

### Basic Commands

```bash
# Start a process
opm start <script> [--name <name>] [--watch <path>]

# List all processes
opm list

# Stop a process
opm stop <id/name>

# Restart a process
opm restart <id/name>

# View process logs
opm logs <id/name>
```

## Configuration

The daemon can be configured in `~/.opm/config.toml`:

```toml
[daemon]
restarts = 10      # Maximum restart attempts for a process
interval = 1000    # Interval between restart attempts (ms)

[daemon.web]
ui = false      # Enable/disable web UI
api = false     # Enable/disable API server
address = "127.0.0.1"
port = 9876

# Optional: API security
[daemon.web.secure]
enabled = true
token = "your-secret-token"

# Optional: Restore cleanup configuration
[daemon.restore_cleanup]
process_logs = true  # Delete process logs on restore (default: true)
daemon_log = true    # Delete daemon.log on restore (default: true)
agent_log = true     # Delete agent.log on restore (default: true)
```

### Restore Cleanup Configuration

When the daemon is restored (e.g., after system reboot), you can control which logs are automatically cleaned:

- **process_logs**: Cleans all process log files in the logs directory (e.g., `0-out.log`, `1-error.log`)
- **daemon_log**: Cleans the daemon's own log file (`~/.opm/daemon.log`)
- **agent_log**: Cleans the agent connection log file (`~/.opm/agent.log`)

All cleanup options default to `true` (enabled). Set them to `false` if you want to preserve logs across restores.

## API Endpoints

The API server provides REST endpoints for process management:

- `GET /health` - Check server health
- `GET /daemon/list` - List all processes
- `GET /daemon/info/{id}` - Get process details
- `POST /daemon/action` - Control processes (start, stop, restart)
- `GET /openapi.json` - OpenAPI specification

## Web UI

Once started with `--webui`, you can access the web interface at:

```
http://127.0.0.1:9876/
```

Default configuration:
- Address: 127.0.0.1
- Port: 9876

The web UI provides:
- Visual process list with status indicators
- Process control (start, stop, restart)
- Real-time metrics and monitoring
- Agent management for distributed process control
- Notification settings configuration

## Additional Resources

**Repository:** [lab.themackabu.dev/self/opm](https://lab.themackabu.dev/self/opm)

**License:** MIT

For complete usage information, see [README.md](README.md)
