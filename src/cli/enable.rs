use crate::ipc::client::IpcClient;
use crate::Result;

/// Allow a daemon to start
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "e", verbatim_doc_comment)]
pub struct Enable {
    /// Name of the daemon to enable
    id: String,
}

impl Enable {
    pub async fn run(&self) -> Result<()> {
        let ipc = IpcClient::connect(false).await?;
        ipc.enable(self.id.clone()).await?;
        Ok(())
    }
}
