use crate::Result;

/// Kill a running daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "kill", verbatim_doc_comment)]
pub struct Stop {}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
