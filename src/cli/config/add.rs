use crate::Result;

/// Add a new daemon to pitchfork.toml
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "a", verbatim_doc_comment)]
pub struct Add {}

impl Add {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
