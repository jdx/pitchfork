//! Keep descriptors this process did not mean to share out of its children.
//!
//! A child inherits, across exec, every descriptor of its parent that lacks
//! `FD_CLOEXEC`. Such descriptors come from several places:
//!
//! - On macOS, which has no `SOCK_CLOEXEC`, a socket is created in one call
//!   and marked close-on-exec in another. A child forked by another thread
//!   between the two calls keeps the socket. The supervisor's port check is
//!   one such socket: it binds a daemon's port, then drops it, and a log sink
//!   spawned in that window kept the port bound for as long as it ran, so the
//!   daemon itself could not bind it.
//! - `openpty(3)` does not set `FD_CLOEXEC`, so every child spawned while a
//!   `pty = true` daemon runs would otherwise get that daemon's PTY.
//! - Callers hand the CLI descriptors without `O_CLOEXEC` (bats' fd 3, pipes
//!   from a wrapping script). The long-lived background supervisor would hold
//!   them open forever, so whoever waits for EOF on the pipe (bats, `$(...)`,
//!   CI log capture) hangs even after the CLI has exited.
//!
//! `CloseInheritedFds` marks every descriptor above stderr close-on-exec in
//! the forked child, after stdio is set up and before exec, so the child
//! starts with its stdio and nothing else, whatever other threads were doing
//! at the time. It marks them rather than closing them outright so that std's
//! internal exec-status pipe (already `FD_CLOEXEC`) keeps working and exec
//! failures are still reported to the parent.

/// Start the child with only its stdio: no other descriptor of this process
/// survives its exec.
///
/// Implemented for both `std::process::Command` and `tokio::process::Command`,
/// and returns `&mut Self` so it drops into the existing fluent chains. The
/// Windows impls are no-ops, which keeps the call sites free of `cfg`: std
/// creates its handles non-inheritable atomically, so the race this guards
/// against on Unix does not arise there.
///
/// On Unix this adds a `pre_exec` hook, so the command is spawned with
/// fork/exec rather than `posix_spawn`.
pub(crate) trait CloseInheritedFds {
    /// Mark every descriptor above stderr close-on-exec in the child before
    /// it execs. A no-op on Windows.
    fn close_inherited_fds(&mut self) -> &mut Self;
}

#[cfg(unix)]
impl CloseInheritedFds for std::process::Command {
    /// Install a `pre_exec` hook that marks the child's inherited descriptors
    /// close-on-exec, using a buffer and bound prepared here in the parent.
    fn close_inherited_fds(&mut self) -> &mut Self {
        use std::os::unix::process::CommandExt;
        let mut fds = InheritedFds::new();
        // SAFETY: mark_cloexec only issues syscalls, which are
        // async-signal-safe, and does not allocate.
        unsafe {
            self.pre_exec(move || {
                fds.mark_cloexec();
                Ok(())
            })
        }
    }
}

#[cfg(unix)]
impl CloseInheritedFds for tokio::process::Command {
    /// Install the same `pre_exec` hook as the `std::process::Command` impl.
    fn close_inherited_fds(&mut self) -> &mut Self {
        let mut fds = InheritedFds::new();
        // SAFETY: as for std::process::Command above.
        unsafe {
            self.pre_exec(move || {
                fds.mark_cloexec();
                Ok(())
            })
        }
    }
}

#[cfg(windows)]
impl CloseInheritedFds for std::process::Command {
    /// No-op: std creates its handles non-inheritable.
    fn close_inherited_fds(&mut self) -> &mut Self {
        self
    }
}

#[cfg(windows)]
impl CloseInheritedFds for tokio::process::Command {
    /// No-op: std creates its handles non-inheritable.
    fn close_inherited_fds(&mut self) -> &mut Self {
        self
    }
}

/// What the forked child needs to find its inherited descriptors, prepared in
/// the parent: sysconf, reading `/dev/fd`, and allocating are not
/// async-signal-safe, so none of them can happen in the child.
#[cfg(unix)]
struct InheritedFds {
    /// Upper bound (exclusive) for [`cloexec_fd_scan`], the fallback used when
    /// the platform cannot list or range-mark descriptors.
    scan_end: libc::c_int,
    /// Buffer for the child's own descriptor listing.
    #[cfg(target_os = "macos")]
    listing: Vec<libc::proc_fdinfo>,
}

