use crate::Result;

mod run;

#[derive(Debug, clap::Args)]
pub struct Daemon {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Run(run::Run),
}

impl Daemon {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Commands::Run(run) => run.run().await,
        }
    }
}
