use crate::Result;
use crate::cli::daemons::resolve_config_path;
use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::{PitchforkToml, namespace_from_path};
use miette::IntoDiagnostic;

/// Remove a daemon from a pitchfork config file
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "rm", verbatim_doc_comment)]
pub struct Remove {
    /// The ID of the daemon to remove (e.g., "api" or "namespace/api")
    id: String,
    /// Remove from pitchfork.local.toml instead of pitchfork.toml
    #[clap(long)]
    local: bool,
    /// Remove from pitchfork.toml explicitly (default if no flag specified)
    #[clap(long)]
    project: bool,
    /// Remove from the user-level global config (~/.config/pitchfork/config.toml)
    #[clap(long)]
    global: bool,
}

impl Remove {
    pub async fn run(&self) -> Result<()> {
        let path = resolve_config_path(self.global, self.local, self.project, true).await?;

        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            if self.global {
                warn!("No global config.toml found at {}", path.display());
            } else if self.local {
                warn!("No pitchfork.local.toml found");
            } else {
                warn!("No project pitchfork.toml files found");
            }
            return Ok(());
        }

        let mut pt = {
            let path_clone = path.clone();
            let result = tokio::task::spawn_blocking(move || PitchforkToml::read(&path_clone))
                .await
                .into_diagnostic()?;
            result.map_err(|e| miette::miette!("{e}"))?
        };
        let canonical_path = tokio::fs::canonicalize(&path)
            .await
            .unwrap_or_else(|_| path.clone());
        let daemon_id = if self.id.contains('/') {
            DaemonId::parse(&self.id)?
        } else {
            let namespace = namespace_from_path(&canonical_path)?;
            DaemonId::try_new(&namespace, &self.id)?
        };
        if pt.daemons.shift_remove(&daemon_id).is_some() {
            tokio::task::spawn_blocking(move || pt.write())
                .await
                .into_diagnostic()?
                .map_err(|e| miette::miette!("{e}"))?;
            println!("removed {} from {}", daemon_id, path.display());
        } else {
            warn!("{} not found in {}", daemon_id, path.display());
        }
        Ok(())
    }
}
