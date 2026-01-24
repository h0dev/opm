# OPM Web UI and API Server

## Overview

OPM now includes a modern web-based user interface and RESTful API server for managing processes. The web UI provides a visual dashboard for monitoring and controlling your processes, while the API enables programmatic access and integration with other tools.

## Quick Start

### Starting the Server

```bash
# Start with API only
opm daemon restore --api

# Start with both API and Web UI
opm daemon restore --api --webui
```

### Accessing the Web UI

Once started, access the web interface at:
```
http://127.0.0.1:9876/
```

## Configuration

### Default Configuration

The default configuration is automatically created in `~/.opm/config.toml`:

```toml
[daemon.web]
ui = false              # Enable/disable web UI
api = false             # Enable/disable API server
address = "127.0.0.1"   # Listen address
port = 9876             # Listen port
path = "/"              # Optional: Base path for the API/UI
```

### Security Configuration

To enable API token authentication:

```toml
[daemon.web.secure]
enabled = true
token = "your-secret-token-here"
```

When security is enabled, all API requests must include a `token` header:

```bash
curl -H "token: your-secret-token-here" http://127.0.0.1:9876/daemon/list
```

### Enabling by Default

To start the API/UI automatically when the daemon starts, update your config:

```toml
[daemon.web]
ui = true    # Enable web UI by default
api = true   # Enable API by default
address = "127.0.0.1"
port = 9876
```

## Web UI Features

The web UI provides the following capabilities:

### Dashboard
- View all running processes at a glance
- Real-time status updates
- CPU and memory usage visualization
- Quick access to process controls

### Process Management
- Start, stop, restart, and reload processes
- View detailed process information
- Access process logs
- Monitor resource usage
- View environment variables

### Process Details Page
Each process has a dedicated page showing:
- Process metadata (PID, uptime, status)
- Resource usage (CPU, memory)
- Log output with real-time updates
- Environment variables
- Restart history

### Server Management
- View and manage remote OPM servers
- Switch between multiple OPM instances
- Monitor server health and status

## API Endpoints

### Health Check
```bash
GET /health
```
Returns server health status.

### Process Management

#### List Processes
```bash
GET /daemon/list
```
Returns a list of all managed processes.

#### Get Process Info
```bash
GET /daemon/info/{id}
```
Returns detailed information about a specific process.

#### Process Actions
```bash
POST /daemon/action
Content-Type: application/json

{
  "id": 0,
  "action": "start|stop|restart|reload"
}
```

#### Create Process
```bash
POST /daemon/create
Content-Type: application/json

{
  "script": "node app.js",
  "name": "my-app",
  "path": "/path/to/app",
  "watch": {
    "enabled": false,
    "path": "."
  }
}
```

### Logs

#### Get Process Logs
```bash
GET /daemon/logs/{id}?lines=100
```
Returns the last N lines of process logs.

#### Stream Logs (WebSocket)
```bash
GET /daemon/logs/{id}/stream
```
WebSocket endpoint for real-time log streaming.

### Metrics

#### Get Daemon Metrics
```bash
GET /daemon/metrics
```
Returns daemon CPU, memory, and runtime statistics.

#### Prometheus Metrics
```bash
GET /daemon/prometheus
```
Returns metrics in Prometheus format for monitoring integration.

### API Documentation

#### OpenAPI Specification
```bash
GET /openapi.json
```
Returns the full OpenAPI 3.0 specification.

#### Interactive Documentation
```bash
GET /docs/embed
```
Access Swagger UI for interactive API exploration and testing.

## Building from Source

### Development Build (API only, no UI)
```bash
cargo build
```

### Release Build without Web UI
```bash
cargo build --release
```

This builds OPM without the web interface. The API server will still work, but will show placeholder pages when accessing the web UI.

### Release Build with Web UI
```bash
cargo build --release --features webui
```

Building with the `webui` feature will:
1. Use system Node.js or download Node.js (v20.11.0) if not available
2. Install npm dependencies
3. Build the Astro-based web UI
4. Embed the compiled UI assets in the binary

**Note**: Building with the `webui` feature requires Node.js to be available on your system.

### Installing from Cargo

