use crate::Result;

/// Display the status of a daemons
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "stat", verbatim_doc_comment)]
pub struct Status {}

impl Status {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
