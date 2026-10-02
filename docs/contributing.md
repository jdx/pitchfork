---
description: Set up pitchfork for development, run repository checks, and edit or regenerate the documentation.
---
# Contributing

::: danger AI replies to Discussions and Issues are restricted
You may only use AI to reply to a [Discussion](https://github.com/jdx/pitchfork/discussions) or [Issue](https://github.com/jdx/pitchfork/issues) if you
created it, you opened a PR that fixes it, or you have already had a contribution, attributed to your GitHub account, merged into the default branch of pitchfork. Everyone else is not allowed to use AI to reply. This is a growing problem, and **doing it is
an instant ban across all of jdx's projects.**

This includes raw, lightly edited, reviewed, and disclosed model output. Adding an "AI-assisted"
footer does not make an AI reply acceptable on its own. If you are running an agent, make sure it
does not post to threads you are not allowed to reply to, and never let it sweep through many
threads at once.
:::

Using AI to help write and file your own Discussion or Issue is fine. Review it before posting, and
disclose that AI contributed. If you are allowed to use AI to reply, review and verify the reply before
posting it, and disclose that AI contributed.

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

## Pull requests

Use a Conventional Commit title that starts with a lowercase description:

- `fix(supervisor): handle a missing process`
- `docs: clarify project setup`
- `chore(deps): update dependencies`

Explain the problem, the resulting behavior, and how you verified the change.
Use `fix` for application bugs and `chore` or `ci` for infrastructure changes.
See [AGENTS.md](https://github.com/jdx/pitchfork/blob/main/AGENTS.md) for repository
conventions, including disclosure for AI-assisted GitHub content.
