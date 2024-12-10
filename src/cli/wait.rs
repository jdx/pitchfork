use crate::Result;

/// Wait for a daemon to stop, tailing the logs along the way
///
/// Exits with the same status code as the daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "w", verbatim_doc_comment)]
pub struct Wait {}

impl Wait {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
