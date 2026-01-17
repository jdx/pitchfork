# Ready Checks

Pitchfork now supports readiness checks to determine when a daemon has successfully started and is ready to accept requests. This is useful when you want `pitchfork start` or `pitchfork run` to wait until the daemon is actually ready before returning.

## Features

### Delay Check (Default)

By default, pitchfork waits 3 seconds after starting a daemon. If the daemon is still running after this delay, it's considered ready and `pitchfork start`/`pitchfork run` exits successfully.

```bash
pitchfork run myapp --delay 5 -- node server.js
pitchfork start myapp --delay 10
```

```toml
[daemons.myapp]
run = "node server.js"
ready_delay = 5  # Wait 5 seconds
```

### Output Check

Wait until a specific pattern appears in the daemon's output (stdout or stderr). The pattern is a regular expression.

```bash
pitchfork run myapp --output "Server listening on port" -- node server.js
pitchfork start myapp --output "ready to accept connections"
```

```toml
[daemons.database]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "database system is ready to accept connections"

[daemons.webserver]
run = "python -m http.server 8080"
ready_output = "Serving HTTP on"
```

### HTTP Check

Wait until an HTTP endpoint returns a 2xx status code. This is useful for web services that have a health check endpoint.

```bash
pitchfork run myapp --http http://localhost:8080/health -- node server.js
pitchfork start myapp --http http://localhost:3000/ready
```

```toml
[daemons.api]
run = "python -m uvicorn main:app"
ready_http = "http://localhost:8000/health"

[daemons.webserver]
run = "node server.js"
ready_http = "http://localhost:3000/ready"
```

The HTTP check polls the endpoint every 500ms with a 5 second timeout per request until it receives a successful response (2xx status code).

## Behaviors

- **Delay check**: Daemon runs for the specified delay period without failing → `pitchfork start`/`pitchfork run` exits with code 0
- **Output check**: Daemon output matches the pattern → `pitchfork start`/`pitchfork run` exits with code 0
- **HTTP check**: HTTP endpoint returns 2xx status → `pitchfork start`/`pitchfork run` exits with code 0
- If multiple checks are specified, the first one to succeed marks the daemon as ready
- Daemon fails (exits with non-zero code) before becoming ready → `pitchfork start`/`pitchfork run` exits with the same exit code as the daemon




