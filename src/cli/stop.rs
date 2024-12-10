use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Stop {}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
