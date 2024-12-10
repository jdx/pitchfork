use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Enable {}

impl Enable {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
