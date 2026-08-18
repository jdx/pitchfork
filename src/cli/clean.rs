use crate::Result;
use crate::ipc::client::IpcClient;
use crate::{daemon_id::DaemonId, env, pitchfork_toml::PitchforkToml};

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
    pitchfork clean my-worktree     Remove entries in one namespace
    pitchfork clean --daemon api    Remove the local namespace's api entry
    pitchfork clean --prune         Remove entries whose directories disappeared
    pitchfork c                     Alias for 'clean'"
)]
pub struct Clean {
    /// Only clean daemon registrations in these namespaces
    #[clap(value_name = "NAMESPACE")]
    namespaces: Vec<String>,
    /// Only clean these daemons (repeatable; bare names use the current namespace)
    #[clap(long = "daemon", value_name = "ID")]
    daemons: Vec<String>,
    /// Only clean registrations whose working directories no longer exist
    #[clap(long)]
    prune: bool,
}

impl Clean {
    pub async fn run(&self) -> Result<()> {
        if self.namespaces.is_empty() && self.daemons.is_empty() && !self.prune {
            let ipc = IpcClient::connect(false).await?;
            ipc.clean().await?;
            return Ok(());
        }

        // Validate namespace arguments using the same rules as daemon IDs.
        for namespace in &self.namespaces {
            DaemonId::try_new(namespace, "probe")?;
        }
        let local_namespace = PitchforkToml::namespace_for_dir(&env::CWD)?;
        let daemons = self
            .daemons
            .iter()
            .map(|id| {
                if id.contains('/') {
                    DaemonId::parse(id)
                } else {
                    DaemonId::try_new(&local_namespace, id)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let ipc = IpcClient::connect(false).await?;
        let count = ipc
            .clean_filtered(self.namespaces.clone(), daemons, self.prune)
            .await?;
        info!("Cleaned up {count} stopped/failed daemon registration(s)");
        Ok(())
    }
}
