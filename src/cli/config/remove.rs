use crate::Result;
use crate::pitchfork_toml::PitchforkToml;

/// Remove a daemon from pitchfork.toml
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "rm", verbatim_doc_comment)]
pub struct Remove {
    /// The ID of the daemon to remove
    id: String,
}

impl Remove {
    pub async fn run(&self) -> Result<()> {
        if let Some(path) = PitchforkToml::list_paths().first() {
            let mut pt = PitchforkToml::read(path)?;
            if pt.daemons.shift_remove(&self.id).is_some() {
                pt.write()?;
                println!("removed {} from {}", self.id, path.display());
            } else {
                warn!("{} not found in {}", self.id, path.display());
            }
        } else {
            warn!("No pitchfork.toml files found");
        }
        Ok(())
    }
}
