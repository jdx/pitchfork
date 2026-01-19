use crate::Result;

/// Launch the interactive TUI dashboard
#[derive(Debug, clap::Args)]
pub struct Tui {}

impl Tui {
    pub async fn run(&self) -> Result<()> {
        crate::tui::run().await
    }
}
