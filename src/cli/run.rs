use crate::daemon::RunOptions;
use crate::ipc::client::IpcClient;
use crate::Result;
use miette::bail;

/// Runs a one-off daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "r", verbatim_doc_comment)]
pub struct Run {
    /// Name of the daemon to run
    id: String,
    #[clap(allow_hyphen_values = true, trailing_var_arg = true)]
    run: Vec<String>,
    #[clap(short, long)]
    force: bool,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        info!("Running one-off daemon");
        if self.run.is_empty() {
            bail!("No command provided");
        }

        let ipc = IpcClient::connect(true).await?;

        ipc.run(RunOptions {
            id: self.id.clone(),
            cmd: self.run.clone(),
            shell_pid: None,
            force: self.force,
            dir: None,
            autostop: false,
        })
        .await?;
        Ok(())
    }
}
