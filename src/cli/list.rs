use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct List {}

impl List {
    pub async fn run(&self) -> Result<()> {
        Ok(())
    }
}
