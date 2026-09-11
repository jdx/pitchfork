# External configuration files

Attach generated configuration to a project without writing a `pitchfork.toml`
into its repository:

```sh
pitchfork config add /path/to/generated.toml --dir /path/to/project
pitchfork config list --json
pitchfork config remove /path/to/generated.toml
```

The file must exist and contain valid pitchfork configuration when added. Paths
inside it, including daemon `dir` and watch patterns, resolve against the project.
The attachment applies in the project and its descendants, not unrelated directories.

Registration is stored in the user global configuration:

```toml
[namespaces.myproject]
dir = "/path/to/project"
config = ["/path/to/generated.toml"]
```

Relative attachment paths resolve against `dir`; `~` is supported. An attachment
inherits the project's explicit namespace or its directory name. Use
`--namespace` when registering a project without its own configuration. A namespace
cannot own attachments for different directories, and one file cannot be attached
to multiple projects. Removing a missing file is supported; removal preserves the
namespace registration and never deletes the file or stops an already running daemon.

Registered files override ordinary project files. Ancestor attachments load before
child attachments; files within an entry load in their listed order. Existing
supervisors discover attachments for cron, hooks, project sessions, watches, and
proxy auto-start. Regenerate files atomically so readers never see partial TOML.
`pitchfork daemons add/remove` continues to edit ordinary project files.

## Invocation-scoped configuration

```sh
PITCHFORK_CONFIG=/path/to/generated.toml pitchfork daemons
```

`PITCHFORK_CONFIG` accepts a platform path list (`:` on Unix, `;` on Windows).
Files are associated with the invocation's current directory and override registered
attachments. This does not register anything and is not inherited by an automatically
started supervisor. Commands can consume these definitions immediately, but subsequent
supervisor-side discovery requires `pitchfork config add`.
