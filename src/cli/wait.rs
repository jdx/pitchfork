use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Wait {}

impl Wait {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
