use crate::state_file::StateFile;
use crate::Result;
use miette::bail;

/// Prevent a daemon from restarting
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "d", verbatim_doc_comment)]
pub struct Disable {
    /// Name of the daemon to disable
    id: String,
}

impl Disable {
    pub async fn run(&self) -> Result<()> {
        let mut sf = StateFile::get().clone();
        if self.id == "pitchfork" {
            bail!("Cannot disable pitchfork daemon");
        }
        let disabled = sf.disabled.insert(self.id.clone());
        if disabled {
            sf.write()?;
            println!("disabled {}", self.id);
        }
        Ok(())
    }
}
