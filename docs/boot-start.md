# Start on Boot

Pitchfork can automatically start the supervisor and configured daemons when your system boots, powered by [auto_launch](https://github.com/gaojunran/auto-launch).

## Enable Start on Boot

To enable start on boot functionality, run:

```bash
pitchfork boot enable
```

This will configure your system to automatically start the pitchfork supervisor when you log in.

## Disable Start on Boot

To disable start on boot functionality, run:

```bash
pitchfork boot disable
```

## Check Boot Start Status

To check if boot start is currently enabled:

```bash
pitchfork boot status
```

## Configuring Daemons for Boot Start

To configure specific daemons to start at boot, add `boot_start = true` to their configuration in your global `pitchfork.toml` file:

```toml
[daemons.postgres]
run = "postgres -D /usr/local/var/postgres"
boot_start = true

[daemons.redis]
run = "redis-server"
boot_start = true

[daemons.my-app]
run = "npm start"
boot_start = false  # This daemon won't start at boot
```

## How it works

The Rust Library [auto_launch](https://crates.io/crates/auto-launch) is not maintained actively, so pitchfork includes a [fork](https://github.com/gaojunran/auto-launch) of the library with some fixes and improvements. 

For macOS, it uses `LaunchAgents` to start the supervisor at login. For Linux, it uses `systemd` user services. For Windows, it adds registry entries. Feel free if you need more methods to register pitchfork on your specific platform, just open an issue or a PR at [our forked library](https://github.com/gaojunran/auto-launch)! 😎
