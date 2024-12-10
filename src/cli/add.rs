use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Add {}

impl Add {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
