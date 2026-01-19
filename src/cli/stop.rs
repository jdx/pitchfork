use crate::Result;
use crate::deps::resolve_dependencies;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use miette::ensure;

/// Sends a stop signal to a daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "kill",
    verbatim_doc_comment,
    long_about = "\
Sends a stop signal to a daemon

Sends SIGTERM to gracefully stop a running daemon. Use 'pitchfork status'
to check if the daemon has stopped.

Examples:
  pitchfork stop api           Stop a single daemon
  pitchfork stop api worker    Stop multiple daemons
  pitchfork stop --all         Stop all daemons in pitchfork.toml
  pitchfork kill api           Same as 'stop' (alias)"
)]
pub struct Stop {
    /// The name of the daemon to stop
    id: Vec<String>,
    /// Stop all daemons in all pitchfork.tomls
    #[clap(long, short)]
    all: bool,
}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.all || !self.id.is_empty(),
            "At least one daemon ID must be provided"
        );
        let pt = PitchforkToml::all_merged();
        let ipc = IpcClient::connect(false).await?;
        let disabled_daemons = ipc.get_disabled_daemons().await?;

        let requested_ids: Vec<String> = if self.all {
            pt.daemons.keys().cloned().collect()
        } else {
            self.id.clone()
        };

        // Filter out disabled daemons from the requested list
        let requested_ids: Vec<String> = requested_ids
            .into_iter()
            .filter(|id| {
                if disabled_daemons.contains(id) {
                    warn!("Daemon {} is disabled", id);
                    false
                } else {
                    true
                }
            })
            .collect();

        if requested_ids.is_empty() {
            return Ok(());
        }

        let dep_order = resolve_dependencies(&requested_ids, &pt.daemons)?;
        for level in dep_order.levels.iter().rev() {
            for id in level {
                ipc.stop(id.clone()).await?;
            }
        }

        Ok(())
    }
}
