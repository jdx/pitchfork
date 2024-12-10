use crate::cli::supervisor;
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
        supervisor::start_if_not_running()?;
        info!("Running one-off daemon");
        if self.cmd.is_empty() {
            bail!("No command provided");
        }

        let ipc = IpcClient::connect().await?;

        if self.force {
            ipc.send(IpcMessage::Stop(self.name.clone())).await?;
            loop {
                match ipc.read().await {
                    Some(IpcMessage::DaemonStop { name }) => {
                        info!("stopped daemon {}", name);
                        break;
                    }
                    None => {
                        break;
                    }
                    msg => {
                        debug!("ignoring message: {:?}", msg);
                    }
                }
            }
        }

        ipc.send(IpcMessage::Run(self.name.clone(), self.cmd.clone()))
            .await?;
        loop {
            match ipc.read().await {
                Some(IpcMessage::DaemonAlreadyRunning(id)) => {
                    if self.force {
                        bail!("failed to stop daemon {}", id);
                    } else {
                        info!("daemon {} already running", id);
                    }
                    break;
                }
                Some(IpcMessage::DaemonStart(daemon)) => {
                    info!(
                        "started daemon {} with pid {}",
                        daemon.name,
                        daemon.pid.unwrap()
                    );
                    break;
                }
                Some(IpcMessage::DaemonFailed { name, error }) => {
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
