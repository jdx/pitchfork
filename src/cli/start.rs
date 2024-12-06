use crate::Result;

#[derive(Debug, clap::Args)]
#[clap()]
pub struct Start {}

impl Start {
    pub async fn run(&self) -> Result<()> {
        println!("Start");
        Ok(())
    }
}
