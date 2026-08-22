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
        let rendered = app().completion_script(self.shell.clone().into());
        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(rendered.as_bytes())
            .await
            .into_diagnostic()?;
        stdout.flush().await.into_diagnostic()?;
        Ok(())
    }
}

fn complete_daemon_ids(
    ctx: usage_rs::spec::CompleteCtx<'_>,
) -> usage_rs::complete::CompletionFuture<'_> {
    Box::pin(async move {
        let statuses: &[&str] = match ctx.words.get(1).map(String::as_str) {
            Some("stop" | "wait") => &["running"],
            Some("restart") => &["running", "stopped", "errored", "failed"],
            Some("start") => &["available", "stopped", "errored", "failed"],
            Some("enable") => &["disabled"],
            Some("disable") => &["running", "available"],
            _ => &[],
        };
        let Ok(exe) = std::env::current_exe() else {
            return Vec::new();
        };
        let mut command = tokio::process::Command::new(exe);
        command.args(["ls", "--hide-header"]);
        for status in statuses {
            command.args(["--status", status]);
        }
        let Ok(output) = command.output().await else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(|id| usage_rs::spec::Candidate::new(id.to_string()))
            .collect()
    })
}

static COMPLETIONS: [usage_rs::complete::CompletionOverlay<'static>; 1] =
    [usage_rs::complete::CompletionOverlay::async_any(
        "id",
        complete_daemon_ids,
    )];

pub(super) fn app() -> usage_rs::complete::App<'static> {
    crate::cli::Cli::app()
        .completion_app()
        .completions(&COMPLETIONS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_script_uses_the_runtime_completion_protocol() {
        let script = app().completion_script(usage_rs::complete::Shell::Bash);
        assert!(script.contains("__complete_word__"), "{script}");
    }
}
