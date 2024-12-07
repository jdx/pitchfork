use crate::{procs, Result};

mod run;
mod start;

#[derive(Debug, clap::Args)]
pub struct Daemon {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Run(run::Run),
    Start(start::Start),
}

impl Daemon {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Commands::Run(run) => run.run().await,
            Commands::Start(start) => start.run().await,
        }
    }
}

/// if --force is passed, will kill existing process
/// Returns false if existing pid is running and --force was not passed (so we should cancel starting the daemon)
pub fn kill_or_stop(existing_pid: u32, force: bool) -> Result<bool> {
    if let Some(process) = procs::get_process(existing_pid) {
        if force {
            if sysinfo::Process::kill_with(process, sysinfo::Signal::Term).is_none() {
                sysinfo::Process::kill(process);
            }
            Ok(true)
        } else {
            let existing_pid = process.pid();
            warn!("Pitchfork is already running with pid {existing_pid}. Kill it with `--force`");
            Ok(false)
        }
    } else {
        Ok(true)
    }
}
