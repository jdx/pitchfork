# Configuration Templates

Use [Tera](https://keats.github.io/tera/) templates in configuration fields to reference values from other daemons, settings, and runtime state.

## Why Templates?

When daemons depend on each other, you often need to pass connection details between them. Without templates, you have to hardcode ports and URLs:

```toml
[daemons.redis]
run = "redis-server"
port = 6379

[daemons.api]
run = "server --port 3000 --redis-port 6379"  # hardcoded!
env = { DATABASE_URL = "redis://localhost:6379/0" }  # hardcoded!
depends = ["redis"]
```

This breaks when `redis` uses `port = { expect = [6379], bump = 10 }` and the port gets auto-bumped. Templates solve this by resolving values at start time.

## Basic Usage

Any field that accepts a string can use <code v-pre>{{ ... }}</code> template expressions:

```toml
[daemons.redis]
run = "redis-server"
port = 6379

[daemons.api]
run = "server --redis-port {{ daemons.redis.port }}"
env = { DATABASE_URL = "redis://localhost:{{ daemons.redis.port }}/0" }
depends = ["redis"]
```

Template rendering follows the dependency order: daemons in later levels can reference values from daemons that completed successfully in earlier levels. If a daemon has `depends = ["redis"]`, it starts after `redis` and can use <code v-pre>{{ daemons.redis.port }}</code>.

## Template Fields

Templates work in these configuration fields:

| Field | Example |
|-------|---------|
| `run` | <code v-pre>run = "server --port {{ daemons.redis.port }}"</code> |
| `env` values | <code v-pre>env = { DB_URL = "postgres://localhost:{{ daemons.db.port }}" }</code> |
| `hooks.*` commands | <code v-pre>on_ready = "curl http://localhost:{{ daemons.api.port }}/health"</code> |
| `ready_cmd` | <code v-pre>ready_cmd = "curl http://localhost:{{ daemons.api.port }}/health"</code> |

## Template Variables

### Self Variables

The current daemon's own metadata is always available:

| Variable | Description | Example |
|----------|-------------|---------|
| <code v-pre>{{ name }}</code> | Daemon short name | `"api"` |
| <code v-pre>{{ namespace }}</code> | Daemon namespace | `"myproj"` |
| <code v-pre>{{ id }}</code> | Qualified ID | `"myproj/api"` |
| <code v-pre>{{ slug }}</code> | Proxy slug alias (or null) | `"myapi"` |
| <code v-pre>{{ dir }}</code> | Resolved working directory | `"/home/user/myproj"` |

### Daemon References

Reference same-namespace daemons by their short name:

| Variable | Description | Example |
|----------|-------------|---------|
| <code v-pre>{{ daemons.redis.port }}</code> | First resolved port | `6379` |
| <code v-pre>{{ daemons.redis.ports }}</code> | All resolved ports | `[6379, 6380]` |
| <code v-pre>{{ daemons.redis.ports[0] }}</code> | Port by index | `6379` |
| <code v-pre>{{ daemons.redis.id }}</code> | Qualified ID | `"myproj/redis"` |
| <code v-pre>{{ daemons.redis.name }}</code> | Short name | `"redis"` |
| <code v-pre>{{ daemons.redis.namespace }}</code> | Namespace | `"myproj"` |
| <code v-pre>{{ daemons.redis.slug }}</code> | Slug alias | `"myredis"` |
| <code v-pre>{{ daemons.redis.dir }}</code> | Working directory | `"/home/user/myproj"` |

::: tip
`port` is shorthand for `ports[0]`. Use `ports[N]` when a daemon has multiple ports configured.
:::

### Cross-Namespace References

When referencing daemons in a different namespace, use the `namespace.name` key format:

```toml
{{ daemons["infra.redis"].port }}
```

This mirrors the `depends` field, which also supports cross-namespace references like `depends = ["infra/redis"]`.

### Settings

Global proxy settings are available:

| Variable | Description |
|----------|-------------|
| <code v-pre>{{ settings.proxy.enable }}</code> | Whether the proxy is enabled |
| <code v-pre>{{ settings.proxy.tld }}</code> | Proxy TLD (default: `"localhost"`) |
| <code v-pre>{{ settings.proxy.port }}</code> | Proxy port (default: `443`) |
| <code v-pre>{{ settings.proxy.https }}</code> | Whether HTTPS is enabled |

### Proxy URL

<code v-pre>{{ proxy_url }}</code> provides the full proxy URL for the current daemon when it has a registered slug:

```toml
[daemons.api]
run = "echo {{ proxy_url }}"
# Renders to: "echo https://myapi.localhost"
```

## Resolution Order

Templates are rendered level-by-level following the dependency graph:

```
Level 0: redis    (no dependencies, starts first)
Level 1: api      (depends on redis, templates can reference redis)
Level 2: worker   (depends on api, templates can reference redis and api)
```

- Daemons within the same level start concurrently and **cannot** reference each other's ports
- A daemon can reference any daemon that completed successfully in a previous level
- `port` and `ports` for the current daemon are **not** available at template rendering time (ports are resolved after the command is constructed). Use the `$PORT` / `$PORT0`, `$PORT1`, ... environment variables to access the current daemon's own resolved ports. Use <code v-pre>{{ daemons.xxx.port }}</code> to reference **dependencies'** ports instead

## Error Handling

- Undefined template variables produce clear errors at start time (strict mode)
- A template error in `run` or `env` prevents the daemon from starting, and its dependents are also skipped
- A template error in a hook command logs a warning and skips the hook execution

## Examples

### Database Connection String

```toml
[daemons.postgres]
run = "postgres -D /usr/local/var/postgres"
port = 5432

[daemons.api]
run = "node server.js"
env = { DATABASE_URL = "postgres://localhost:{{ daemons.postgres.port }}/myapp" }
depends = ["postgres"]
```

### Multiple Services

```toml
[daemons.redis]
run = "redis-server"
port = 6379

[daemons.postgres]
run = "postgres"
port = 5432

[daemons.api]
run = "server start"
env = { REDIS_URL = "redis://localhost:{{ daemons.redis.port }}", DATABASE_URL = "postgres://localhost:{{ daemons.postgres.port }}/app" }
depends = ["redis", "postgres"]
```

### Multiple Ports

```toml
[daemons.grpc]
run = "grpc-server"
port = [50051, 50052]

[daemons.gateway]
run = "gateway --grpc-port {{ daemons.grpc.ports[0] }} --metrics-port {{ daemons.grpc.ports[1] }}"
depends = ["grpc"]
```

### Auto-Bumped Port

When `bump` is configured, the resolved port may differ from the expected port. Templates always use the resolved value:

```toml
[daemons.redis]
run = "redis-server"
port = { expect = [6379], bump = 10 }
# If 6379 is occupied, redis starts on 6380, 6390, etc.

[daemons.api]
run = "server --redis-port {{ daemons.redis.port }}"
# Always uses the actual port, regardless of bumping
depends = ["redis"]
```

### Hook with Port Reference

```toml
[daemons.api]
run = "node server.js"
port = 3000
ready_http = "http://localhost:3000/health"

[daemons.monitor]
run = "echo monitoring"
depends = ["api"]

[daemons.monitor.hooks]
on_ready = "curl -X POST https://monitor.example.com/register -d '{\"url\": \"http://localhost:{{ daemons.api.port }}\"}'"
```
