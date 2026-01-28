use crate::Result;
use crate::deps::resolve_dependencies;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use miette::ensure;
use std::collections::HashSet;
use std::sync::Arc;

/// Sends a stop signal to a daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "kill",
    verbatim_doc_comment,
    long_about = "\
Sends a stop signal to a daemon

Sends SIGTERM to gracefully stop a running daemon. Use 'pitchfork status'
to check if the daemon has stopped.

When using --all, daemons are stopped in reverse dependency order:
dependents are stopped before the daemons they depend on.

Examples:
  pitchfork stop api           Stop a single daemon
  pitchfork stop api worker    Stop multiple daemons
  pitchfork stop --all         Stop all running daemons in dependency order
  pitchfork kill api           Same as 'stop' (alias)"
)]
pub struct Stop {
    /// The name of the daemon(s) to stop
    id: Vec<String>,
    /// Stop all running daemons (in reverse dependency order)
    #[clap(long, short)]
    all: bool,
}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            !(self.all && !self.id.is_empty()),
            "--all and daemon IDs cannot be used together"
        );
        ensure!(
            self.all || !self.id.is_empty(),
            "At least one daemon ID must be provided (or use --all)"
        );

        let ipc = Arc::new(IpcClient::connect(false).await?);

        if self.all {
            self.stop_all(&ipc).await
        } else {
            self.stop_specific(&ipc).await
        }
    }

    /// Stop specific daemons by ID (original behavior)
    async fn stop_specific(&self, ipc: &Arc<IpcClient>) -> Result<()> {
        let mut any_failed = false;
        for id in &self.id {
            if let Err(e) = ipc.stop(id.clone()).await {
                error!("Failed to stop daemon {id}: {e}");
                any_failed = true;
            }
        }
        if any_failed {
            std::process::exit(1);
        }
        Ok(())
    }

    /// Stop all running daemons in reverse dependency order
    async fn stop_all(&self, ipc: &Arc<IpcClient>) -> Result<()> {
        let pt = PitchforkToml::all_merged();

        // Get currently running daemons
        let running_daemons: HashSet<String> = ipc
            .active_daemons()
            .await?
            .iter()
            .filter(|d| d.status.is_running())
            .map(|d| d.id.clone())
            .collect();

        if running_daemons.is_empty() {
            info!("No running daemons to stop");
            return Ok(());
        }

        // Filter to only daemons that exist in config (for dependency resolution)
        let running_in_config: Vec<String> = running_daemons
            .iter()
            .filter(|id| pt.daemons.contains_key(*id))
            .cloned()
            .collect();

        // Daemons not in config (started ad-hoc) - stop these first
        let adhoc_daemons: Vec<String> = running_daemons
            .iter()
            .filter(|id| !pt.daemons.contains_key(*id))
            .cloned()
            .collect();

        let mut any_failed = false;

        // Stop ad-hoc daemons first (no dependency info) concurrently
        if !adhoc_daemons.is_empty() {
            debug!("Stopping ad-hoc daemons: {adhoc_daemons:?}");
            let mut tasks = Vec::new();
            for id in adhoc_daemons {
                let ipc_clone = ipc.clone();
                tasks.push(tokio::spawn(async move {
                    let result = ipc_clone.stop(id.clone()).await;
                    (id, result)
                }));
            }
            for task in tasks {
                match task.await {
                    Ok((id, Err(e))) => {
                        error!("Failed to stop daemon {id}: {e}");
                        any_failed = true;
                    }
                    Err(e) => {
                        error!("Task panicked: {e}");
                        any_failed = true;
                    }
                    _ => {}
                }
            }
        }

        // Resolve dependencies for config-based daemons
        if !running_in_config.is_empty() {
            let dep_order = resolve_dependencies(&running_in_config, &pt.daemons)?;

            // Reverse the levels: stop dependents first, then their dependencies
            let reversed_levels: Vec<Vec<String>> = dep_order.levels.into_iter().rev().collect();

            for level in reversed_levels {
                // Filter to only running daemons in this level
                let to_stop: Vec<String> = level
                    .into_iter()
                    .filter(|id| running_daemons.contains(id))
                    .collect();

                if to_stop.is_empty() {
                    continue;
                }

                debug!("Stopping level: {to_stop:?}");

                // Stop all daemons in this level concurrently
                let mut tasks = Vec::new();
                for id in to_stop {
                    let ipc_clone = ipc.clone();
                    tasks.push(tokio::spawn(async move {
                        let result = ipc_clone.stop(id.clone()).await;
                        (id, result)
                    }));
                }

                // Wait for all daemons in this level to stop before moving to next level
                for task in tasks {
                    match task.await {
                        Ok((id, Err(e))) => {
                            error!("Failed to stop daemon {id}: {e}");
                            any_failed = true;
                        }
                        Err(e) => {
                            error!("Task panicked: {e}");
                            any_failed = true;
                        }
                        _ => {}
                    }
                }
            }
        }

        if any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
