use crate::ipc::client::IpcClient;
use crate::Result;

/// Removes stopped/failed daemons from `pitchfork list`
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "c",
    verbatim_doc_comment,
    long_about = "\
Removes stopped/failed daemons from `pitchfork list`

Cleans up the daemon list by removing entries for daemons that are no
longer running. Does not affect running daemons or their configurations.

Use this to clear out old entries after stopping daemons manually or
after daemons have failed.

Examples:
  pitchfork clean                 Remove all stopped/failed entries
  pitchfork c                     Alias for 'clean'"
)]
pub struct Clean {}

impl Clean {
    pub async fn run(&self) -> Result<()> {
        let ipc = IpcClient::connect(false).await?;
        ipc.clean().await?;
        Ok(())
    }
}
