use crate::Result;
use miette::IntoDiagnostic as _;
use tokio::io::AsyncWriteExt as _;

#[derive(Clone, Debug, usage_rs::ValueEnum)]
enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl From<Shell> for usage_rs::complete::Shell {
    fn from(shell: Shell) -> Self {
        match shell {
            Shell::Bash => Self::Bash,
            Shell::Zsh => Self::Zsh,
            Shell::Fish => Self::Fish,
        }
    }
}

/// Generates shell completion scripts
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = "\
Generates shell completion scripts

Creates self-contained tab-completion scripts for your shell.

Supported shells: bash, zsh, fish

Installation:

    bash:
      pitchfork completion bash > ~/.local/share/bash-completion/completions/pitchfork

    zsh:
      pitchfork completion zsh > ~/.zfunc/_pitchfork

    fish:
      pitchfork completion fish > ~/.config/fish/completions/pitchfork.fish"
)]
pub struct Completion {
    /// Shell to generate completions for (bash, zsh, fish)
    #[usage(value_enum)]
    shell: Shell,
}

impl Completion {
    pub async fn run(&self) -> Result<()> {
        let rendered = crate::cli::Cli::completion_script(self.shell.clone().into());
        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(rendered.as_bytes())
            .await
            .into_diagnostic()?;
        stdout.flush().await.into_diagnostic()?;
        Ok(())
    }
}