#[cfg(unix)]
impl InheritedFds {
    /// Size the listing buffer and the scan bound from the descriptors open
    /// now. Runs in the parent, before the fork.
    fn new() -> Self {
        // SAFETY: sysconf has no preconditions.
        let open_max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        // RLIMIT_NOFILE alone is not a safe bound: a caller can open a
        // high-numbered descriptor and then lower the limit before running us.
        // Also list the fds that are actually open (both macOS and Linux
        // provide /dev/fd) so the scan reaches every one of them.
        let open_fds: Vec<libc::c_int> = std::fs::read_dir("/dev/fd")
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        Self {
            // Room for descriptors other threads open before the fork. A
            // listing that fills the buffer falls back to the scan.
            #[cfg(target_os = "macos")]
            listing: vec![
                libc::proc_fdinfo {
                    proc_fd: 0,
                    proc_fdtype: 0,
                };
                open_fds.len() * 2 + 64
            ],
            scan_end: fd_scan_end(open_max, open_fds),
        }
    }

    /// Runs in the forked child. Must stay async-signal-safe: no allocation,
    /// no locks, syscalls only.
    fn mark_cloexec(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            // SAFETY: close_range takes plain integer arguments.
            // CLOSE_RANGE_CLOEXEC needs Linux 5.11; older kernels return
            // ENOSYS/EINVAL and we fall through to the scan below.
            let ret = unsafe {
                libc::syscall(
                    libc::SYS_close_range,
                    3 as libc::c_uint,
                    libc::c_uint::MAX,
                    libc::CLOSE_RANGE_CLOEXEC as libc::c_uint,
                )
            };
            if ret == 0 {
                return;
            }
        }
        #[cfg(target_os = "macos")]
        if self.mark_listed() {
            return;
        }
        cloexec_fd_scan(self.scan_end);
    }

    /// Mark the descriptors the kernel lists for this process, rather than
    /// probing every number below the fd limit: at `ulimit -n 1048576` the
    /// scan costs about 100ms per spawn, the listing well under 1ms.
    /// Returns false when the listing failed or may have been cut short.
    #[cfg(target_os = "macos")]
    fn mark_listed(&mut self) -> bool {
        let entry_size = std::mem::size_of::<libc::proc_fdinfo>();
        let Ok(capacity) = libc::c_int::try_from(self.listing.len() * entry_size) else {
            return false;
        };
        // SAFETY: the buffer is valid for `capacity` bytes. proc_pidinfo is a
        // thin wrapper around the proc_info syscall.
        let filled = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                self.listing.as_mut_ptr().cast(),
                capacity,
            )
        };
        if filled <= 0 || filled >= capacity {
            return false;
        }
        for info in &self.listing[..filled as usize / entry_size] {
            if info.proc_fd >= 3 {
                set_cloexec(info.proc_fd);
            }
        }
        true
    }
}

/// Upper bound (exclusive) for [`cloexec_fd_scan`]: the fd limit `open_max`,
/// capped, but always past the highest descriptor in `open_fds`.
#[cfg(unix)]
fn fd_scan_end(
    open_max: libc::c_long,
    open_fds: impl IntoIterator<Item = libc::c_int>,
) -> libc::c_int {
    // -1 means "no limit"; cap the scan so an unlimited or huge RLIMIT_NOFILE
    // doesn't make spawning noticeably slow.
    let limit = if open_max <= 0 {
        1 << 16
    } else {
        open_max.min(1 << 20) as libc::c_int
    };
    let past_highest_open = open_fds
        .into_iter()
        .max()
        .map_or(0, |fd| fd.saturating_add(1));
    limit.max(past_highest_open)
}

/// Mark fds `3..scan_end` close-on-exec one at a time. Async-signal-safe.
#[cfg(unix)]
fn cloexec_fd_scan(scan_end: libc::c_int) {
    for fd in 3..scan_end {
        set_cloexec(fd);
    }
}

