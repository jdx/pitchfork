use crate::Result;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;

/// Allow a daemon to start
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "e",
    verbatim_doc_comment,
    long_about = "\
Allow a daemon to start

Re-enables a previously disabled daemon, allowing it to be started manually
or automatically. Use this after 'pitchfork disable' to restore normal operation.

Examples:
  pitchfork enable api            Enable a disabled daemon
  pitchfork e api                 Alias for 'enable'"
)]
pub struct Enable {
    /// Name of the daemon to enable
    id: String,
}

impl Enable {
    pub async fn run(&self) -> Result<()> {
        let id = PitchforkToml::resolve_id(&self.id)?;
        let ipc = IpcClient::connect(false).await?;
        ipc.enable(id).await?;
        Ok(())
    }
}
