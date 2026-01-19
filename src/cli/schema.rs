use crate::Result;
use crate::pitchfork_toml::PitchforkToml;
use schemars::schema_for;

/// Generate JSON Schema for pitchfork.toml configuration
#[derive(Debug, clap::Args)]
#[clap(hide = true)]
pub struct Schema;

impl Schema {
    pub async fn run(&self) -> Result<()> {
        let schema = schema_for!(PitchforkToml);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        println!("{json}");
        Ok(())
    }
}
