# New Features

## 1. Adjust Command

The `adjust` command allows you to modify a process's execution command and/or name without removing and recreating the process.

### Usage

```bash
# Update both command and name
opm adjust <id|name> --command "new command" --name "new-name"

# Update only command
opm adjust <id|name> --command "new command"

# Update only name
opm adjust <id|name> --name "new-name"
```

### Examples

```bash
# Update by process ID
opm adjust 1 --command "node updated-server.js" --name "new-name"

# Update by process name
opm adjust my-app --command "python new-script.py"

# Rename a process
opm adjust 3 --name "renamed-process"
```

### Notes
- At least one of `--command` or `--name` must be provided
- Changes take effect on next restart of the process
- The process itself is not restarted by this command

## 2. Worker-Based Load Balancing

The `start` command now supports spawning multiple worker instances for load balancing.

### Usage

```bash
opm start --workers <count> [--port-range <range>] <command>
# Or use abbreviated flags:
opm start -w <count> [-p <range>] <command>
```

### Parameters

- `-w, --workers <count>`: Number of worker instances to spawn (minimum 2)
- `-p, --port-range <range>`: Port allocation for workers
  - Format: `"start-end"` for unique ports per worker (e.g., "3000-3002")
  - Format: `"port"` for shared port using SO_REUSEPORT (e.g., "3000")

### Examples

```bash
# Start 3 workers on different ports (long flags)
opm start --name "web-server" --workers 3 --port-range "3000-3002" "node server.js"

# Start 3 workers on different ports (abbreviated flags)
opm start --name "web-server" -w 3 -p "3000-3002" "node server.js"

# Start 4 workers using SO_REUSEPORT (all on same port)
opm start --name "api" -w 4 -p "8080" "python app.py"

# Start workers without specific port configuration
opm start --name "worker-pool" -w 5 "node worker.js"
```

### Worker Naming

Workers are automatically assigned unique names with the pattern: `<base-name>-worker-<n>`

Example: If you use `--name "web-server"`, workers will be named:
- `web-server-worker-1`
- `web-server-worker-2`
- `web-server-worker-3`

### Load Balancing Modes

#### Unique Port Per Worker
When using a port range (e.g., `--port-range "3000-3002"`), each worker is assigned a unique port. Your application needs to handle port binding appropriately.

#### SO_REUSEPORT Mode
When using a single port (e.g., `--port-range "3000"`), all workers can bind to the same port if your application supports SO_REUSEPORT. This allows the operating system to distribute incoming connections across all worker processes.

### Notes
- Each worker is a separate process with its own ID and PID
- Workers can be managed individually (stop, restart, etc.)
- The port range must match the number of workers when specified
- Workers do not automatically receive PORT environment variable - applications should read from their environment or configuration

## 3. Crash Counter Reset on Restore

The crash counter is automatically reset for ALL processes when the restore command is run. This gives all processes a fresh start with full restart attempts available.

### Behavior
- Occurs automatically during `opm restore` command
- Resets both `restarts` and `crash.value` counters
- Applies to **all processes in the system** (both running and stopped)
- Ensures every process gets a clean slate after system restore/reboot
- Already implemented - no user action required

## 4. PM2-like Direct Execution Strategy

OPM now intelligently chooses between direct process execution and shell-wrapped execution based on command complexity, similar to PM2's behavior.

### How It Works

**Simple Commands (Direct Execution):**
- Commands without shell operators spawn directly without a shell wrapper
- Eliminates the "intermediate shell PID" problem
- Examples:
  ```bash
  opm start "node server.js"        # Spawns directly as: node server.js
  opm start "python app.py"         # Spawns directly as: python app.py
  opm start "./binary --flag value" # Spawns directly as: ./binary --flag value
  ```

**Complex Commands (Shell Execution):**
- Commands with shell operators use the configured shell (sh/bash from config)
- Shell operators detected: `&&`, `||`, `|`, `>`, `>>`, `<`, `;`, `&`, `` ` ``, `$()`, `~`, `*`, `export`, `source`, `alias`
- Examples:
  ```bash
  opm start "node app.js | grep error"          # Uses shell due to pipe
  opm start "echo hello && node server.js"      # Uses shell due to &&
  opm start "node app.js > output.log"          # Uses shell due to redirect
  ```

### Benefits
- **Better PID tracking**: Simple commands have no shell wrapper, making PID management cleaner
- **Performance**: Direct execution is slightly faster (no shell overhead)
- **Compatibility**: Complex commands still work exactly as before with full shell support
- **Configuration**: Shell selection (`sh` or `bash`) still respects your `config.toml` settings

### Configuration
The shell used for complex commands is configured in `~/.opm/config.toml`:
```toml
[runner]
shell = "/bin/sh"     # or "/bin/bash"
args = ["-c"]
```

## 5. Parallel Process Restoration

The `opm restore` command now spawns all processes concurrently instead of sequentially, dramatically reducing restore time.

### Performance Improvements
- **Before**: Processes spawned sequentially with 1-second wait between each
  - 3 processes = ~3+ seconds
  - 10 processes = ~10+ seconds
- **After**: All processes spawn in parallel with single 1-second stabilization wait
  - Any number of processes = ~1-2 seconds

### How It Works
1. All processes are spawned concurrently using thread pool
2. System waits 1 second for all processes to stabilize
3. Verification checks run to confirm successful startup
4. Process list automatically displayed

### Benefits
- **Faster restores**: Multi-process restores complete 3-10x faster
- **Better resource utilization**: CPU cores used efficiently
- **Improved reliability**: Proper synchronization prevents race conditions
- **Same guarantees**: All safety checks and PID tracking still work correctly

### Example
```bash
# Before (sequential): ~10 seconds for 10 processes
# After (parallel): ~1-2 seconds for 10 processes
opm restore
```

The parallel restoration is automatic and requires no configuration changes.
