# Start on Boot

Configure pitchfork to start automatically when your system boots.

## Enable Boot Start

```bash
pitchfork boot enable
```

This registers pitchfork to start automatically when you log in.

## Disable Boot Start

```bash
pitchfork boot disable
```

## Check Status

```bash
pitchfork boot status
```

## Configure Boot Daemons

Add `boot_start = true` to daemons you want to start at boot. These should be in your global config file (`~/.config/pitchfork/config.toml`):

```toml
[daemons.postgres]
run = "postgres -D /usr/local/var/postgres"
boot_start = true

[daemons.redis]
run = "redis-server"
boot_start = true

[daemons.my-app]
run = "npm start"
boot_start = false  # Won't start at boot
```

## How It Works

| Platform | Method |
|----------|--------|
| macOS | LaunchAgents |
| Linux | systemd user services |
| Windows | Registry entries |

When boot start is enabled:
1. System login triggers the pitchfork supervisor
2. Supervisor starts all daemons with `boot_start = true`
3. Daemons run in the background

## Typical Setup

1. Enable boot start:
   ```bash
   pitchfork boot enable
   ```

2. Add daemons to global config (`~/.config/pitchfork/config.toml`):
   ```toml
   [daemons.postgres]
   run = "postgres -D /usr/local/var/postgres"
   boot_start = true
   ready_output = "ready to accept connections"
   ```

3. Verify it's working:
   ```bash
   pitchfork boot status
   pitchfork list
   ```
