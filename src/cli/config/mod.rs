use crate::Result;
use crate::pitchfork_toml::PitchforkToml;

mod add;
mod remove;

/// manage/edit pitchfork.toml files
///
/// without a subcommand, lists all pitchfork.toml files from the current directory
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "cfg", verbatim_doc_comment)]
pub struct Config {
    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Add(add::Add),
    Remove(remove::Remove),
}

impl Config {
    pub async fn run(self) -> Result<()> {
        if let Some(cmd) = self.command {
            match cmd {
                Commands::Add(add) => add.run().await,
                Commands::Remove(remove) => remove.run().await,
            }
        } else {
            for p in PitchforkToml::list_paths() {
                println!("{}", p.display());
            }
            Ok(())
        }
    }
}
