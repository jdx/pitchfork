use crate::ipc::client::IpcClient;
use crate::ipc::IpcMessage;
use crate::Result;
use miette::bail;

/// Runs a one-off daemon
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Run {
    /// Name of the daemon to run
    name: String,
    #[clap(trailing_var_arg = true)]
    cmd: Vec<String>,
    #[clap(short, long)]
    force: bool,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        info!("Running one-off daemon");
        if self.cmd.is_empty() {
            bail!("No command provided");
        }

        let ipc = IpcClient::connect().await?;
        ipc.send(IpcMessage::Run(self.name.clone(), self.cmd.clone()))
            .await?;
        loop {
            match ipc.read().await? {
                IpcMessage::Started(name) => {
                    info!("Started daemon {}", name);
                    break;
                }
                msg => {
                    debug!("ignoring message: {:?}", msg);
                }
            }
        }
        Ok(())
    }
}
