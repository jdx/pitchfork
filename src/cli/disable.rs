use crate::ipc::client::IpcClient;
use crate::Result;

/// Prevent a daemon from restarting
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "d", verbatim_doc_comment)]
pub struct Disable {
    /// Name of the daemon to disable
    id: String,
}

impl Disable {
    pub async fn run(&self) -> Result<()> {
        let ipc = IpcClient::connect(false).await?;
        ipc.disable(self.id.clone()).await?;
        Ok(())
    }
}
