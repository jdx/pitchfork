use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Remove {}

impl Remove {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
