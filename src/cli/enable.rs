use crate::Result;

/// Allow a daemon to start
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "e", verbatim_doc_comment)]
pub struct Enable {}

impl Enable {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
