# Pitchfork

Pitchfork is a CLI for launching daemons with a focus on developer experience.

## Use-cases

- launching development services like web APIs and databases
- running rsync/unison to synchronize directories with a remote machine

## Features

- only starting daemons if they have not already been started
- starting daemons only when working in a project directory—then automatically stopping when you leave
- [coming soon] automatically start daemons on boot
- [coming soon] restarting daemons on failure

## Workflows

Here's some common ways to use pitchfork.

## Installing pitchfork

[mise-en-place](https://mise.jdx.dev) is the recommended way to install pitchfork.

- mise-en-place – `mise use -g pitchfork`
- cargo – `cargo install pitchfork-cli`
- github - <https://github.com/jdx/pitchfork/releases>

### Launching a one-off daemon

This workflow is an alternative to something like shell jobs—`mytask &`. [`pitchfork run`](/cli/run) just runs a process in
the background:

```bash
pitchfork run docs -- npm start docs-dev-server
```

You need to label the daemon with a name, in this case "docs". Once it's started, "docs" will be how
we reference it. If you run `pitchfork run docs "..."` again, it will not do anything if the daemon
is still running—this way you can start one-off daemons without thinking if you've already done so.

On [`pitchfork run`](/cli/run), pitchfork will emit the output of `npm start docs-dev-server` for a few seconds.
If it fails during that time, it will exit non-zero to help you see if the daemon was configured/setup
correctly. -- TODO this needs to be implemented

### Adding a daemon to a project

A project may have several daemons defined, this is configured in `pitchfork.toml` in the root of the project:

```toml
[daemons.redis]
run = "redis-server"
[daemons.api]
run = "npm run server:api"
[daemons.docs]
run = "npm run server:docs"
```

You can start all the daemons with [`pitchfork start --all`](/cli/start) or individual ones with their name, e.g.: `pitchfork start redis`.
If it's already started, nothing happens.
You can also have pitchfork automatically start the daemons when entering the project in your terminal with the [shell hook](#shell_hook).

### Adding a global daemon that runs on boot

TODO - implement this

## Shell hook

Pitchfork has an optional shell hook for bash, zsh, and fish that will autostart and autostop daemons when entering/leaving projects.

To install it, run the [`pitchfork activate`](/cli/activate) command below for your shell:

```bash
echo '$(pitchfork activate bash)' >> ~/.bashrc
echo '$(pitchfork activate zsh)' >> ~/.zshrc
echo 'pitchfork activate fish | source' >> ~/.config/fish/config.fish
```

Then when you restart your shell pitchfork will automatically start "autostart" daemons when entering the directory. daemons with
"autostop" will stop daemons when leaving the directory after a bit of a delay if no terminal sessions are still inside the directory.

:::tip
You can also have daemons only autostop. You can manually start them with [`pitchfork start`](/cli/start) then they
will be stopped when you leave the directory.
:::

Here's a `pitchfork.toml` with this configured:

```toml
[daemons.api]
run = "npm run server:api"
auto = ["start", "stop"]
```

## Logs

Logs for daemons started with pitchfork can be viewed with [`pitchfork logs`](/cli/logs) or by viewing
the files directly in `~/.local/state/pitchfork/logs`.

```bash
$ pitchfork logs api
[2021-08-01T12:00:00Z] api: starting
[2021-08-01T12:00:01Z] api: listening on
```

You can also view the supervisor logs with `pitchfork logs pitchfork`.

## Supervisor

pitchfork has a supervisor daemon that automatically starts when you run commands like `pitchfork start|run`.
You can manually start/stop it with [`pitchfork supervisor start|stop`](/cli/supervisor/start) or
run a blocking supervisor with [`pitchfork supervisor run`](/cli/supervisor/run).

This watches the daemons status, restarts them if the fail, and a bunch of other things. Ideally
you shouldn't need to really be aware of it. It should just run in the background quietly.

## Configuration

TODO: For now, you'll need to reference [the code](https://github.com/jdx/pitchfork/blob/main/src/env.rs).

## Troubleshooting

You can get extra logs by setting `PITCHFORK_LOG=debug` or `PITCHFORK_LOG=trace` in your environment.
The supervisor will output extra logs but _only_ if this was set when it was started. To ensure the
supervisor is printing debug logs, restart it with this set:

```bash
PITCHFORK_LOG=debug pitchfork supervisor start --force
pitchfork logs pitchfork
```

:::tip
The `--force` is likely needed to kill an existing supervisor process.
:::
