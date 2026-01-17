use crate::Result;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use sysinfo::ProcessesToUpdate;
#[cfg(unix)]
use sysinfo::Signal;

pub struct Procs {
    system: Mutex<sysinfo::System>,
}

pub static PROCS: Lazy<Procs> = Lazy::new(Procs::new);

impl Default for Procs {
    fn default() -> Self {
        Self::new()
    }
}

impl Procs {
    pub fn new() -> Self {
        let procs = Self {
            system: Mutex::new(sysinfo::System::new()),
        };
        procs.refresh_processes();
        procs
    }

    fn lock_system(&self) -> std::sync::MutexGuard<'_, sysinfo::System> {
        self.system.lock().unwrap_or_else(|poisoned| {
            warn!("System mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }

    pub fn title(&self, pid: u32) -> Option<String> {
        self.lock_system()
            .process(sysinfo::Pid::from_u32(pid))
            .map(|p| p.name().to_string_lossy().to_string())
    }

    pub fn is_running(&self, pid: u32) -> bool {
        self.lock_system()
            .process(sysinfo::Pid::from_u32(pid))
            .is_some()
    }

    pub fn all_children(&self, pid: u32) -> Vec<u32> {
        let system = self.lock_system();
        let all = system.processes();
        let mut children = vec![];
        for (child_pid, process) in all {
            let mut process = process;
            while let Some(parent) = process.parent() {
                if parent == sysinfo::Pid::from_u32(pid) {
                    children.push(child_pid.as_u32());
                    break;
                }
                match system.process(parent) {
                    Some(p) => process = p,
                    None => break,
                }
            }
        }
        children
    }

    pub async fn kill_async(&self, pid: u32) -> Result<bool> {
        let result = tokio::task::spawn_blocking(move || PROCS.kill(pid))
            .await
            .into_diagnostic()?;
        Ok(result)
    }

    fn kill(&self, pid: u32) -> bool {
        if let Some(process) = self.lock_system().process(sysinfo::Pid::from_u32(pid)) {
            debug!("killing process {}", pid);
            #[cfg(windows)]
            process.kill();
            #[cfg(unix)]
            process.kill_with(Signal::Term);
            process.wait();
            true
        } else {
            false
        }
    }

    pub(crate) fn refresh_processes(&self) {
        self.lock_system()
            .refresh_processes(ProcessesToUpdate::All, true);
    }
}
