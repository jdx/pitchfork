use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Disable {}

impl Disable {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
