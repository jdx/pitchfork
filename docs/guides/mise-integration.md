# mise Integration

Use [mise](https://mise.jdx.dev) with pitchfork for enhanced development workflows.

## Why Use mise with Pitchfork?

[mise](https://mise.jdx.dev) handles:
- Installing and managing dev tools (Node, Python, etc.)
- Environment variables
- Task dependencies and setup

Pitchfork handles:
- Running background daemons
- Process lifecycle management
- Ready checks and retries

Together, they provide a complete development environment solution.

## Basic Setup

Define a simple daemon that calls a mise task:

**pitchfork.toml:**
```toml
[daemons.docs]
run = "mise run docs:dev"
```

**mise.toml:**
```toml
[env]
NODE_ENV = "development"

[tools]
node = "20"

[tasks."docs:setup"]
run = "npm install"

[tasks."docs:dev"]
run = "node docs/index.js"
depends = ["docs:setup"]
```

## How It Works

1. `pitchfork start docs` launches the daemon
2. Pitchfork calls `mise run docs:dev`
3. mise ensures Node 20 is installed
4. mise runs the `docs:setup` dependency first
5. mise sets `NODE_ENV=development` and starts the server
6. Pitchfork monitors the process and handles restarts

## Example: Full Stack App

**pitchfork.toml:**
```toml
[daemons.api]
run = "mise run api:dev"
auto = ["start", "stop"]
ready_http = "http://localhost:3000/health"

[daemons.frontend]
run = "mise run frontend:dev"
auto = ["start", "stop"]
ready_output = "ready in"
```

**mise.toml:**
```toml
[tools]
node = "20"
python = "3.11"

[env]
DATABASE_URL = "postgres://localhost/myapp"

[tasks."api:setup"]
run = "pip install -r requirements.txt"

[tasks."api:dev"]
run = "uvicorn main:app --reload"
depends = ["api:setup"]

[tasks."frontend:setup"]
run = "npm install"

[tasks."frontend:dev"]
run = "npm run dev"
depends = ["frontend:setup"]
```

## Benefits

- **Tool management:** mise ensures correct tool versions are installed
- **Environment:** mise sets environment variables before the daemon starts
- **Dependencies:** mise runs setup tasks (npm install, etc.) automatically
- **Lifecycle:** pitchfork handles process monitoring, restarts, and ready checks
