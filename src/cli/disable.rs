use crate::Result;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;

/// Prevent a daemon from restarting
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "d",
    verbatim_doc_comment,
    long_about = "\
Prevent a daemon from restarting

Disables a daemon to prevent it from being started automatically or manually.
The daemon will remain disabled until 'pitchfork enable' is called.
Useful for temporarily stopping a service without removing it from config.

Examples:
  pitchfork disable api           Prevent daemon from starting
  pitchfork d api                 Alias for 'disable'
  pitchfork list                  Shows 'disabled' status in output"
)]
pub struct Disable {
    /// Name of the daemon to disable
    id: String,
}

impl Disable {
    pub async fn run(&self) -> Result<()> {
        let id = PitchforkToml::resolve_id(&self.id)?;
        let ipc = IpcClient::connect(false).await?;
        ipc.disable(id).await?;
        Ok(())
    }
}
