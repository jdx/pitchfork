//! A job object for each daemon on Windows, so its whole process tree can be
//! stopped.
//!
//! Windows records only a parent PID, so a process tree can be walked from the
//! daemon only while every process on the way is alive: `taskkill /T` misses a
//! process whose parent has exited, and a daemon that exits on Ctrl+C leaves no
//! root to walk from at all. A job object has no such gap. Every process the
//! daemon starts joins its job, whatever becomes of its parent, and
//! terminating the job ends them all at once.
//!
//! The daemon is started suspended and assigned to the job before it runs, so
//! nothing it starts can escape by starting first. The job allows breakaway:
//! a program that starts a child with `CREATE_BREAKAWAY_FROM_JOB` would
//! otherwise fail to start it, so such a child leaves the job and is not
//! stopped with the daemon.
//!
//! The job is named after the daemon's PID and start time, and the daemon holds
//! a handle to it that it never uses, which keeps the name alive while the
//! daemon runs. The supervisor keeps no handle, so a restarted supervisor finds
//! the job of a daemon it adopted the same way. A stop opens the job while the
//! daemon is still running, and the handle it holds keeps the job reachable
//! after the daemon itself has exited.

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, OpenJobObjectW,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, GetCurrentProcess, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

/// `JOB_OBJECT_TERMINATE`, the access right `TerminateJobObject` needs. It lives
/// in windows-sys' `SystemServices`, too large a feature for one constant.
const JOB_OBJECT_TERMINATE: u32 = 0x0008;

/// The exit code a process ended by its job's termination reports.
const TERMINATED_EXIT_CODE: u32 = 1;

/// Make `cmd` start its process suspended, for [`contain_and_resume`] to put
/// in a job before it runs.
///
/// Replaces the command's creation flags, so it is called after
/// `hide_console_window`, whose flag it keeps.
pub(crate) fn start_suspended(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(crate::shell::console_creation_flags() | CREATE_SUSPENDED);
}

/// Put the suspended process `pid`, whose handle is `process`, in a job of its
/// own, then let it run.
///
/// Best effort: a process that could not be put in a job still runs, and is
/// stopped by walking its tree as before.
pub(crate) fn contain_and_resume(process: HANDLE, pid: u32) {
    if let Err(e) = contain(process, pid) {
        debug!("daemon process {pid} runs without a job object: {e}");
    }
    if let Err(e) = resume(pid) {
        // A daemon that never runs is worse than one without a job, so this
        // is the one failure worth a warning.
        warn!("failed to resume daemon process {pid}: {e}");
    }
}

fn contain(process: HANDLE, pid: u32) -> std::io::Result<()> {
    let name = job_name(pid, process)?;
    let job = OwnedHandle(unsafe { CreateJobObjectW(std::ptr::null(), name.as_ptr()) });
    if job.0.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    let ok = unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { AssignProcessToJobObject(job.0, process) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // The daemon's own handle keeps the job's name alive once this one is
    // closed. It is not inheritable, so the daemon's children do not get it.
    let mut in_daemon: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            job.0,
            process,
            &mut in_daemon,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Resume every thread of the suspended process `pid`: its main thread, the
/// only one a process started suspended has.
fn resume(pid: u32) -> std::io::Result<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let snapshot = OwnedHandle(snapshot);
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut resumed = 0;
    let mut more = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            let thread =
                OwnedHandle(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) });
            if thread.0.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { ResumeThread(thread.0) } == u32::MAX {
                return Err(std::io::Error::last_os_error());
            }
            resumed += 1;
        }
        more = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(std::io::Error::other(format!(
            "no thread of process {pid} found"
        )));
    }
    Ok(())
}

/// The job of a daemon pitchfork started, opened while it can still be found.
pub(crate) struct DaemonJob(OwnedHandle);

impl DaemonJob {
    /// The job of the running process `pid`, whose handle is `process`, if it
    /// has one: a daemon started before job objects were used, or one that
    /// could not be put in a job, has none.
    pub(crate) fn open(pid: u32, process: HANDLE) -> Option<Self> {
        let name = job_name(pid, process).ok()?;
        let job = unsafe { OpenJobObjectW(JOB_OBJECT_TERMINATE, 0, name.as_ptr()) };
        (!job.is_null()).then(|| Self(OwnedHandle(job)))
    }

    /// Terminate every process still in the job.
    pub(crate) fn terminate(&self) -> bool {
        let ok = unsafe { TerminateJobObject(self.0.0, TERMINATED_EXIT_CODE) } != 0;
        if !ok {
            debug!(
                "failed to terminate job object: {}",
                std::io::Error::last_os_error()
            );
        }
        ok
    }
}

/// The name of the job for the process `pid` started at the time `process`
/// reports, NUL-terminated. The start time tells a reused PID apart.
fn job_name(pid: u32, process: HANDLE) -> std::io::Result<Vec<u16>> {
    let start = crate::procs::process_start_token_from_handle(process)
        .ok_or_else(std::io::Error::last_os_error)?;
    Ok(format_job_name(pid, start)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect())
}

fn format_job_name(pid: u32, start: u64) -> String {
    format!("Local\\pitchfork-daemon-{pid}-{start}")
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

// Handles are process-wide values, safe to use from any thread.
unsafe impl Send for OwnedHandle {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_name_is_session_local_and_tells_pid_reuse_apart() {
        assert_eq!(
            format_job_name(1234, 133_000_000_000_000_000),
            "Local\\pitchfork-daemon-1234-133000000000000000"
        );
        assert_ne!(format_job_name(1234, 1), format_job_name(1234, 2));
    }

    /// How many processes are in the job `job`, opened with query access.
    fn active_processes(job: &OwnedHandle) -> u32 {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            QueryInformationJobObject(
                job.0,
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
        info.ActiveProcesses
    }

    #[test]
    fn terminating_the_job_ends_a_child_whose_parent_has_exited() {
        /// `JOB_OBJECT_QUERY`, for reading the job's process count.
        const JOB_OBJECT_QUERY: u32 = 0x0004;

        // `cmd /c start /b` leaves a ping running after cmd itself exits, so
        // the ping's parent is gone, the case walking the tree misses.
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/c", "start", "/b", "ping", "-n", "30", "127.0.0.1"]);
        start_suspended(&mut cmd);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut child = cmd
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id().unwrap();
            let process = child.raw_handle().unwrap() as HANDLE;
            contain_and_resume(process, pid);
            let job = DaemonJob::open(pid, process).expect("the daemon has a job");
            let name = job_name(pid, process).unwrap();
            let query = OwnedHandle(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) });
            assert!(!query.0.is_null());
            child.wait().await.unwrap();

            // cmd has exited; the ping it started is still in the job.
            assert!(active_processes(&query) >= 1, "ping is not in the job");

            assert!(job.terminate());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while active_processes(&query) > 0 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the ping outlived its job"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
    }
}
