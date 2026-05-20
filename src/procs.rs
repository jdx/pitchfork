use crate::Result;
#[cfg(unix)]
use crate::settings::settings;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use sysinfo::ProcessesToUpdate;

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

    pub async fn kill_process_group_async(
        &self,
        pid: u32,
        stop_signal: i32,
        stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        tokio::task::spawn_blocking(move || {
            PROCS.kill_process_group(pid, stop_signal, stop_timeout)
        })
        .await
        .into_diagnostic()?
    }

    /// Kill an entire process group with graceful shutdown strategy:
    /// 1. Send the configured stop signal to the process group (-pgid) and wait up to ~3s
    /// 2. If any processes remain, send SIGKILL to the group
    ///
    /// Since daemons are spawned with setsid(), the daemon PID == PGID,
    /// so this atomically signals all descendant processes.
    ///
    /// Returns `Err` if the signal could not be sent (e.g. permission denied).
    #[cfg(unix)]
    fn kill_process_group(
        &self,
        pid: u32,
        stop_signal: i32,
        stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        let pgid = pid as i32;
        let signal_name = signal_name(stop_signal);

        debug!("killing process group {pgid} with {signal_name}");

        // Send the stop signal to the entire process group.
        // killpg sends to all processes in the group atomically.
        // We intentionally skip the zombie check here because the leader may be
        // a zombie while children in the group are still running.
        let ret = unsafe { libc::killpg(pgid, stop_signal) };
        if ret == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                debug!("process group {pgid} no longer exists");
                return Ok(false);
            }
            if err.raw_os_error() == Some(libc::EPERM) {
                return Err(miette::miette!(
                    "failed to send {signal_name} to process group {pgid}: permission denied"
                ));
            }
            warn!("failed to send {signal_name} to process group {pgid}: {err}");
        }

        // Wait for graceful shutdown: fast initial check then slower polling.
        // Per-daemon timeout overrides the global setting.
        let stop_timeout = stop_timeout.unwrap_or_else(|| settings().supervisor_stop_timeout());
        let fast_ms = 10u64;
        let slow_ms = 50u64;
        let total_ms = stop_timeout.as_millis().max(1) as u64;
        let fast_count = ((total_ms / fast_ms) as usize).min(10);
        let fast_total_ms = fast_ms * fast_count as u64;
        let remaining_ms = total_ms.saturating_sub(fast_total_ms);
        let slow_count = (remaining_ms / slow_ms) as usize;

        let fast_checks =
            std::iter::repeat_n(std::time::Duration::from_millis(fast_ms), fast_count);
        let slow_checks =
            std::iter::repeat_n(std::time::Duration::from_millis(slow_ms), slow_count);
        let mut elapsed_ms = 0u64;

        for sleep_duration in fast_checks.chain(slow_checks) {
            std::thread::sleep(sleep_duration);
            self.refresh_pids(&[pid]);
            elapsed_ms += sleep_duration.as_millis() as u64;
            if self.is_terminated_or_zombie(sysinfo::Pid::from_u32(pid)) {
                debug!("process group {pgid} terminated after {signal_name} ({elapsed_ms} ms)",);
                return Ok(true);
            }
        }

        // SIGKILL the entire process group as last resort
        warn!(
            "process group {pgid} did not respond to {signal_name} after {}ms, sending SIGKILL",
            stop_timeout.as_millis()
        );
        let ret = unsafe { libc::killpg(pgid, libc::SIGKILL) };
        if ret == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                warn!("failed to send SIGKILL to process group {pgid}: {err}");
            }
        }

        // Brief wait for SIGKILL to take effect
        std::thread::sleep(std::time::Duration::from_millis(100));
        Ok(true)
    }

    #[cfg(not(unix))]
    fn kill_process_group(
        &self,
        pid: u32,
        _stop_signal: i32,
        _stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        self.kill(pid, 0, None)
    }

    pub async fn kill_async(
        &self,
        pid: u32,
        stop_signal: i32,
        stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        tokio::task::spawn_blocking(move || PROCS.kill(pid, stop_signal, stop_timeout))
            .await
            .into_diagnostic()?
    }

    /// Kill a process with graceful shutdown strategy:
    /// 1. Send the configured stop signal and wait up to ~3s (10ms intervals for first 100ms, then 50ms intervals)
    /// 2. If still running, send SIGKILL to force termination
    ///
    /// This ensures fast-exiting processes don't wait unnecessarily,
    /// while stubborn processes eventually get forcefully terminated.
    ///
    /// Returns `Err` if the signal could not be sent (e.g. permission denied
    /// when targeting a process owned by another user/root).
    fn kill(
        &self,
        pid: u32,
        stop_signal: i32,
        stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        let sysinfo_pid = sysinfo::Pid::from_u32(pid);

        // Check if process exists or is a zombie (already terminated but not reaped)
        if self.is_terminated_or_zombie(sysinfo_pid) {
            return Ok(false);
        }

        debug!("killing process {pid}");

        #[cfg(windows)]
        {
            let _ = (stop_signal, stop_timeout);
            if let Some(process) = self.lock_system().process(sysinfo_pid) {
                process.kill();
                process.wait();
            }
            Ok(true)
        }

        #[cfg(unix)]
        {
            let signal_name = signal_name(stop_signal);
            // Send stop signal for graceful shutdown using libc::kill directly
            // so we can distinguish EPERM (permission denied) from ESRCH
            // (process already gone — possible in a narrow race window).
            debug!("sending {signal_name} to process {pid}");
            let ret = unsafe { libc::kill(pid as i32, stop_signal) };
            if ret == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ESRCH) {
                    debug!("process {pid} no longer exists");
                    return Ok(false);
                }
                if err.raw_os_error() == Some(libc::EPERM) {
                    return Err(miette::miette!(
                        "failed to send {signal_name} to process {pid}: permission denied"
                    ));
                }
                return Err(miette::miette!(
                    "failed to send {signal_name} to process {pid}: {err}"
                ));
            }

            // Fast check: 10ms intervals, then slower 50ms polling for stop_timeout.
            // Per-daemon timeout overrides the global setting.
            let stop_timeout = stop_timeout.unwrap_or_else(|| settings().supervisor_stop_timeout());
            let fast_ms = 10u64;
            let slow_ms = 50u64;
            let total_ms = stop_timeout.as_millis().max(1) as u64;
            let fast_count = ((total_ms / fast_ms) as usize).min(10);
            let fast_total_ms = fast_ms * fast_count as u64;
            let remaining_ms = total_ms.saturating_sub(fast_total_ms);
            let slow_count = (remaining_ms / slow_ms) as usize;

            for i in 0..fast_count {
                std::thread::sleep(std::time::Duration::from_millis(fast_ms));
                self.refresh_pids(&[pid]);
                if self.is_terminated_or_zombie(sysinfo_pid) {
                    debug!(
                        "process {pid} terminated after {signal_name} ({} ms)",
                        (i + 1) * fast_ms as usize
                    );
                    return Ok(true);
                }
            }

            // Slower check: 50ms intervals for the remainder of stop_timeout
            for i in 0..slow_count {
                std::thread::sleep(std::time::Duration::from_millis(slow_ms));
                self.refresh_pids(&[pid]);
                if self.is_terminated_or_zombie(sysinfo_pid) {
                    debug!(
                        "process {pid} terminated after {signal_name} ({} ms)",
                        fast_total_ms + (i + 1) as u64 * slow_ms
                    );
                    return Ok(true);
                }
            }

            // SIGKILL as last resort after stop_timeout
            warn!(
                "process {pid} did not respond to {signal_name} after {}ms, sending SIGKILL",
                stop_timeout.as_millis()
            );
            let ret = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            if ret == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    warn!("failed to send SIGKILL to process {pid}: {err}");
                }
            }

            // Brief wait for SIGKILL to take effect
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(true)
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

    /// Collect daemon PIDs from a StateFile and refresh only those.
    /// Avoids the cost of refreshing all system processes when we only
    /// need stats for managed daemons.
    pub(crate) fn refresh_daemon_pids(&self, state: &crate::state_file::StateFile) {
        let pids: Vec<u32> = state.daemons.values().filter_map(|d| d.pid).collect();
        if !pids.is_empty() {
            self.refresh_pids(&pids);
        }
    }

    /// Get aggregated stats for multiple process trees in a single pass.
    ///
    /// Builds the parent→children map once (O(N)) and then BFS-es from each
    /// root PID (O(D_i) per daemon). Total cost is O(N + ΣD_i) instead of
    /// O(D × N) when collecting stats for each daemon separately.
    pub fn get_batch_group_stats(&self, pids: &[u32]) -> Vec<(u32, Option<ProcessStats>)> {
        if pids.is_empty() {
            return Vec::new();
        }

        let system = self.lock_system();
        let processes = system.processes();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Build parent → children map once for all daemons
        let mut children_map: std::collections::HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> =
            std::collections::HashMap::new();
        for (child_pid, child) in processes {
            if let Some(ppid) = child.parent() {
                children_map.entry(ppid).or_default().push(*child_pid);
            }
        }

        pids.iter()
            .map(|&pid| {
                let root_pid = sysinfo::Pid::from_u32(pid);
                let Some(root) = processes.get(&root_pid) else {
                    return (pid, None);
                };

                let root_disk = root.disk_usage();
                let mut stats = ProcessStats {
                    cpu_percent: root.cpu_usage(),
                    memory_bytes: root.memory(),
                    uptime_secs: now.saturating_sub(root.start_time()),
                    disk_read_bytes: root_disk.read_bytes,
                    disk_write_bytes: root_disk.written_bytes,
                };

                // BFS from root_pid to find all descendants
                let mut queue = std::collections::VecDeque::new();
                if let Some(direct_children) = children_map.get(&root_pid) {
                    queue.extend(direct_children);
                }
                while let Some(child_pid) = queue.pop_front() {
                    if let Some(child) = processes.get(&child_pid) {
                        let disk = child.disk_usage();
                        stats.cpu_percent += child.cpu_usage();
                        stats.memory_bytes += child.memory();
                        stats.disk_read_bytes += disk.read_bytes;
                        stats.disk_write_bytes += disk.written_bytes;
                    }
                    if let Some(grandchildren) = children_map.get(&child_pid) {
                        queue.extend(grandchildren);
                    }
                }

                (pid, Some(stats))
            })
            .collect()
    }

    /// Get process-tree stats for multiple root PIDs, omitting roots that no longer exist.
    pub fn get_batch_tree_stats_map(&self, pids: &[u32]) -> HashMap<u32, ProcessStats> {
        self.get_batch_group_stats(pids)
            .into_iter()
            .filter_map(|(pid, stats)| stats.map(|stats| (pid, stats)))
            .collect()
    }

    /// Get process-tree stats (cpu%, memory bytes, uptime secs, disk I/O) for a given root PID.
    pub fn get_stats(&self, pid: u32) -> Option<ProcessStats> {
        self.get_batch_group_stats(&[pid])
            .into_iter()
            .next()
            .and_then(|(_, stats)| stats)
    }

    /// Get extended process information for a given PID
    pub fn get_extended_stats(&self, pid: u32) -> Option<ExtendedProcessStats> {
        let system = self.lock_system();
        let processes = system.processes();
        let root_pid = sysinfo::Pid::from_u32(pid);
        let p = processes.get(&root_pid)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let root_disk = p.disk_usage();
        let mut aggregate_stats = ProcessStats {
            cpu_percent: p.cpu_usage(),
            memory_bytes: p.memory(),
            uptime_secs: now.saturating_sub(p.start_time()),
            disk_read_bytes: root_disk.read_bytes,
            disk_write_bytes: root_disk.written_bytes,
        };

        let mut children_map: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
        for (child_pid, child) in processes {
            if let Some(ppid) = child.parent() {
                children_map.entry(ppid).or_default().push(*child_pid);
            }
        }

        let mut queue = std::collections::VecDeque::new();
        if let Some(direct_children) = children_map.get(&root_pid) {
            queue.extend(direct_children);
        }
        while let Some(child_pid) = queue.pop_front() {
            if let Some(child) = processes.get(&child_pid) {
                let disk = child.disk_usage();
                aggregate_stats.cpu_percent += child.cpu_usage();
                aggregate_stats.memory_bytes += child.memory();
                aggregate_stats.disk_read_bytes += disk.read_bytes;
                aggregate_stats.disk_write_bytes += disk.written_bytes;
            }
            if let Some(grandchildren) = children_map.get(&child_pid) {
                queue.extend(grandchildren);
            }
        }

        Some(ExtendedProcessStats {
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
            cpu_percent: aggregate_stats.cpu_percent,
            memory_bytes: aggregate_stats.memory_bytes,
            virtual_memory_bytes: p.virtual_memory(),
            uptime_secs: aggregate_stats.uptime_secs,
            start_time: p.start_time(),
            disk_read_bytes: aggregate_stats.disk_read_bytes,
            disk_write_bytes: aggregate_stats.disk_write_bytes,
            parent_pid: p.parent().map(|pp| pp.as_u32()),
            thread_count: p.tasks().map(|t| t.len()).unwrap_or(0),
            user_id: p.user_id().map(|u| u.to_string()),
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

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGTERM => "SIGTERM",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGUSR2 => "SIGUSR2",
        libc::SIGKILL => "SIGKILL",
        _ => "UNKNOWN",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let pid = self.0.id() as i32;
            // The test process is started in its own session, so PID == PGID.
            let _ = unsafe { libc::killpg(pid, libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }

    #[test]
    fn get_stats_includes_descendant_rss() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let parent = command.spawn().expect("failed to spawn process tree");
        let parent_pid = parent.id();
        let _parent = ChildGuard(parent);

        let procs = Procs::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut child_pids = Vec::new();
        while Instant::now() < deadline {
            procs.refresh_processes();
            child_pids = procs.all_children(parent_pid);
            if !child_pids.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !child_pids.is_empty(),
            "test process tree did not appear under parent pid {parent_pid}"
        );

        procs.refresh_processes();
        child_pids = procs.all_children(parent_pid);
        assert!(
            !child_pids.is_empty(),
            "test process tree disappeared under parent pid {parent_pid}"
        );
        let root_pid = sysinfo::Pid::from_u32(parent_pid);
        let direct_memory = {
            let system = procs.lock_system();
            system
                .process(root_pid)
                .expect("parent process should exist")
                .memory()
        };
        let descendant_memory = {
            let system = procs.lock_system();
            child_pids
                .iter()
                .filter_map(|pid| system.process(sysinfo::Pid::from_u32(*pid)))
                .map(|process| process.memory())
                .sum::<u64>()
        };
        assert!(
            descendant_memory > 0,
            "descendants {child_pids:?} should have nonzero RSS"
        );

        let stats = procs
            .get_stats(parent_pid)
            .expect("parent process should have aggregate stats");

        assert_eq!(
            stats.memory_bytes,
            direct_memory + descendant_memory,
            "get_stats should include descendant RSS for parent pid {parent_pid}; \
             descendants: {child_pids:?}, direct RSS: {direct_memory}, \
             descendant RSS: {descendant_memory}, reported RSS: {}",
            stats.memory_bytes
        );
    }
}
