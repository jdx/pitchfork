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

The normal `mise run build`, `mise run test`, and `mise run lint` workflows use
[mbx](https://mr-boxington.jdx.dev) for compilation-heavy Cargo work. If mbx
appears to be the problem, first run `mise run build:ui`, then use the
equivalent Cargo commands to unblock yourself without skipping or weakening the
check:

```sh
cargo build
cargo nextest run
git submodule update --init --recursive
mise run test:bats
cargo clippy --manifest-path Cargo.toml --quiet -- -D warnings
```

If Cargo succeeds where mbx fails, or mbx introduces a papercut, please start a
[mr-boxington Discussion](https://github.com/jdx/mr-boxington/discussions).
Include the repository and commit, operating system, `mbx --version`, both
commands and their output, the mbx cache summary, and an `MBX_BYPASS_LOG` when
relevant (for example, `MBX_BYPASS_LOG=mbx-bypasses.log mise run build`).
