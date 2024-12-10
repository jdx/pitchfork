use crate::Result;

/// List all daemons
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "ls", verbatim_doc_comment)]
pub struct List {}

impl List {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