/// Set `FD_CLOEXEC` on `fd` if it is open and lacks it. Async-signal-safe.
#[cfg(unix)]
fn set_cloexec(fd: libc::c_int) {
    // SAFETY: fcntl on an arbitrary fd number is safe; unused numbers just
    // fail with EBADF.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, MutexGuard, PoisonError};

    /// Held while a test has an inheritable descriptor open, or spawns a
    /// child without `close_inherited_fds`. Without it, the control child of
    /// one test inherits the descriptor of another and keeps it open.
    static INHERITABLE: Mutex<()> = Mutex::new(());

    /// Lock [`INHERITABLE`], recovering it if an earlier test panicked.
    fn inheritable() -> MutexGuard<'static, ()> {
        INHERITABLE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A pipe without O_CLOEXEC, like bats' fd 3 or a wrapper script's pipe.
    /// The caller closes both ends.
    fn inheritable_pipe() -> [libc::c_int; 2] {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        fds
    }

    /// Close both ends of a pipe from [`inheritable_pipe`].
    fn close_pipe(fds: [libc::c_int; 2]) {
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    /// A command whose exit status says whether `fd` is open in the child.
    fn sees_fd(fd: libc::c_int) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("[ -e /dev/fd/{fd} ]"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    /// The scan must reach descriptors above a lowered fd limit, and stay
    /// bounded when the limit is unlimited or huge.
    #[test]
    fn fd_scan_end_reaches_open_fds_above_the_fd_limit() {
        // A caller opened fd 900, then lowered RLIMIT_NOFILE to 256.
        assert_eq!(fd_scan_end(256, [0, 1, 2, 900]), 901);
        assert_eq!(fd_scan_end(256, [0, 1, 2, 10]), 256);
        assert_eq!(fd_scan_end(-1, []), 1 << 16);
        assert_eq!(fd_scan_end(libc::c_long::MAX, []), 1 << 20);
    }

    /// A std child spawned through `close_inherited_fds` does not see an
    /// inheritable pipe that a plain spawn passes on.
    #[test]
    fn close_inherited_fds_hides_inherited_fds_from_children() {
        let _guard = inheritable();
        let pipe = inheritable_pipe();
        let without = sees_fd(pipe[1]).status().unwrap().success();
        let with = sees_fd(pipe[1])
            .close_inherited_fds()
            .status()
            .unwrap()
            .success();
        close_pipe(pipe);
        assert!(without, "control: child should inherit fd {}", pipe[1]);
        assert!(!with, "fd {} leaked past close_inherited_fds", pipe[1]);
    }

    /// Exercises the fcntl fallback directly: on Linux >= 5.11 and macOS the
    /// spawn path normally never reaches it.
    #[test]
    fn cloexec_fd_scan_hides_inherited_fds_from_children() {
        let _guard = inheritable();
        let pipe = inheritable_pipe();
        let scan_end = InheritedFds::new().scan_end;
        let mut cmd = sees_fd(pipe[1]);
        unsafe {
            cmd.pre_exec(move || {
                cloexec_fd_scan(scan_end);
                Ok(())
            });
        }
        let with_scan = cmd.status().unwrap().success();
        close_pipe(pipe);
        assert!(!with_scan, "fd {} leaked past cloexec_fd_scan", pipe[1]);
    }

    /// The listing must succeed in the child, or every spawn on macOS pays for
    /// the full scan.
    #[cfg(target_os = "macos")]
    #[test]
    fn mark_listed_hides_inherited_fds_from_children() {
        let _guard = inheritable();
        let pipe = inheritable_pipe();
        let mut fds = InheritedFds::new();
        let mut cmd = sees_fd(pipe[1]);
        unsafe {
            cmd.pre_exec(move || {
                if fds.mark_listed() {
                    Ok(())
                } else {
                    Err(std::io::Error::other("proc_pidinfo listing failed"))
                }
            });
        }
        let status = cmd.status();
        close_pipe(pipe);
        assert!(
            !status.unwrap().success(),
            "fd {} leaked past mark_listed",
            pipe[1]
        );
    }

    /// The port-check race on macOS: a child spawned between socket() and the
    /// call that sets FD_CLOEXEC inherits the probe's socket, and once the probe
    /// has dropped it, the child holds the port.
    #[test]
    fn close_inherited_fds_hides_a_listening_socket_from_tokio_children() {
        use std::os::fd::AsRawFd;

        let _guard = inheritable();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let fd = listener.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            assert_eq!(libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC), 0);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let child_sees_socket = |close: bool| {
            let mut cmd = tokio::process::Command::from(sees_fd(fd));
            if close {
                cmd.close_inherited_fds();
            }
            runtime
                .block_on(async { cmd.status().await })
                .unwrap()
                .success()
        };
        let without = child_sees_socket(false);
        let with = child_sees_socket(true);
        drop(listener);
        assert!(without, "control: child should inherit the socket");
        assert!(!with, "listening socket leaked past close_inherited_fds");
    }
}
