# Installation

## Install Pitchfork

### mise (Recommended)

[mise-en-place](https://mise.jdx.dev) is the recommended way to install pitchfork:

```bash
mise use -g pitchfork
```

### Cargo

Install from crates.io:

```bash
cargo install pitchfork-cli
```

### GitHub Releases

Download pre-built binaries from [GitHub Releases](https://github.com/jdx/pitchfork/releases).

## Verify Installation

```bash
pitchfork --version
```

## Shell Completion

Pitchfork supports tab completion for bash, zsh, and fish.

::: tip
Shell completion requires the [`usage`](https://usage.jdx.dev) CLI tool to be installed.
:::

::: code-group

```bash [bash]
mkdir -p ~/.local/share/bash-completion/completions
pitchfork completion bash > ~/.local/share/bash-completion/completions/pitchfork
```

```bash [zsh]
mkdir -p ~/.zfunc
pitchfork completion zsh > ~/.zfunc/_pitchfork
# Add to ~/.zshrc: fpath=(~/.zfunc $fpath)
```

```bash [fish]
pitchfork completion fish > ~/.config/fish/completions/pitchfork.fish
```

:::

## Shell Alias (Optional)

For a shorter command, add a `pf` alias to your shell. Combined with the
implicit `start` shorthand, you can then start a daemon with just `pf api`:

::: code-group

```bash [bash]
echo 'alias pf=pitchfork' >> ~/.bashrc
```

```bash [zsh]
echo 'alias pf=pitchfork' >> ~/.zshrc
```

```bash [fish]
echo 'alias pf pitchfork' >> ~/.config/fish/config.fish
```

:::

Restart your shell or `source` the file, then `pf api` runs `pitchfork start api`.

## What's Next?

- [Quick Start](/quickstart) - Run your first daemon
- [Your First Project](/first-daemon) - Set up a project with multiple daemons
