use crate::Result;

mod add;
mod remove;

/// manage/edit pitchfork.toml files
#[derive(Debug, clap::Args)]
pub struct Config {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Add(add::Add),
    Remove(remove::Remove),
}

impl Config {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Commands::Add(add) => add.run().await,
            Commands::Remove(remove) => remove.run().await,
        }
    }
}
