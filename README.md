# Process Management Controller (OPM)

## Overview

OPM (Process Management Controller) is a simple PM2 alternative written in Rust. It provides a command-line/api interface to start, stop, restart, and manage fork processes

## Features

- Start, stop, restart, and reload processes.
- Watch for file changes and auto-reload processes.
- Set memory limits for processes.
- List all running processes with customizable output formats.
- Retrieve detailed information about a specific process.
- Get startup commands for processes.
- Use HTTP/rust api to control processes.

## Usage

```bash
# Start/Restart a process
opm start <id/name> or <script> [--name <name>] [--watch <path>] [--max-memory <limit>]

# Restart a process
opm restart <id/name>

# Reload a process (alias for restart)
opm reload <id/name>

# Stop/Kill a process
opm stop <id/name>

# Remove a process
opm remove <id/name>

# Get process info
opm info <id/name>

# Get process env
opm env <id/name>

# Get startup command for a process
opm cstart <id/name>

# Save all processes to dumpfile
opm save

# Restore all processes
opm restore

# List all processes
opm list [--format <raw|json|default>]

# Get process logs
opm logs <id/name> [--lines <num_lines>]

# Reset process index
opm daemon reset

# Stop daemon
opm daemon stop

# Start/Restart daemon
opm daemon start

# Check daemon health
opm daemon health
```

### Advanced Features

#### Watch Mode
Automatically reload your process when files change:
```bash
opm start app.js --watch .
```

#### Memory Limits
Set a maximum memory limit for a process:
```bash
opm start app.js --max-memory 500M
opm start app.py --max-memory 1G
```

#### Get Startup Command
Get the exact command used to start a process:
```bash
opm cstart 0
# or
opm get-command myapp
```

For more command information, check out `opm --help`

### Installation

Pre-built binaries for Linux, MacOS, and WSL can be found on the [releases](releases) page.

There is no windows support yet. Install from crates.io using `cargo install opm` (requires clang++)

#### Building

- Clone the project
- Open a terminal in the project folder
- Check if you have cargo (Rust's package manager) installed, just type in `cargo`
- If cargo is installed, run `cargo build --release`
- Put the executable into one of your PATH entries, usually `/bin/` or `/usr/bin/`
