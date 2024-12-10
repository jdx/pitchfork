use crate::ipc::client::IpcClient;
use crate::ipc::IpcMessage;
use crate::Result;
use miette::bail;

/// Runs a one-off daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "r", verbatim_doc_comment)]
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
                IpcMessage::DaemonStart(daemon) => {
                    info!(
                        "Started daemon {} with pid {}",
                        daemon.name,
                        daemon.pid.unwrap()
                    );
                    break;
                }
                IpcMessage::DaemonFailed { name, error } => {
                    bail!("Failed to start daemon {}: {}", name, error);
                }
                msg => {
                    debug!("ignoring message: {:?}", msg);
                }
            }
        }
        Ok(())
    }
}
