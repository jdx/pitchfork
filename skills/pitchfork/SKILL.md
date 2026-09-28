---
name: pitchfork
description: Configure and operate project daemons with pitchfork, including readiness, dependencies, ports, logs, and mise-managed environments. Use for pitchfork.toml and pitchfork CLI workflows.
---

# Pitchfork

Work from the intended project directory. Check `pitchfork --version` and
`pitchfork <command> --help` for flags supported by the installed release.

## Find the right daemon

```sh
pitchfork config list --json
pitchfork list --project --json
pitchfork status api --json
pitchfork logs api -n 100 --no-pager
```

System and user configs merge first, followed by project directories from root
to the current directory. Within each directory, precedence is
`.config/pitchfork.toml` < `.config/pitchfork.local.toml` < `pitchfork.toml` <
`pitchfork.local.toml`. Inspect existing definitions before adding another.

Short daemon names resolve in the current namespace. Use `namespace/daemon` when
targeting another project or resolving ambiguity. Prefer explicit daemon names
or an existing group for a bounded operation. `--local` includes inherited parent
configs; `--global` selects global services. `start --all` selects local and global
configured daemons, while `stop --all` and `restart --all` select every running or
waiting daemon, including unrelated projects.

## Define foreground services

Adapt the command and readiness check to the actual application:

```toml
#:schema https://pitchfork.jdx.dev/schema.json

[daemons.api]
run = "node server.js"
mise = true
port = 3000
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
retry = 3
```

This example assumes `server.js` listens on `PORT` and serves `/health`.
Commands run from the config's project directory; set `dir` for a subdirectory.
Keep the service in the foreground: avoid `&`, daemonizing flags, and
`docker run -d`, which detach the process Pitchfork must supervise.

Use `mise = true` when the daemon needs the project's mise tools or environment.
The supervisor does not inherit later changes to an interactive shell. A literal
`run = "mise run api:dev"` is also supported, but then `mise` itself must be on
the supervisor's PATH or addressed by absolute path.

## Readiness, dependencies, and ports

- `depends = ["database"]` starts dependencies and waits for their readiness
  before launching the dependent service. Reference resolved dependency ports
  with templates such as `{{ daemons.database.port }}`.
- Prefer an application check with an explicit timeout. `ready_http`,
  `ready_cmd`, `ready_port`, and `ready_output` are alternatives: if several are
  configured, the first successful check wins. Combine conditions in one
  `ready_cmd` when all must pass. `ready_delay` is only a fallback.
- `ready_port` (and `start --port`) probes a port; it does not assign one.
  `port` (and `--expected-port`) assigns the expected port and exposes `$PORT`.
  The application must actually use it.
- For bumped ports, use a probe that follows the resolved value:

  ```toml
  port = { expect = [3000], bump = 10 }
  ready_cmd = { run = "curl -fsS http://127.0.0.1:$PORT/health", timeout = "30s" }
  ```

- Use `oneshot = true` for setup commands that must finish successfully, such as
  migrations. Oneshots cannot have `ready_*` or `health_*` fields.
- Readiness gates startup. `health_*` checks monitor an already running service;
  their failed-probe count and the daemon's `retry` policy are separate settings.

## Apply and diagnose

```sh
pitchfork start api
pitchfork status api --json
pitchfork logs api -n 100 --no-pager
pitchfork restart api
pitchfork stop api
```

`start` waits for readiness and leaves an already running daemon alone.
Use `restart` to apply changed commands or environment; already running
dependencies stay up. Check status and recent logs after a failed start before
repeating it. Bound retries and fix the failed command, port conflict, or probe
instead of repeatedly restarting the supervisor.

`auto = ["start", "stop"]` needs shell integration or tracked project sessions;
it is not a traffic-based idle policy. Do not enable shell hooks, login startup,
proxy DNS, certificate trust, or LAN exposure as a side effect of configuring a
single daemon. When those features are requested, use the relevant guides:
[shell sessions](https://pitchfork.jdx.dev/guides/shell-hook),
[boot startup](https://pitchfork.jdx.dev/guides/boot-start), and
[ports and proxy](https://pitchfork.jdx.dev/guides/port-management).
