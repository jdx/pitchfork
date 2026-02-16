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

    /// Walk the /proc tree to find all descendant PIDs.
    /// Kept for diagnostics/status display; no longer used in the kill path.
    #[allow(dead_code)]
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

    pub async fn kill_process_group_async(&self, pid: u32) -> Result<bool> {
        let result = tokio::task::spawn_blocking(move || PROCS.kill_process_group(pid))
            .await
            .into_diagnostic()?;
        Ok(result)
    }

    /// Kill an entire process group with graceful shutdown strategy:
    /// 1. Send SIGTERM to the process group (-pgid) and wait up to ~3s
    /// 2. If any processes remain, send SIGKILL to the group
    ///
    /// Since daemons are spawned with setsid(), the daemon PID == PGID,
    /// so this atomically signals all descendant processes.
    #[cfg(unix)]
    fn kill_process_group(&self, pid: u32) -> bool {
        let pgid = pid as i32;

        // Check if the leader process is still around
        if self.is_terminated_or_zombie(sysinfo::Pid::from_u32(pid)) {
            return false;
        }

        debug!("killing process group {pgid}");

        // Send SIGTERM to the entire process group
        // killpg sends to all processes in the group atomically
        let ret = unsafe { libc::killpg(pgid, libc::SIGTERM) };
        if ret == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                debug!("process group {pgid} no longer exists");
                return false;
            }
            warn!("failed to send SIGTERM to process group {pgid}: {err}");
        }

        // Fast check: 10ms intervals for first 100ms
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            self.refresh_pids(&[pid]);
            if self.is_terminated_or_zombie(sysinfo::Pid::from_u32(pid)) {
                debug!(
                    "process group {pgid} terminated after SIGTERM ({} ms)",
                    (i + 1) * 10
                );
                return true;
            }
        }

        // Slower check: 50ms intervals for up to ~3 more seconds
        for i in 0..58 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            self.refresh_pids(&[pid]);
            if self.is_terminated_or_zombie(sysinfo::Pid::from_u32(pid)) {
                debug!(
                    "process group {pgid} terminated after SIGTERM ({} ms)",
                    100 + (i + 1) * 50
                );
                return true;
            }
        }

        // SIGKILL the entire process group as last resort
        warn!("process group {pgid} did not respond to SIGTERM after ~3s, sending SIGKILL");
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }

        // Brief wait for SIGKILL to take effect
        std::thread::sleep(std::time::Duration::from_millis(100));
        true
    }

    #[cfg(not(unix))]
    fn kill_process_group(&self, pid: u32) -> bool {
        // On non-unix platforms, fall back to single-process kill
        self.kill(pid)
    }

    pub async fn kill_async(&self, pid: u32) -> Result<bool> {
        let result = tokio::task::spawn_blocking(move || PROCS.kill(pid))
            .await
            .into_diagnostic()?;
        Ok(result)
    }

    /// Kill a process with graceful shutdown strategy:
    /// 1. Send SIGTERM and wait up to ~3s (10ms intervals for first 100ms, then 50ms intervals)
    /// 2. If still running, send SIGKILL to force termination
    ///
    /// This ensures fast-exiting processes don't wait unnecessarily,
    /// while stubborn processes eventually get forcefully terminated.
    fn kill(&self, pid: u32) -> bool {
        let sysinfo_pid = sysinfo::Pid::from_u32(pid);

        // Check if process exists or is a zombie (already terminated but not reaped)
        if self.is_terminated_or_zombie(sysinfo_pid) {
            return false;
        }

        debug!("killing process {pid}");

        #[cfg(windows)]
        {
            if let Some(process) = self.lock_system().process(sysinfo_pid) {
                process.kill();
                process.wait();
            }
            return true;
        }

        #[cfg(unix)]
        {
            // Send SIGTERM for graceful shutdown
            if let Some(process) = self.lock_system().process(sysinfo_pid) {
                debug!("sending SIGTERM to process {pid}");
                process.kill_with(Signal::Term);
            }

            // Fast check: 10ms intervals for first 100ms (for processes that exit immediately)
            for i in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(10));
                self.refresh_pids(&[pid]);
                if self.is_terminated_or_zombie(sysinfo_pid) {
                    debug!(
                        "process {pid} terminated after SIGTERM ({} ms)",
                        (i + 1) * 10
                    );
                    return true;
                }
            }

            // Slower check: 50ms intervals for up to ~3 more seconds (100ms + 2900ms = 3000ms total)
            for i in 0..58 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                self.refresh_pids(&[pid]);
                if self.is_terminated_or_zombie(sysinfo_pid) {
                    debug!(
                        "process {pid} terminated after SIGTERM ({} ms)",
                        100 + (i + 1) * 50
                    );
                    return true;
                }
            }

            // SIGKILL as last resort after ~3s
            if let Some(process) = self.lock_system().process(sysinfo_pid) {
                warn!("process {pid} did not respond to SIGTERM after ~3s, sending SIGKILL");
                process.kill_with(Signal::Kill);
                process.wait();
            }

            true
        }
    }

    /// Check if a process is terminated or is a zombie.
    /// On Linux, zombie processes still have /proc/[pid] entries but are effectively dead.
    /// This prevents unnecessary signal escalation for processes that have already exited.
    fn is_terminated_or_zombie(&self, sysinfo_pid: sysinfo::Pid) -> bool {
        let system = self.lock_system();
        match system.process(sysinfo_pid) {
            None => true,
            Some(process) => {
                #[cfg(unix)]
                {
                    matches!(process.status(), sysinfo::ProcessStatus::Zombie)
                }
                #[cfg(not(unix))]
                {
                    let _ = process;
                    false
                }
            }
        }
    }

    pub(crate) fn refresh_processes(&self) {
        self.lock_system()
            .refresh_processes(ProcessesToUpdate::All, true);
    }

    /// Refresh only specific PIDs instead of all processes.
    /// More efficient when you only need to check a small set of known PIDs.
    pub(crate) fn refresh_pids(&self, pids: &[u32]) {
        let sysinfo_pids: Vec<sysinfo::Pid> =
            pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        self.lock_system()
            .refresh_processes(ProcessesToUpdate::Some(&sysinfo_pids), true);
    }

    /// Get process stats (cpu%, memory bytes, uptime secs, disk I/O) for a given PID
    pub fn get_stats(&self, pid: u32) -> Option<ProcessStats> {
        let system = self.lock_system();
        system.process(sysinfo::Pid::from_u32(pid)).map(|p| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let disk = p.disk_usage();
            ProcessStats {
                cpu_percent: p.cpu_usage(),
                memory_bytes: p.memory(),
                uptime_secs: now.saturating_sub(p.start_time()),
                disk_read_bytes: disk.read_bytes,
                disk_write_bytes: disk.written_bytes,
            }
        })
    }

    /// Get extended process information for a given PID
    pub fn get_extended_stats(&self, pid: u32) -> Option<ExtendedProcessStats> {
        let system = self.lock_system();
        system.process(sysinfo::Pid::from_u32(pid)).map(|p| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let disk = p.disk_usage();

            ExtendedProcessStats {
                name: p.name().to_string_lossy().to_string(),
                exe_path: p.exe().map(|e| e.to_string_lossy().to_string()),
                cwd: p.cwd().map(|c| c.to_string_lossy().to_string()),
                environ: p
                    .environ()
                    .iter()
                    .take(20)
                    .map(|s| s.to_string_lossy().to_string())
                    .collect(),
                status: format!("{:?}", p.status()),
                cpu_percent: p.cpu_usage(),
                memory_bytes: p.memory(),
                virtual_memory_bytes: p.virtual_memory(),
                uptime_secs: now.saturating_sub(p.start_time()),
                start_time: p.start_time(),
                disk_read_bytes: disk.read_bytes,
                disk_write_bytes: disk.written_bytes,
                parent_pid: p.parent().map(|pp| pp.as_u32()),
                thread_count: p.tasks().map(|t| t.len()).unwrap_or(0),
                user_id: p.user_id().map(|u| u.to_string()),
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessStats {
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub uptime_secs: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
}

impl ProcessStats {
    pub fn memory_display(&self) -> String {
        format_bytes(self.memory_bytes)
    }

    pub fn cpu_display(&self) -> String {
        format!("{:.1}%", self.cpu_percent)
    }

    pub fn uptime_display(&self) -> String {
        format_duration(self.uptime_secs)
    }

    pub fn disk_read_display(&self) -> String {
        format_bytes_per_sec(self.disk_read_bytes)
    }

    pub fn disk_write_display(&self) -> String {
        format_bytes_per_sec(self.disk_write_bytes)
    }
}

/// Extended process stats with more detailed information
#[derive(Debug, Clone)]
pub struct ExtendedProcessStats {
    pub name: String,
    pub exe_path: Option<String>,
    pub cwd: Option<String>,
    pub environ: Vec<String>,
    pub status: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub virtual_memory_bytes: u64,
    pub uptime_secs: u64,
    pub start_time: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub parent_pid: Option<u32>,
    pub thread_count: usize,
    pub user_id: Option<String>,
}

impl ExtendedProcessStats {
    pub fn memory_display(&self) -> String {
        format_bytes(self.memory_bytes)
    }

    pub fn virtual_memory_display(&self) -> String {
        format_bytes(self.virtual_memory_bytes)
    }

    pub fn cpu_display(&self) -> String {
        format!("{:.1}%", self.cpu_percent)
    }

    pub fn uptime_display(&self) -> String {
        format_duration(self.uptime_secs)
    }

    pub fn start_time_display(&self) -> String {
        use std::time::{Duration, UNIX_EPOCH};
        let datetime = UNIX_EPOCH + Duration::from_secs(self.start_time);
        chrono::DateTime::<chrono::Local>::from(datetime)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    pub fn disk_read_display(&self) -> String {
        format_bytes_per_sec(self.disk_read_bytes)
    }

    pub fn disk_write_display(&self) -> String {
        format_bytes_per_sec(self.disk_write_bytes)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{hours}h {mins}m")
    } else {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        format!("{days}d {hours}h")
    }
}

fn format_bytes_per_sec(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B/s")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB/s", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB/s", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB/s", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
