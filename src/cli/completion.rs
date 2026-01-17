use crate::Result;
use duct::cmd;
use miette::IntoDiagnostic;

/// Generates shell completion scripts
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    long_about = "\
Generates shell completion scripts

Creates tab-completion scripts for your shell. Requires the 'usage' CLI tool.

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
    #[clap()]
    shell: String,
}

impl Completion {
    pub async fn run(&self) -> Result<()> {
        cmd!(
            "usage",
            "g",
            "completion",
            &self.shell,
            "pitchfork",
            "--usage-cmd",
            "pitchfork usage",
        )
        .run()
        .into_diagnostic()?;
        Ok(())
    }
}
