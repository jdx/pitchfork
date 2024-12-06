use crate::state_file::StateFile;
use crate::supervisor::Supervisor;
use crate::Result;
use crate::{env, procs};

#[derive(Debug, clap::Args)]
pub struct Run {
    #[clap(short, long)]
    force: bool,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        let pid_file = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
        if let Some(d) = pid_file.daemons.get("pitchfork") {
            if !(self.kill_or_stop(d.pid)?) {
                return Ok(());
            }
        }

        Supervisor::new(pid_file).start().await
    }

    /// if --force is passed, will kill existing process
    /// Returns false if existing pid is running and --force was not passed (so we should cancel starting the daemon)
    fn kill_or_stop(&self, existing_pid: u32) -> Result<bool> {
        if let Some(process) = procs::get_process(existing_pid) {
            if self.force {
                if sysinfo::Process::kill_with(process, sysinfo::Signal::Term).is_none() {
                    sysinfo::Process::kill(process);
                }
                Ok(true)
            } else {
                let existing_pid = process.pid();
                warn!(
                    "Pitchfork is already running with pid {existing_pid}. Kill it with `--force`"
                );
                Ok(false)
            }
        } else {
            Ok(true)
        }
    }
}
