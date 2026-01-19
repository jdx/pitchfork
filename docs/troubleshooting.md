# Troubleshooting

Common issues and how to resolve them.

## Enable Debug Logging

Get detailed logs to diagnose issues:

```bash
PITCHFORK_LOG=debug pitchfork supervisor start --force
pitchfork logs pitchfork
```

::: tip
The `--force` flag is needed to restart the supervisor with new log settings.
:::

For even more detail:

```bash
PITCHFORK_LOG=trace pitchfork supervisor start --force
```

## Common Issues

### Daemon Won't Start

**Symptoms:** `pitchfork start` fails or daemon immediately stops.

**Check:**

1. Verify the command works manually:
   ```bash
   cd /path/to/project
   npm run server  # or whatever your command is
   ```

2. Check daemon logs:
   ```bash
   pitchfork logs myapp
   ```

3. Check supervisor logs:
   ```bash
   pitchfork logs pitchfork
   ```

### Autostop Not Working

**Symptoms:** Daemons don't stop when leaving directory.

**Check:**

1. Verify shell hook is installed:
   ```bash
   # For zsh, check ~/.zshrc contains:
   eval "$(pitchfork activate zsh)"
   ```

2. Verify `auto` includes `"stop"`:
   ```toml
   [daemons.api]
   auto = ["start", "stop"]  # Must include "stop"
   ```

3. Autostop has a delay. Wait a few seconds after leaving.

4. Other terminals in the same directory prevent autostop.

### Supervisor Won't Start

**Symptoms:** Commands hang or fail to connect.

**Check:**

1. Kill any existing supervisor:
   ```bash
   pitchfork supervisor stop
   # Or force kill
   pkill -f "pitchfork supervisor"
   ```

2. Remove stale socket:
   ```bash
   rm ~/.local/state/pitchfork/ipc/main.sock
   ```

3. Start fresh:
   ```bash
   pitchfork supervisor start
   ```

### Port Already in Use

**Symptoms:** Web UI doesn't start, or daemon fails with port conflict.

**For Web UI:**
```bash
# Use a different port
PITCHFORK_WEB_PORT=8888 pitchfork supervisor start --force
```

**For your daemon:** Check what's using the port:
```bash
lsof -i :3000  # Replace 3000 with your port
```

### Ready Check Times Out

**Symptoms:** Daemon starts but pitchfork reports failure.

**Solutions:**

1. Increase the delay:
   ```toml
   [daemons.api]
   ready_delay = 30  # Give more time
   ```

2. Use output pattern instead:
   ```toml
   [daemons.api]
   ready_output = "listening on"  # Wait for specific output
   ```

3. Check your HTTP health endpoint:
   ```bash
   curl http://localhost:3000/health
   ```

### State File Corruption

**Symptoms:** Strange behavior, daemons showing wrong status.

**Fix:**

1. Stop all daemons:
   ```bash
   pitchfork supervisor stop
   ```

2. Remove state file:
   ```bash
   rm ~/.local/state/pitchfork/state.toml
   ```

3. Start fresh:
   ```bash
   pitchfork start --all
   ```

## Getting Help

If you're still stuck:

1. Check the [GitHub Issues](https://github.com/jdx/pitchfork/issues)
2. Open a new issue with:
   - Your pitchfork version (`pitchfork --version`)
   - Your OS
   - Debug logs (`PITCHFORK_LOG=debug`)
   - Your `pitchfork.toml` configuration
