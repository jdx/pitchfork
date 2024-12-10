use crate::Result;

/// Prevent a daemon from restarting
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "d", verbatim_doc_comment)]
pub struct Disable {}

impl Disable {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
