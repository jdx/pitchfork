use crate::state_file::{DaemonStatus, StateFile};
use crate::Result;

/// Removes stopped/failed daemons from `pitchfork list`
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "c", verbatim_doc_comment)]
pub struct Clean {}

impl Clean {
    pub async fn run(&self) -> Result<()> {
        let mut sf = StateFile::get().clone();
        let count = sf.daemons.len();
        sf.daemons
            .retain(|_, d| matches!(d.status, DaemonStatus::Running | DaemonStatus::Waiting));
        sf.write()?;
        println!("Removed {} daemons", count - sf.daemons.len());
        Ok(())
    }
}
