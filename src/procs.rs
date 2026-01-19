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

    /// Get process stats (cpu%, memory bytes, uptime secs) for a given PID
    pub fn get_stats(&self, pid: u32) -> Option<ProcessStats> {
        let system = self.lock_system();
        system.process(sysinfo::Pid::from_u32(pid)).map(|p| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            ProcessStats {
                cpu_percent: p.cpu_usage(),
                memory_bytes: p.memory(),
                uptime_secs: now.saturating_sub(p.start_time()),
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessStats {
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub uptime_secs: u64,
}

impl ProcessStats {
    pub fn memory_display(&self) -> String {
        let bytes = self.memory_bytes;
        if bytes < 1024 {
            format!("{}B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1}KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    pub fn cpu_display(&self) -> String {
        format!("{:.1}%", self.cpu_percent)
    }

    pub fn uptime_display(&self) -> String {
        let secs = self.uptime_secs;
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else if secs < 86400 {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            format!("{}h {}m", hours, mins)
        } else {
            let days = secs / 86400;
            let hours = (secs % 86400) / 3600;
            format!("{}d {}h", days, hours)
        }
    }
}
