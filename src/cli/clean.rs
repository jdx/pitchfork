use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Clean {}

impl Clean {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
