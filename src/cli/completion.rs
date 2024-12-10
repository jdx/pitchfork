use crate::Result;
use duct::cmd;
use miette::IntoDiagnostic;

/// Generates shell completion scripts
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Completion {
    /// The shell to generate completion for
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
