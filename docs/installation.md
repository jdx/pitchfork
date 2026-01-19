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

### Bash

```bash
mkdir -p ~/.local/share/bash-completion/completions
pitchfork completion bash > ~/.local/share/bash-completion/completions/pitchfork
```

### Zsh

```bash
mkdir -p ~/.zfunc
pitchfork completion zsh > ~/.zfunc/_pitchfork
# Add to ~/.zshrc: fpath=(~/.zfunc $fpath)
```

### Fish

```bash
pitchfork completion fish > ~/.config/fish/completions/pitchfork.fish
```

## What's Next?

- [Quick Start](/quickstart) - Run your first daemon
- [Your First Project](/first-daemon) - Set up a project with multiple daemons
