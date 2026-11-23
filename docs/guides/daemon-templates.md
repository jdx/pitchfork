---
description: Define a set of daemons once and run several numbered or named copies of it, each wired to its own dependencies.
---
# Daemon templates

A daemon template defines a set of daemons once. Pitchfork runs one copy of
the set for each of the template's instances. Use a template to run several
copies of a stack side by side, such as one backend per tenant or several
workers, each with its own API server.

## Define a template

Put the daemons under `[daemon_templates.<template>.daemons]` and set
`instances`:

```toml
[daemons.redis]
run = "redis-server --port 6379"

[daemon_templates.workers]
instances = 2

[daemon_templates.workers.daemons.api]
run = "./api --port $PORT"
port = { expect = [3000], bump = 10 }

[daemon_templates.workers.daemons.worker]
run = "./worker --api http://127.0.0.1:{{ daemons.api.port }} --id {{ instance }}"
depends = ["api", "redis"]
```

This config defines these daemons and groups:

| Name | Kind | Contains |
| --- | --- | --- |
| `redis` | daemon | |
| `workers.0.api`, `workers.0.worker` | daemons | instance `0` |
| `workers.1.api`, `workers.1.worker` | daemons | instance `1` |
| `workers.0` | group | `workers.0.api`, `workers.0.worker` |
| `workers.1` | group | `workers.1.api`, `workers.1.worker` |
| `workers` | group | the daemons of every instance |

The copies are ordinary daemons: `start`, `stop`, `logs`, `status`, the TUI
and the web UI all work with them.

```sh
pitchfork start --group workers.1   # one instance
pitchfork start --group workers     # every instance
pitchfork logs workers.0.worker
```

The template itself is not a daemon. A template without `instances`, or with
an empty list, adds nothing.

## Instances

`instances` is either a count or a list of names:

```toml
[daemon_templates.workers]
instances = 3                     # instances 0, 1 and 2

[daemon_templates.tenants]
instances = ["acme", "globex"]    # instances acme and globex
```

Template, instance and template daemon names may use letters, digits, `_` and
`-`, and may not start or end with `-` or contain `--`. Config parsing fails
on other names and on an instance listed twice.

## Dependencies within an instance

In a copy's `depends`, the name of another daemon of the same template means
that daemon's copy in the same instance. In the example, `workers.1.worker`
depends on `workers.1.api` and on the regular `redis` daemon. A template
daemon's name takes precedence over a regular daemon with the same name.

Templates in a copy use the same names, so <code v-pre>{{ daemons.api.port }}</code>
in `workers.1.worker` is the port of `workers.1.api`. Once they have started,
the other daemons of the instance are available by their template names too,
including those the copy only depends on indirectly. A copy is also
available under its full name, such as
<code v-pre>{{ daemons["workers.1.api"].port }}</code>. An `as` name in
`depends` that points to another template daemon also points to the copy in
the same instance.

## Template variables

Fields that accept [templates](/guides/configuration-templates) can use two
more variables in a copy:

| Variable | Value | Example |
| --- | --- | --- |
| <code v-pre>{{ instance }}</code> | Instance name | `"1"`, `"acme"` |
| <code v-pre>{{ instance_index }}</code> | Position in `instances`, from 0 | `1` |

Both are null in a daemon that is not a copy, so
<code v-pre>{% if instance %}</code> works in any daemon.

## Ports and local URLs

Each copy is configured with the same ports. Give a template daemon that listens on a port
a `bump` range, as in the example, so each copy starts on the next free port.
<code v-pre>{{ daemons.api.port }}</code> and `$PORT` have the port the copy
started on. See [port management](/guides/port-management) for `bump`.

The reverse proxy names each copy from its full name, so `workers.1.api`
gets a hostname such as `workers-1-api.myproj.localhost`. Do not set
`proxy.label` in a template daemon: every copy would ask for the same label.

## Configuration files

Copies are made after all config files are merged:

- A later file of the same project can define a template with the same name. Its
  `instances` replaces the earlier value, and its daemons are added to the
  template or replace those with the same name. For example, a
  `pitchfork.local.toml` with `instances = ["mine"]` runs a single copy for you
  only.
- A template from another project's config replaces the template.
- A copy or group whose name is already defined, such as a regular daemon named
  `workers.0.api` or a group named `workers`, is a config error.

Commands that edit config files, such as `pitchfork daemons add`, keep
`[daemon_templates]` as written and never write copies to the file.

Instances are fixed in the config: there is no command to add an instance
while pitchfork runs.
