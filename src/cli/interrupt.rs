use crate::Result;

/// Sends Ctrl+C to the console of a daemon being stopped (Windows)
///
/// Run by the supervisor to stop a daemon whose `stop_signal` is SIGINT, since
/// Windows interrupts console programs with Ctrl+C rather than a signal.
/// Raising Ctrl+C means attaching to the daemon's console, which cannot be done
/// from inside the supervisor, so it is done here. Exits with status 3, sending
/// nothing, when the daemon shares the supervisor's console.
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct Interrupt {
    /// Process whose console receives Ctrl+C
    #[usage(long)]
    pid: u32,

    /// The supervisor's process, whose console is never interrupted
    #[usage(long)]
    supervisor_pid: u32,
}

impl Interrupt {
    pub async fn run(&self) -> Result<()> {
        #[cfg(windows)]
        {
            use crate::console_ctrl::{EXIT_SHARED_CONSOLE, Interrupted, interrupt_console_of};
            match interrupt_console_of(self.pid, self.supervisor_pid) {
                Ok(Interrupted::Sent) => Ok(()),
                Ok(Interrupted::SharedWithSupervisor) => std::process::exit(EXIT_SHARED_CONSOLE),
                Err(e) => Err(miette::miette!(
                    "failed to send Ctrl+C to the console of process {}: {e}",
                    self.pid
                )),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (self.pid, self.supervisor_pid);
            Err(miette::miette!(
                "pitchfork interrupt is only used on Windows"
            ))
        }
    }
}
