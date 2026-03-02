use crate::Result;
use crate::daemon_id::DaemonId;
use crate::error::FileError;
use crate::pitchfork_toml::{PitchforkToml, namespace_from_path};

fn is_project_config_path(path: &std::path::Path) -> bool {
    path.file_name()
        .map(|name| name == "pitchfork.toml" || name == "pitchfork.local.toml")
        .unwrap_or(false)
}

/// Remove a daemon from pitchfork.toml
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "rm", verbatim_doc_comment)]
pub struct Remove {
    /// The ID of the daemon to remove (e.g., "api" or "namespace/api")
    id: String,
}

impl Remove {
    pub async fn run(&self) -> Result<()> {
        // Select the most specific existing project config (closest to CWD)
        let cwd = std::env::current_dir().map_err(|e| FileError::ReadError {
            path: ".".into(),
            source: e,
        })?;
        let paths = PitchforkToml::list_paths_from(&cwd);
        if let Some(path) = paths
            .into_iter()
            .rev()
            .find(|p| p.exists() && is_project_config_path(p))
        {
            let mut pt = PitchforkToml::read(&path)?;
            // Parse the daemon ID: if qualified, use it directly; otherwise use the
            // namespace from the config file being edited (not global resolution)
            // Canonicalize the path first to get correct namespace
            let canonical_path = path.canonicalize().unwrap_or_else(|_| cwd.join(&path));
            let daemon_id = if self.id.contains('/') {
                DaemonId::parse(&self.id)?
            } else {
                let namespace = namespace_from_path(&canonical_path)?;
                DaemonId::try_new(&namespace, &self.id)?
            };
            if pt.daemons.shift_remove(&daemon_id).is_some() {
                pt.write()?;
                println!("removed {} from {}", daemon_id, path.display());
            } else {
                warn!("{} not found in {}", daemon_id, path.display());
            }
        } else {
            warn!("No project pitchfork.toml files found");
        }
        Ok(())
    }
}
