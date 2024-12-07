## Pitchfork

Daemons with DX

![pitchfork logo](/logo.png)

Pitchfork is a CLI for launching daemons with a focus on developer experience.

> [!CAUTION]
> This is a WIP. Most of this is not yet implemented. I will remove this message when you should try it out.

## Use-cases

- launching development services like web APIs and databases
- running rsync/unison to synchronize directories with a remote machine
- cross-platform systemd alternative that requires far less boiler-plate

## Features

- automatically start daemons on boot
- only starting daemons if they have not already been started
- restarting daemons on failure
- starting daemons only when working in a project directory—then automatically stopping when you leave

## Workflows

Here's some common ways to use pitchfork.

### Launching a one-off daemon

This workflow is an alternative to something like shell jobs—`mytask &`. This just runs a process in
the background:

```bash
pitchfork run docs "npm start docs-dev-server"
```

You need to label the daemon with a name, in this case "docs". Once it's started, "docs" will be how
we reference it. If you run `pitchfork run docs "..."` again, it will not do anything if the daemon
is still running—this way you can start one-off daemons without thinking if you've already done so.

On `pitchfork run`, pitchfork will emit the output of `npm start docs-dev-server` for a few seconds.
If it fails during that time, it will exit non-zero to help you see if the daemon was configured/setup
correctly.

### Adding a daemon to a project

A project may have several daemons defined, this is configured in `pitchfork.toml` in the root of the project:

```toml
[daemons]
redis = "redis-server"
api = "npm run server:api"
docs = "npm run server:docs"
```

You can start all the daemons with `pitchfork start --all` or individual ones with their name, e.g.: `pitchfork start redis`.
If it's already started, nothing happens.
You can also have pitchfork automatically start the daemons when entering the project in your terminal with the [shell hook](#shell_hook).

### Adding a global daemon that runs on boot

TODO

## Shell hook

Pitchfork has an optional shell hook for bash, zsh, and fish that will autostart and autostop daemons when entering/leaving projects.

To install it, run the command below for your shell:

```bash
echo '$(pitchfork activate bash)' >> ~/.bashrc
echo '$(pitchfork activate zsh)' >> ~/.zshrc
echo 'pitchfork activate fish | source' >> ~/.config/fish/config.fish
```

Then when you restart your shell pitchfork will automatically start "autostart" daemons when entering the directory. daemons with
"autostop" will stop daemons when leaving the directory after a bit of a delay if no terminal sessions are still inside the directory.

Here's a `pitchfork.toml` with this configured:

```toml
[daemons.api]
run = "npm run server:api"
autostart = true
autostop = true
```

## Integration with mise

[mise](https://mise.jdx.dev) is a project for installing/managing dev tools, managing environment variables,
and running tasks. Unlike pitchfork, [mise tasks](https://mise.jdx.dev/tasks/) do not run in the background however
they offer a lot of functionality you won't find in pitchfork. It's encouraged to define relatively simple daemons
that just call `mise run` to launch the daemon as a mise task.

To do so, put the following into `pitchfork.toml`:

```toml
[daemons.docs]
run = "mise run docs:dev"
```

And in `mise.toml` you can define how `mise run docs:dev` gets setup and behaves:

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
