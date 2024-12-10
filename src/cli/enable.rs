use crate::state_file::StateFile;
use crate::Result;
use miette::bail;

/// Allow a daemon to start
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "e", verbatim_doc_comment)]
pub struct Enable {
    /// Name of the daemon to enable
    id: String,
}

impl Enable {
    pub async fn run(&self) -> Result<()> {
        let mut sf = StateFile::get().clone();
        if self.id == "pitchfork" {
            bail!("Cannot disable pitchfork daemon");
        }
        let enabled = sf.disabled.remove(&self.id);
        if enabled {
            sf.write()?;
            println!("enabled {}", self.id);
        }
        Ok(())
    }
}
