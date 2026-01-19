# File Watching

Automatically restart daemons when source files change. This is useful for development workflows where you want hot-reloading behavior.

## Basic Configuration

Add the `watch` field to your daemon configuration with glob patterns:

```toml
[daemons.api]
run = "npm run dev"
watch = ["src/**/*.ts", "package.json"]
```

When any `.ts` file in `src/` or `package.json` changes, the daemon will automatically restart.

## How It Works

1. **On supervisor start**: Pitchfork scans all daemons for `watch` patterns
2. **Directory watching**: Patterns are expanded and their parent directories are watched recursively
3. **File change detection**: The `notify` crate detects file changes with debouncing (1 second)
4. **Pattern matching**: Changed files are matched against glob patterns
5. **Auto-restart**: Running daemons with matching patterns are automatically restarted

::: tip
Only running daemons are restarted. If a daemon is stopped, file changes won't start it.
:::

## Glob Pattern Syntax

Patterns use standard glob syntax:

| Pattern | Matches |
|---------|---------|
| `*.js` | All `.js` files in the daemon's directory |
| `src/**/*.ts` | All `.ts` files in `src/` and subdirectories |
| `package.json` | Specific file |
| `lib/**/*.py` | All `.py` files in `lib/` and subdirectories |
| `config/*.toml` | All `.toml` files in `config/` directory |

Patterns are resolved relative to the `pitchfork.toml` file that defines the daemon.

## Examples

### Node.js Development Server

```toml
[daemons.api]
run = "npm run dev"
watch = ["src/**/*.ts", "src/**/*.tsx", "package.json", "tsconfig.json"]
ready_http = "http://localhost:3000/health"
```

### Python Flask App

```toml
[daemons.flask]
run = "flask run --reload"
watch = ["app/**/*.py", "templates/**/*.html", "requirements.txt"]
ready_port = 5000
```

### Go Service

```toml
[daemons.server]
run = "go run ./cmd/server"
watch = ["**/*.go", "go.mod", "go.sum"]
ready_port = 8080
```

### Multi-Service Setup

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_port = 5432
# No watch - database doesn't need hot reload

[daemons.api]
run = "npm run dev"
depends = ["postgres"]
watch = ["src/**/*.ts", "package.json"]
ready_http = "http://localhost:3000/health"

[daemons.worker]
run = "npm run worker"
depends = ["postgres"]
watch = ["src/worker/**/*.ts", "package.json"]
```

## Combining with Other Features

### With Ready Checks

File watching works well with ready checks to ensure the daemon is fully restarted:

```toml
[daemons.api]
run = "npm run dev"
watch = ["src/**/*.ts"]
ready_http = "http://localhost:3000/health"  # Wait for health endpoint
```

### With Auto Start/Stop

Combine with shell hook for full development workflow automation:

```toml
[daemons.api]
run = "npm run dev"
watch = ["src/**/*.ts"]
auto = ["start", "stop"]  # Auto-start when entering directory
```

### With Retry

If your daemon might fail during restart, add retry:

```toml
[daemons.api]
run = "npm run dev"
watch = ["src/**/*.ts"]
retry = 3  # Retry up to 3 times if restart fails
```

## Performance Considerations

- **Debouncing**: Changes are debounced for 1 second to avoid rapid restarts during batch saves
- **Directory watching**: Only unique parent directories are watched, not individual files
- **Recursive watching**: Subdirectories are watched automatically for `**` patterns
- **Running daemons only**: Stopped daemons ignore file changes

## Troubleshooting

### Files not triggering restart

1. Check the pattern matches your files (patterns are case-sensitive)
2. Ensure the daemon is running (`pitchfork list`)
3. Check supervisor logs for watch registration messages

### Too many restarts

1. Add more specific patterns to avoid matching build artifacts
2. Exclude directories like `node_modules`, `target`, `.git`:
   ```toml
   watch = ["src/**/*.ts"]  # Only watch src/, not node_modules
   ```

### Restart delay

File changes are debounced for 1 second. If you're making rapid edits, only the final state triggers a restart.
