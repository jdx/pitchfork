use crate::Result;
use crate::cli::Cli;
use miette::IntoDiagnostic as _;
use tokio::io::AsyncWriteExt as _;

/// Generates a usage spec for the CLI
///
/// https://usage.jdx.dev
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct Usage {}

impl Usage {
    pub async fn run(&self) -> Result<()> {
        let overlays: Vec<_> = super::command_effects::EFFECTS
            .iter()
            .map(|(path, effect)| usage_rs::spec::CommandOverlay::effect(path, *effect))
            .collect();
        // The command tree comes from the spec view (so command-effect
        // overlays apply); the settings `config` block comes from the
        // `usage_rs::Config` derive on the settings structs — the same block
        // `Cli::to_kdl()` would append via `#[usage(config = ...)]`.
        let rendered = format!(
            "// @generated from usage-rs metadata\n{}\n{}{}\n",
            Cli::spec().view().overlay(&overlays).to_kdl(),
            crate::settings::Settings::spec_kdl(),
            include_str!("../../pitchfork-extras.usage.kdl")
        );
        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(rendered.as_bytes())
            .await
            .into_diagnostic()?;
        stdout.flush().await.into_diagnostic()?;
        Ok(())
    }
}
