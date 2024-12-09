use crate::procs::Procs;
use crate::state_file::StateFile;
use crate::{env, Result};
use duct::cmd;
use miette::IntoDiagnostic;

mod run;
mod start;
mod status;
mod stop;

/// Start, stop, and check the status of the pitchfork supervisor daemon
#[derive(Debug, clap::Args)]
pub struct Supervisor {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Run(run::Run),
    Start(start::Start),
    Status(status::Status),
    Stop(stop::Stop),
}

impl Supervisor {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Commands::Run(run) => run.run().await,
            Commands::Start(start) => start.run().await,
            Commands::Status(status) => status.run().await,
            Commands::Stop(stop) => stop.run().await,
        }
    }
}

/// if --force is passed, will kill existing process
/// Returns false if existing pid is running and --force was not passed (so we should cancel starting the daemon)
pub fn kill_or_stop(existing_pid: u32, force: bool) -> Result<bool> {
    if let Some(process) = Procs::new().get_process(existing_pid) {
        if force {
            debug!("killing pid {existing_pid}");
            if sysinfo::Process::kill_with(process, sysinfo::Signal::Term).is_none() {
                sysinfo::Process::kill(process);
            }
            Ok(true)
        } else {
            let existing_pid = process.pid();
            warn!("pitchfork supervisor is already running with pid {existing_pid}. Kill it with `--force`");
            Ok(false)
        }
    } else {
        Ok(true)
    }
}

pub fn start() -> Result<()> {
    cmd!(&*env::BIN_PATH, "supervisor", "run")
        .stdout_null()
        .stderr_null()
        .start()
        .into_diagnostic()?;
    Ok(())
}

pub fn start_if_not_running() -> Result<()> {
    let sf = StateFile::get();
    if let Some(d) = sf.daemons.get("pitchfork") {
        if let Some(pid) = d.pid {
            if Procs::new().get_process(pid).is_some() {
                return Ok(());
            }
        }
    }
    start()
}
