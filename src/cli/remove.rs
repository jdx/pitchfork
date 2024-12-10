use crate::Result;

/// Remove a daemon from pitchfork.toml
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "rm", verbatim_doc_comment)]
pub struct Remove {}

impl Remove {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