```bash
# Install without Web UI (recommended for simple installations)
cargo install opm

# Install with Web UI support (requires Node.js)
cargo install opm --features webui

# Install from GitHub without Web UI
cargo install --git https://github.com/h0dev/opm

# Install from GitHub with Web UI support
cargo install --git https://github.com/h0dev/opm --features webui
```

### Frontend Development

When making changes to the frontend (files in `src/webui/src/`), you need to rebuild the frontend:

```bash
# Navigate to the webui directory
cd src/webui

# Install dependencies (first time only)
npm install

# Build the frontend
npm run build

# Then rebuild the Rust binary with webui feature
cd ../..
cargo build --release --features webui
```

Note: Frontend changes are only embedded in builds with the `webui` feature enabled. Builds without the feature will show placeholder pages. For rapid frontend development, you can use Astro's dev server:

```bash
cd src/webui
npm run dev
```

This starts a development server at `http://localhost:4321` with hot reload.

## Technology Stack

### Backend
- **Rust** - High-performance, safe systems programming
- **Rocket** - Web framework for the API server
- **Tokio** - Async runtime
- **Tera** - Template engine
- **Prometheus** - Metrics collection

### Frontend (Web UI)
- **Astro** - Modern static site builder
- **React** - UI components
- **TailwindCSS** - Styling
- **TypeScript** - Type-safe JavaScript

## Troubleshooting

### Web UI Not Loading

If you see "WebUI not available" or placeholder pages:

1. The binary was built without the `webui` feature. To enable the web UI:
```bash
# Rebuild with webui feature enabled
cargo build --release --features webui
```

2. If you installed from cargo/git without the feature:
```bash
# Reinstall with webui support
cargo install opm --features webui --force
# or
cargo install --git https://github.com/h0dev/opm --features webui
```

### Build Errors with webui Feature

If you encounter errors when building with `--features webui`:

1. **Node.js not found**: Ensure Node.js is installed on your system
   ```bash
   node --version  # Should show v16.0.0 or higher
   ```

2. **npm dependency errors**: Try clearing the npm cache and package-lock.json
   ```bash
   cd src/webui
   rm -rf node_modules package-lock.json
   npm install
   cd ../..
   cargo build --release --features webui
   ```

### API Not Accessible

1. Check if the daemon is running:
```bash
opm daemon health
```

2. Verify the API is enabled:
```bash
# Check config
cat ~/.opm/config.toml
```

3. Check if port is already in use:
```bash
lsof -i :9876
```

### Authentication Errors

If you're getting 401 Unauthorized errors:
1. Check if security is enabled in config
2. Ensure you're including the token header:
```bash
curl -H "token: your-token" http://127.0.0.1:9876/daemon/list
```

## Security Considerations

### Token Authentication
When deploying with API access:
- Always enable token authentication in production
- Use a strong, randomly generated token
- Rotate tokens regularly
- Never commit tokens to version control

### Network Access
- By default, OPM binds to `127.0.0.1` (localhost only)
- To allow remote access, change `address` in config
- Use a reverse proxy (nginx, caddy) for HTTPS
- Consider firewall rules for additional security

### Example Secure Configuration

```toml
[daemon.web]
ui = true
api = true
address = "0.0.0.0"    # Listen on all interfaces
port = 9876

[daemon.web.secure]
enabled = true
token = "$(openssl rand -hex 32)"  # Generate with: openssl rand -hex 32
```

## Examples

### Starting a Process via API
```bash
curl -X POST http://127.0.0.1:9876/daemon/create \
  -H "Content-Type: application/json" \
  -d '{
    "script": "python app.py",
    "name": "python-app",
    "path": "/home/user/my-app"
  }'
```

### Monitoring with Prometheus
```bash
# Add to prometheus.yml
scrape_configs:
  - job_name: 'opm'
    static_configs:
      - targets: ['localhost:9876']
    metrics_path: '/daemon/prometheus'
```

### Remote Server Management
```bash
# Add a remote OPM server
opm server new

# List servers
opm server list

# Use a specific server
opm list --server production
opm start app.js --server production
```

## Future Enhancements

Planned features for future releases:
- User authentication and authorization
- Process grouping and tagging
- Custom alerting rules
- Performance graphs and trends
- Cluster management
- Docker container integration
- Log aggregation and search
