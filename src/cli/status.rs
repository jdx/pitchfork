use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Status {}

impl Status {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
