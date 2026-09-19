# Contributing

See the [contributing guide](https://pitchfork.jdx.dev/contributing) for review
expectations and repository conventions.

## Development setup

```sh
git submodule update --init --recursive
mise install
mise run build
```

`build` prepares the embedded web UI before compiling Rust. Use
`mise run build-dev` when you also want to restart your local supervisor.
Run **`mise run ci-dev` before committing** and inspect the generated changes.

## Documentation

```sh
mise run docs        # Local VitePress server
mise run build:docs  # Production build and documentation checks
```

Edit prose in `docs/`, navigation in `docs/.vitepress/config.mts`, and styling in
`docs/.vitepress/theme/`. For generated CLI help and settings, edit `src/cli/`
or `src/settings.rs` and run `mise run render`; do not hand-edit `docs/cli/`.
The render task stages the docs directory and generated usage spec. Build the
docs again after rendering and review staged changes before committing.

## mbx build cache

mise wraps `cargo` with [mbx](https://mr-boxington.jdx.dev), so compiled work
is shared across checkouts. `mise run` tasks and `mise exec -- cargo …` always
use the wrapper; plain `cargo` does too once mise is activated in your shell
(`mise activate`).
