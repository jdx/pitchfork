use crate::Result;

/// Activate pitchfork in your shell session
///
/// Necessary for autostart/stop when entering/exiting projects with pitchfork.toml files
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
pub struct Activate {
    /// The shell to generate source for
    #[clap()]
    shell: String,
}

impl Activate {
    pub async fn run(&self) -> Result<()> {
        unimplemented!();
    }
}
