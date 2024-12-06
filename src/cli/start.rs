use crate::pitchfork_toml::PitchforkToml;
use crate::Result;

/// Starts a daemon from a pitchfork.toml file
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Start {}

impl Start {
    pub async fn run(&self) -> Result<()> {
        // TODO: read all tomls
        let pt = PitchforkToml::read("pitchfork.toml")?;
        dbg!(pt);
        pt.write();
        Ok(())
    }
}
