//! Ctrl+C for daemons on Windows, where there is no SIGINT to send.
//!
//! Windows interrupts a console program with a console control event: Ctrl+C
//! raises `CTRL_C_EVENT` in every process attached to the console, and programs
//! such as postgres or node handle it the way they handle SIGINT elsewhere.
//! That is how a daemon configured with `stop_signal = "SIGINT"` is asked to
//! stop on Windows.
//!
//! The event can only be raised on the console the caller is attached to, and
//! attaching is process-wide: the supervisor would have to give up its own
//! console state, and every command it spawned meanwhile would land on the
//! daemon's console. The hidden `pitchfork interrupt` command does it in a
//! process of its own instead.

/// Exit status of `pitchfork interrupt` when the daemon shares the
/// supervisor's console and was therefore left alone.
pub const EXIT_SHARED_CONSOLE: i32 = 3;

/// What [`interrupt_console_of`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Interrupted {
    /// Ctrl+C was raised on the daemon's console.
    Sent,
    /// The console is the supervisor's own, so nothing was sent.
    ///
    /// A daemon is only on its own console when the supervisor has none,
    /// which is the usual background case. A supervisor run in the foreground
    /// on a real console shares it with its daemons, and Ctrl+C there would
    /// reach the supervisor and every other daemon too.
    SharedWithSupervisor,
}

/// Raise Ctrl+C on the console `pid` is attached to, unless `supervisor_pid`
/// is attached to it as well.
///
/// Detaches this process from its own console to do so, so it is only for the
/// `pitchfork interrupt` process, never the supervisor.
pub fn interrupt_console_of(pid: u32, supervisor_pid: u32) -> std::io::Result<Interrupted> {
    use windows_sys::Win32::System::Console::{
        AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
    };

    // The event reaches this process as well, once it is on that console.
    // Ignore it before attaching, so an interrupt raised by anyone else
    // meanwhile cannot end it either.
    if unsafe { SetConsoleCtrlHandler(None, 1) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Fails when this process has no console, which is fine.
    unsafe { FreeConsole() };
    if unsafe { AttachConsole(pid) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let result = console_process_list().and_then(|attached| {
        if attached.contains(&supervisor_pid) {
            return Ok(Interrupted::SharedWithSupervisor);
        }
        if unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Interrupted::Sent)
    });
    unsafe { FreeConsole() };
    result
}

/// Every process attached to this process's console.
fn console_process_list() -> std::io::Result<Vec<u32>> {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;

    let mut pids = vec![0u32; 64];
    // The list can grow between calls; a few retries with headroom settle it.
    for _ in 0..4 {
        let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
        if count == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // A buffer that is too small is answered with the size it needs.
        if count as usize <= pids.len() {
            pids.truncate(count as usize);
            return Ok(pids);
        }
        pids.resize(count as usize * 2, 0);
    }
    Err(std::io::Error::other(
        "the console's process list kept outgrowing the buffer",
    ))
}
