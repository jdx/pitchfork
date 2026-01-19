# How Pitchfork Works

Pitchfork is a process supervisor designed for developers. It manages background services (daemons) with features tailored for development workflows.

## Core Concept

Pitchfork runs a **supervisor daemon** in the background that manages all your processes. CLI commands communicate with this supervisor via a Unix socket.

```
You → CLI → Supervisor → Your Daemons
```

## Why Use Pitchfork?

### Problem: Managing Development Services

When developing, you often need multiple services:
- Database (PostgreSQL, Redis)
- API server
- Frontend dev server
- Background workers

Managing these manually is tedious:
- Starting them in the right order
- Checking if they're already running
- Restarting when they crash
- Stopping them when you're done

### Solution: Pitchfork

Pitchfork handles this automatically:

```bash
# Start all your services with one command
pitchfork start --all

# Or let them start automatically when you cd into your project
cd ~/projects/myapp
# Services start automatically

cd ~
# Services stop automatically
```

## How It Differs from Alternatives

| Tool | Focus | Pitchfork Advantage |
|------|-------|---------------------|
| Shell jobs (`&`) | One-off background processes | Ready checks, restart on failure, log management |
| systemd | System services | Project-scoped, directory-aware auto-start/stop |
| pm2 | Node.js processes | Language-agnostic, simpler config |
| Docker Compose | Containerized services | No containers needed, lighter weight |

## Key Features

### 1. Ready Checks

Know when your service is actually ready, not just started:

```toml
[daemons.api]
run = "npm run server"
ready_http = "http://localhost:3000/health"
```

### 2. Auto Start/Stop

Services follow you as you move between projects:

```toml
[daemons.api]
run = "npm run server"
auto = ["start", "stop"]
```

### 3. Restart on Failure

Keep services running even when they crash:

```toml
[daemons.api]
run = "npm run server"
retry = 3
```

### 4. Cron Scheduling

Run tasks on a schedule:

```toml
[daemons.backup]
run = "./backup.sh"
cron = { schedule = "0 0 2 * * *" }
```

## Architecture Overview

See [Architecture](/concepts/architecture) for technical details.
