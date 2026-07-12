# Ready Checks

Configure how pitchfork determines when a daemon is ready to accept requests.

## Why Use Ready Checks?

When you run `pitchfork start` or `pitchfork run`, the command waits until the daemon is "ready" before returning. This ensures dependent processes don't start before their dependencies are actually available.

## Delay Check (Default)

Wait a fixed number of seconds after starting. If the daemon is still running, it's considered ready.

**CLI:**
```bash
pitchfork run myapp --delay 5 -- node server.js
pitchfork start myapp --delay 10
```

**Config:**
```toml
[daemons.myapp]
run = "node server.js"
ready_delay = 5  # Wait 5 seconds (CLI default: 3)
```

**Best for:** Simple services where a time delay is sufficient.

## Output Check

Wait until a specific pattern appears in the daemon's output. Uses regular expressions.

**CLI:**
```bash
pitchfork run myapp --output "Server listening" -- node server.js
pitchfork start myapp --output "ready to accept connections"
```

**Config:**
```toml
[daemons.database]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "database system is ready to accept connections"

[daemons.webserver]
run = "python -m http.server 8080"
ready_output = "Serving HTTP on"
```

**Best for:** Services that print a specific message when ready.

## HTTP Check

Wait until an HTTP endpoint returns a 2xx status code, or a configured exact
status code.

**CLI:**
```bash
pitchfork run myapp --http http://localhost:8080/health -- node server.js
pitchfork start myapp --http http://localhost:3000/ready
```

**Config:**
```toml
[daemons.api]
run = "python -m uvicorn main:app"
ready_http = "http://localhost:8000/health"

[daemons.webserver]
run = "node server.js"
ready_http = "http://localhost:3000/ready"

[daemons.private_api]
run = "node server.js"
ready_http = { url = "http://localhost:3000/health", status = [200, 401], timeout = "30s" }
```

**Best for:** Web services with health check endpoints.

::: tip
The HTTP check polls every 500ms with a 5 second timeout per request. The string
form accepts any 2xx response; the object form accepts the exact `status` codes
you list and an optional overall `timeout` for the entire readiness polling period.
:::

## Port Check

Wait until the daemon is listening on a TCP port.

**CLI:**
```bash
pitchfork run myapp --port 8080 -- node server.js
pitchfork start myapp --port 3000
```

**Config:**
```toml
[daemons.api]
run = "node server.js"
ready_port = 3000

[daemons.database]
run = "postgres -D /var/lib/pgsql/data"
ready_port = 5432

[daemons.api_with_timeout]
run = "node server.js"
ready_port = { port = 3000, timeout = "30s" }
```

**Best for:** Services that listen on a known port but don't have a health endpoint.

::: tip
The port check polls every 500ms by attempting a TCP connection to 127.0.0.1:port.
Add a `timeout` to cap how long the check will poll. Without a timeout, the check
remains open until the daemon starts listening.
:::

## Command Check

Wait until a shell command returns exit code 0.

**CLI:**
```bash
pitchfork run myapp --cmd "pg_isready -h localhost" -- node server.js
pitchfork start myapp --cmd "curl -sf http://localhost:3000/health"
```

**Config:**
```toml
[daemons.api]
run = "node server.js"
ready_cmd = "curl -sf http://localhost:3000/health"

[daemons.database]
run = "postgres -D /var/lib/pgsql/data"
ready_cmd = "pg_isready -h localhost"

[daemons.worker]
run = "./start-worker.sh"
ready_cmd = { run = "test -f /tmp/worker.ready", timeout = "60s" }
```

**Best for:** Services that require custom readiness logic or external tools.

::: tip
The command check polls every 500ms. Add a `timeout` to cap how long the check
will poll. Use this when you need more complex readiness checks than the built-in options provide.
:::

## Behaviors

| Check Type | Ready When |
|------------|-----------|
| Delay | Daemon runs for N seconds without crashing |
| Output | Pattern matches stdout/stderr |
| HTTP | Endpoint returns 2xx status, or a configured exact status |
| Port | TCP connection to port succeeds |
| Command | Shell command returns exit code 0 |

- If multiple checks are configured (HTTP, port, command), the first one to succeed marks the daemon as ready
- **Delay check** only fires when no other check type (`ready_output`, `ready_http`, `ready_port`, `ready_cmd`) is configured. It acts as the fallback default.
- If the daemon exits with a non-zero code before becoming ready, `pitchfork start/run` exits with that same code
- A timed `ready_http`, `ready_port`, or `ready_cmd` stops polling when its deadline is reached. Startup fails only when every configured check has reached its deadline; any unbounded check keeps startup open. When startup fails because all checks are exhausted, pitchfork exits with code `124`, kills the daemon, and applies normal retry and dependency behavior.

## Common Patterns

**PostgreSQL:**
```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "database system is ready to accept connections"
```

**Redis:**
```toml
[daemons.redis]
run = "redis-server"
ready_output = "Ready to accept connections"
```

**Node.js:**
```toml
[daemons.api]
run = "npm run start"
ready_http = "http://localhost:3000/health"
```

**Python FastAPI:**
```toml
[daemons.api]
run = "uvicorn main:app"
ready_http = "http://localhost:8000/health"
```

**PostgreSQL (using pg_isready):**
```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_cmd = "pg_isready -h localhost"
```

**Redis (using redis-cli):**
```toml
[daemons.redis]
run = "redis-server"
ready_cmd = "redis-cli ping"
```

**File-based readiness:**
```toml
[daemons.worker]
run = "./start-worker.sh"
ready_cmd = "test -f /tmp/worker.ready"
```
