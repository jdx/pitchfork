use crate::pitchfork_toml::PitchforkToml;
use crate::Result;

/// Starts a daemon from a pitchfork.toml file
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "s", verbatim_doc_comment)]
pub struct Start {
    /// Name of the daemon(s) in pitchfork.toml to start
    name: Vec<String>,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        // TODO: read all tomls
        let pt = PitchforkToml::read("pitchfork.toml")?;
        dbg!(&pt);
        pt.write()?;
        Ok(())
    }
}
