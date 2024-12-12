use crate::ipc::client::IpcClient;
use crate::Result;

/// Removes stopped/failed daemons from `pitchfork list`
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "c", verbatim_doc_comment)]
pub struct Clean {}

impl Clean {
    pub async fn run(&self) -> Result<()> {
        let ipc = IpcClient::connect(false).await?;
        ipc.clean().await?;
        Ok(())
    }
}
