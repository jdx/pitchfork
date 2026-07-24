use crate::Result;
#[cfg(unix)]
use crate::settings::settings;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::Mutex;
use sysinfo::ProcessesToUpdate;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Map from parent PID to its child PIDs.
type ParentToChildren = HashMap<u32, Vec<u32>>;

/// Map from PID to process name and optional executable path.
type ProcessNames = HashMap<u32, (String, Option<String>)>;

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
        // IMPORTANT: Do NOT call refresh_processes() or System::new_all() here.
        //
        // Both refresh the state of every process in the system, which takes
        // ~500ms on a typical machine. Since PROCS is a Lazy static, the first
        // access triggers this constructor — and `pitchfork cd` (which only
        // needs to check if the supervisor PID is alive) would block for that
        // duration on every directory change.
        //
        // See https://github.com/jdx/pitchfork/discussions/439
        //
        // Callers that need process info must call refresh_pids() (for specific
        // PIDs) or refresh_processes() (for full-system stats) explicitly.
        Self {
            system: Mutex::new(sysinfo::System::new()),
        }
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

    /// High-resolution kernel start token for the process.
    ///
    /// Combined with the PID this forms a stable identity for the lifetime of a
    /// process. Unlike sysinfo's seconds-since-epoch value, this preserves the
    /// native platform resolution so same-second PID reuse cannot compare equal.
    pub fn start_time(&self, pid: u32) -> Option<u64> {
        process_start_token(pid)
    }

    #[cfg(any(target_os = "linux", windows))]
    fn start_time_matches(&self, pid: u32, expected: u64) -> bool {
        self.start_time(pid) == Some(expected)
    }

    pub fn is_running(&self, pid: u32) -> bool {
        // Use kill(pid, 0) on Unix for an O(1) liveness check that does not
        // depend on the process cache being populated. This avoids the need
        // for a full process refresh just to check a single PID.
        // ESRCH = process does not exist; EPERM = process exists but owned
        // by another user (still "running" from our perspective).
        #[cfg(unix)]
        {
            unsafe {
                if libc::kill(pid as i32, 0) == 0 {
                    return true;
                }
                std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            }
        }
        #[cfg(not(unix))]
        {
            self.refresh_pids(&[pid]);
            self.lock_system()
                .process(sysinfo::Pid::from_u32(pid))
                .is_some()
        }
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
    /// Collect minimal process tree information in a single lock.
    ///
    /// Returns a map of parent PID → child PIDs and a map of PID → (name, exe).
    /// This avoids repeated mutex locking when traversing deep trees.
    pub fn collect_process_tree_info(&self) -> (ParentToChildren, ProcessNames) {
        let system = self.lock_system();
        let all = system.processes();
        let mut parent_to_children: ParentToChildren = HashMap::new();
        let mut process_info: ProcessNames = HashMap::new();

        for (pid, proc) in all {
            let pid_u32 = pid.as_u32();
            process_info.insert(
                pid_u32,
                (
                    proc.name().to_string_lossy().to_string(),
                    proc.exe().map(|e| e.to_string_lossy().to_string()),
                ),
            );

            if let Some(ppid) = proc.parent() {
                parent_to_children
                    .entry(ppid.as_u32())
                    .or_default()
                    .push(pid_u32);
            }
        }

        (parent_to_children, process_info)
    }
    pub async fn kill_process_group_async(
        &self,
        pid: u32,
        stop_signal: i32,
        stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        tokio::task::spawn_blocking(move || {
            PROCS.kill_process_group(pid, stop_signal, stop_timeout, None)
        })
        .await
        .into_diagnostic()?
    }

    /// Kill a process group only while its leader still has `expected_start_time`.
    ///
    /// Identity is refreshed inside the blocking kill operation, immediately
    /// before signaling. Linux holds a pidfd and Windows holds an open process
    /// handle across termination so the validated PID cannot be recycled.
    /// Unix platforms without a durable process handle fail closed.
    pub async fn kill_process_group_if_start_time_matches_async(
        &self,
        pid: u32,
        expected_start_time: u64,
        stop_signal: i32,
        stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        tokio::task::spawn_blocking(move || {
            PROCS.kill_process_group(pid, stop_signal, stop_timeout, Some(expected_start_time))
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
        expected_start_time: Option<u64>,
    ) -> Result<bool> {
        let pgid = pid as i32;
        let signal_name = signal_name(stop_signal);

        #[cfg(target_os = "linux")]
        if let Some(expected) = expected_start_time {
            return self.kill_process_group_with_pidfds(pid, expected, stop_signal, stop_timeout);
        }

        // A start-time check alone cannot prevent the numeric PID/PGID from
        // being recycled before killpg. Linux closes that race with a pidfd;
        // other Unix platforms must refuse identity-checked termination.
        #[cfg(not(target_os = "linux"))]
        if expected_start_time.is_some() {
            warn!(
                "cannot securely identify process group {pgid} on this platform; refusing to signal it"
            );
            return Ok(false);
        }

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

    #[cfg(target_os = "linux")]
    fn kill_process_group_with_pidfds(
        &self,
        pid: u32,
        expected_start_time: u64,
        _stop_signal: i32,
        _stop_timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        let leader = match open_pidfd(pid) {
            Ok(pidfd) => pidfd,
            Err(err) => {
                warn!("cannot securely identify process group {pid}: {err}");
                return Ok(false);
            }
        };
        if !self.start_time_matches(pid, expected_start_time) {
            debug!("process group {pid} leader identity changed before signaling");
            return Ok(false);
        }

        let mut members = vec![(pid, leader)];
        if let Err(err) = stop_pidfds(&members) {
            let _ = signal_pidfds(&members, libc::SIGCONT, "SIGCONT");
            return Err(err);
        }
        if !pidfd_is_running(&members[0].1) {
            debug!("process group {pid} leader exited before it could be frozen");
            return Ok(false);
        }

        // Stop newly discovered members before rescanning. Once a scan adds
        // nothing, every process capable of forking into this group is frozen,
        // so the pinned set is complete and the PGID cannot be recycled.
        loop {
            let known_members = members.len();
            let added = match extend_process_group_pidfds(pid as i32, &mut members) {
                Ok(added) => added,
                Err(err) => {
                    let _ = signal_pidfds(&members, libc::SIGCONT, "SIGCONT");
                    return Err(miette::miette!(
                        "failed to scan pinned process group {pid}: {err}"
                    ));
                }
            };
            if added == 0 {
                break;
            }
            if let Err(err) = stop_pidfds(&members[known_members..]) {
                let _ = signal_pidfds(&members, libc::SIGCONT, "SIGCONT");
                return Err(err);
            }
        }

        warn!(
            "force-terminating {} pinned orphan process(es) in group {pid}",
            members.len()
        );
        if let Err(err) = signal_pidfds(&members, libc::SIGKILL, "SIGKILL") {
            let _ = signal_pidfds(&members, libc::SIGCONT, "SIGCONT");
            return Err(err);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        Ok(true)
    }

    #[cfg(not(unix))]
    fn kill_process_group(
        &self,
        pid: u32,
        _stop_signal: i32,
        _stop_timeout: Option<std::time::Duration>,
        expected_start_time: Option<u64>,
    ) -> Result<bool> {
        // Keep the Windows process object alive through taskkill so its
        // numeric PID cannot be recycled after identity validation.
        #[cfg(windows)]
        let _identity_handle = if let Some(expected) = expected_start_time {
            let handle = match open_process_handle(pid) {
                Ok(handle) => handle,
                Err(err) => {
                    warn!("cannot securely identify process {pid}: {err}");
                    return Ok(false);
                }
            };
            if process_start_token_from_handle(handle.0) != Some(expected) {
                debug!("process {pid} identity changed before taskkill");
                return Ok(false);
            }
            Some(handle)
        } else {
            None
        };

        #[cfg(not(windows))]
        if let Some(expected) = expected_start_time
            && !self.start_time_matches(pid, expected)
        {
            debug!("process {pid} identity changed before termination");
            return Ok(false);
        }

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

        debug!("killing process {pid}");

        #[cfg(windows)]
        {
            let _ = (stop_signal, stop_timeout);
            // Use taskkill /F /T to kill the entire process tree.
            // sysinfo's process.kill() only kills the main process, leaving
            // child processes (e.g. python3 spawned by sh -c) orphaned and
            // still holding ports. The /T flag kills all descendant processes.
            let output = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID"])
                .arg(pid.to_string())
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .output();
            let taskkill_succeeded = match output {
                Ok(o) if o.status.success() => {
                    debug!("taskkill /F /T /PID {pid} succeeded");
                    true
                }
                Ok(o) => {
                    debug!(
                        "taskkill /F /T /PID {pid} exited with status {}: {}",
                        o.status,
                        String::from_utf8_lossy(&o.stderr).trim()
                    );
                    false
                }
                Err(e) => {
                    debug!("failed to spawn taskkill for pid {pid}: {e}");
                    false
                }
            };
            // Brief sleep to let the OS signal the process handle, giving
            // tokio's child.wait() in the monitor task a chance to detect
            // the exit and fire on_stop/on_exit hooks.
            std::thread::sleep(std::time::Duration::from_millis(200));
            if !taskkill_succeeded && self.is_running(pid) {
                return Err(miette::miette!(
                    "taskkill failed and process {pid} is still running"
                ));
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
    #[cfg(unix)]
    fn is_terminated_or_zombie(&self, sysinfo_pid: sysinfo::Pid) -> bool {
        let system = self.lock_system();
        match system.process(sysinfo_pid) {
            None => true,
            Some(process) => {
                matches!(process.status(), sysinfo::ProcessStatus::Zombie)
            }
        }
    }

    pub(crate) fn refresh_processes(&self) {
        let mut system = self.lock_system();
        system.refresh_processes(ProcessesToUpdate::All, true);
        // On Windows, refresh_processes() does not update CPU usage.
        // sysinfo requires a separate refresh_cpu_usage() call to compute
        // the CPU delta between two samples. The first call stores the
        // baseline; subsequent calls return the actual percentage.
        #[cfg(windows)]
        system.refresh_cpu_usage();
    }

    /// Refresh only specific PIDs instead of all processes.
    /// More efficient when you only need to check a small set of known PIDs.
    pub(crate) fn refresh_pids(&self, pids: &[u32]) {
        let sysinfo_pids: Vec<sysinfo::Pid> =
            pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        self.lock_system()
            .refresh_processes(ProcessesToUpdate::Some(&sysinfo_pids), true);
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
            // Skip Linux userland threads: they report the same memory as their parent process,
            // so including them would cause massive double-counting.
            if child.thread_kind().is_some() {
                continue;
            }
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
    /// Refresh the process tree, then call [`Self::get_batch_group_stats`].
    ///
    /// Convenience wrapper that guarantees `get_batch_group_stats` sees a
    /// fresh snapshot of /proc (or its equivalent).  Returns a PID →
    /// [`ProcessStats`] map so callers do not have to repeat the same
    /// `filter_map`/`collect` boilerplate.
    pub fn refresh_and_get_batch_stats(&self, pids: &[u32]) -> HashMap<u32, ProcessStats> {
        self.refresh_processes();
        self.get_batch_group_stats(pids)
            .into_iter()
            .filter_map(|(pid, stats)| stats.map(|s| (pid, s)))
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
            status: format!("{:?}", p.status()),
            cpu_percent: aggregate_stats.cpu_percent,
            memory_bytes: aggregate_stats.memory_bytes,
            virtual_memory_bytes: p.virtual_memory(),
            uptime_secs: aggregate_stats.uptime_secs,
            thread_count: p.tasks().map(|t| t.len()).unwrap_or(0),
        })
    }
}

#[cfg(target_os = "linux")]
fn process_start_token(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let command_end = stat.rfind(')')?;
    // Fields after the command start at field 3 (state); starttime is field 22.
    stat.get(command_end + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
fn process_start_token(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    info.pbi_start_tvsec
        .checked_mul(1_000_000)?
        .checked_add(info.pbi_start_tvusec)
}

#[cfg(windows)]
fn process_start_token(pid: u32) -> Option<u64> {
    let handle = open_process_handle(pid).ok()?;
    process_start_token_from_handle(handle.0)
}

#[cfg(windows)]
fn process_start_token_from_handle(handle: HANDLE) -> Option<u64> {
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }

    Some((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

#[cfg(windows)]
struct ProcessHandle(HANDLE);

#[cfg(windows)]
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn open_process_handle(pid: u32) -> std::io::Result<ProcessHandle> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ProcessHandle(handle))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn process_start_token(pid: u32) -> Option<u64> {
    let mut system = sysinfo::System::new();
    let sysinfo_pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[sysinfo_pid]), true);
    system
        .process(sysinfo_pid)
        .map(|process| process.start_time())
}

#[cfg(target_os = "linux")]
fn open_pidfd(pid: u32) -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

#[cfg(target_os = "linux")]
fn pidfd_is_running(pidfd: &OwnedFd) -> bool {
    match try_pidfd_is_running(pidfd) {
        Ok(running) => running,
        Err(err) => {
            warn!("failed to poll pidfd {}: {err}", pidfd.as_raw_fd());
            true
        }
    }
}

#[cfg(target_os = "linux")]
fn try_pidfd_is_running(pidfd: &OwnedFd) -> std::io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(result == 0)
}

#[cfg(target_os = "linux")]
fn signal_pidfds(members: &[(u32, OwnedFd)], signal: i32, signal_name: &str) -> Result<()> {
    for (pid, pidfd) in members {
        if !pidfd_is_running(pidfd) {
            continue;
        }
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                continue;
            }
            return Err(miette::miette!(
                "failed to send {signal_name} to pinned process {pid}: {err}"
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop_pidfds(members: &[(u32, OwnedFd)]) -> Result<()> {
    signal_pidfds(members, libc::SIGSTOP, "SIGSTOP")?;
    for _ in 0..200 {
        if members.iter().all(|(pid, pidfd)| {
            !pidfd_is_running(pidfd) || matches!(linux_process_state(*pid), Some('T' | 't'))
        }) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Err(miette::miette!(
        "timed out while freezing orphan process group"
    ))
}

#[cfg(target_os = "linux")]
fn extend_process_group_pidfds(
    pgid: i32,
    members: &mut Vec<(u32, OwnedFd)>,
) -> std::io::Result<usize> {
    let entries = std::fs::read_dir("/proc")?;
    let mut added = 0;
    for entry in entries {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(observed_identity) = linux_process_identity(pid) else {
            continue;
        };
        if observed_identity.0 != pgid {
            continue;
        }
        let mut already_pinned = false;
        for (known_pid, pidfd) in members.iter() {
            if *known_pid == pid && try_pidfd_is_running(pidfd)? {
                already_pinned = true;
                break;
            }
        }
        if already_pinned {
            continue;
        }

        let pidfd = match open_pidfd(pid) {
            Ok(pidfd) => pidfd,
            Err(err) if err.raw_os_error() == Some(libc::ESRCH) => continue,
            Err(err) => return Err(err),
        };
        if linux_process_identity(pid) != Some(observed_identity) {
            return Err(std::io::Error::other(format!(
                "process {pid} identity changed while pinning group {pgid}"
            )));
        }
        members.push((pid, pidfd));
        added += 1;
    }
    Ok(added)
}

#[cfg(target_os = "linux")]
fn linux_process_identity(pid: u32) -> Option<(i32, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let command_end = stat.rfind(')')?;
    let fields: Vec<_> = stat.get(command_end + 1..)?.split_whitespace().collect();
    // Fields after the command start at field 3. pgrp is field 5 and the
    // scheduler-tick start token is field 22.
    Some((fields.get(2)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

#[cfg(target_os = "linux")]
fn linux_process_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let command_end = stat.rfind(')')?;
    stat.get(command_end + 1..)?
        .split_whitespace()
        .next()?
        .chars()
        .next()
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

#[derive(Debug, Clone)]
pub struct ExtendedProcessStats {
    pub name: String,
    pub status: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub virtual_memory_bytes: u64,
    pub uptime_secs: u64,
    pub thread_count: usize,
}

fn format_bytes(bytes: u64) -> String {
    humanbyte::to_string(bytes, humanbyte::Format::IEC)
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
    format!("{}/s", humanbyte::to_string(bytes, humanbyte::Format::IEC))
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

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn process_start_time_check_rejects_mismatch() {
        let procs = Procs::new();
        let pid = std::process::id();
        procs.refresh_pids(&[pid]);
        let actual = procs
            .start_time(pid)
            .expect("current process should have a start time");

        assert_ne!(procs.start_time(pid), Some(actual.saturating_add(1)));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50.0 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
        // rolls over past GiB instead of showing e.g. "1100.0GB"
        assert_eq!(format_bytes(1100 * 1024 * 1024 * 1024), "1.1 TiB");
    }

    #[test]
    fn test_format_bytes_per_sec() {
        assert_eq!(format_bytes_per_sec(512), "512 B/s");
        assert_eq!(format_bytes_per_sec(1536), "1.5 KiB/s");
        assert_eq!(format_bytes_per_sec(2 * 1024 * 1024), "2.0 MiB/s");
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

    #[tokio::test]
    async fn orphan_identity_checked_group_kill_rejects_mismatch() {
        let mut command = Command::new("sleep");
        command
            .arg("30")
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

        let child = command.spawn().expect("failed to spawn test process");
        let pid = child.id();
        let _child = ChildGuard(child);

        PROCS.refresh_pids(&[pid]);
        let actual_start_time = PROCS
            .start_time(pid)
            .expect("test process should have a start time");

        let killed = PROCS
            .kill_process_group_if_start_time_matches_async(
                pid,
                actual_start_time.saturating_add(1),
                libc::SIGTERM,
                Some(Duration::from_millis(100)),
            )
            .await
            .expect("identity-checked kill should not error");

        assert!(!killed);
        assert!(PROCS.is_running(pid), "mismatched process must survive");
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn orphan_identity_checked_group_kill_fails_closed_without_pidfd() {
        let mut command = Command::new("sleep");
        command
            .arg("30")
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

        let child = command.spawn().expect("failed to spawn test process");
        let pid = child.id();
        let _child = ChildGuard(child);

        PROCS.refresh_pids(&[pid]);
        let actual_start_time = PROCS
            .start_time(pid)
            .expect("test process should have a start time");

        let killed = PROCS
            .kill_process_group_if_start_time_matches_async(
                pid,
                actual_start_time,
                libc::SIGTERM,
                Some(Duration::from_millis(100)),
            )
            .await
            .expect("identity-checked kill should not error");

        assert!(!killed);
        assert!(
            PROCS.is_running(pid),
            "process must survive when identity cannot be pinned"
        );
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
