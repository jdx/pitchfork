//! PTY (pseudo-terminal) allocation for daemon processes.
//!
//! When `pty = true` is configured for a daemon, we allocate a PTY pair
//! so that the daemon process runs with a controlling terminal. This is
//! useful for programs that check `isatty()` or behave differently when
//! connected to a terminal (e.g., colored output, interactive prompts).

use std::os::fd::{FromRawFd, OwnedFd};

/// A PTY master/slave pair.
pub struct PtyPair {
    /// The master side — read from this to get daemon output.
    pub master: OwnedFd,
    /// The slave side — pass this to the child process as its terminal.
    pub slave: OwnedFd,
}

/// Allocate a new PTY pair using `openpty(3)`.
///
/// Note: `openpty(3)` does *not* set `FD_CLOEXEC` on the returned file
/// descriptors. Children the supervisor spawns use
/// [`crate::inherited_fds::CloseInheritedFds`], so neither end survives into
/// them except as the stdio the daemon is given.
pub fn openpty() -> std::io::Result<PtyPair> {
    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;

    let ret = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(PtyPair {
        master: unsafe { OwnedFd::from_raw_fd(master_fd) },
        slave: unsafe { OwnedFd::from_raw_fd(slave_fd) },
    })
}
