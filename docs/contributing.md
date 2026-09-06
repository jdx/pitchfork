---
description: Set up pitchfork for development, run repository checks, and edit or regenerate the documentation.
---
# Contributing

Pitchfork welcomes focused fixes and improvements. For a non-obvious change,
discuss the direction first in [GitHub Discussions](https://github.com/jdx/pitchfork/discussions)
or [Discord](https://discord.gg/UBa7pJUN7Z). The project has a specific scope;
settling the direction early avoids work on a change that will not be accepted.

Before requesting review, CI must pass and automated review comments must be
addressed. Maintainer review time is limited, and changes may be declined
briefly when they do not fit the project's scope or quality expectations.

## Set up a checkout

```sh
git clone --recurse-submodules https://github.com/jdx/pitchfork.git
cd pitchfork
mise install
mise run build
```

The build task builds the embedded web UI before the Rust binary. Use the task
instead of a bare `cargo build` for normal development.

## Develop and verify

| Task | Command |
| --- | --- |
| Build the UI and CLI | `mise run build` |
| Rebuild and restart your local supervisor | `mise run build-dev` |
| Check formatting and lints | `mise run lint` |
| Apply formatting and lint fixes | `mise run lint-fix` |
| Run Rust and shell integration tests | `mise run test` |
| Run one Rust test | `cargo nextest run test_name` |
| Run web UI browser tests | `mise run test:web-ui` |
| Run the development checks before committing | `mise run ci-dev` |

`build-dev` restarts the supervisor used by your local pitchfork installation.
Use it when you intend to run your development build. `ci-dev` builds, fixes
lints, builds docs, runs tests, and regenerates references; inspect its changes
before committing.

## Work on the docs

```sh
mise run docs
```

Open the local URL printed by VitePress. Markdown lives in `docs/`, navigation
in `docs/.vitepress/config.mts`, and theme components and styles in
`docs/.vitepress/theme/`.

For prose and styling changes:

```sh
mise run build:docs
```

The production build checks examples and internal links, including anchors,
and generates and verifies social preview images. Preview landing and article
pages at mobile and desktop widths and in both themes after styling changes.

### Edit the source of generated content

| Change | Source | Regenerate with |
| --- | --- | --- |
| Command help, flags, arguments | `src/cli/` usage-rs definitions | `mise run render` |
| Settings documentation and defaults | `src/settings.rs` | `mise run render` |
| TOML schema | Config types and schemars definitions | `mise run render` |
| HTTP response schema | API types | `mise run render` |

Do not hand-edit `docs/cli/`, `docs/public/schema.json`,
`docs/public/api-schema.json`, or `pitchfork.usage.kdl`. The render task stages
generated files and the docs directory, so inspect both staged and unstaged
changes afterward. Build the docs again after rendering.

Keep tutorials runnable, label prerequisites, and distinguish complete config
examples from fields to add to an existing table. Link to the canonical guide
instead of repeating long explanations across pages.

## If the mbx build cache fails

Compilation-heavy tasks use [mbx](https://mr-boxington.jdx.dev) through the
repository's Cargo wrapper. If the cache command fails, build the UI first,
then run the equivalent Cargo command without the wrapper. See
[CONTRIBUTING.md](https://github.com/jdx/pitchfork/blob/main/CONTRIBUTING.md#mbx-build-cache)
for the fallback commands and the information needed to report a mismatch.

## Pull requests

Use a Conventional Commit title that starts with a lowercase description:

- `fix(supervisor): handle a missing process`
- `docs: clarify project setup`
- `chore(deps): update dependencies`

Explain the problem, the resulting behavior, and how you verified the change.
Use `fix` for application bugs and `chore` or `ci` for infrastructure changes.
See [AGENTS.md](https://github.com/jdx/pitchfork/blob/main/AGENTS.md) for repository
conventions, including disclosure for AI-assisted GitHub content.
