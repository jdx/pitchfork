//! Shell abstraction for cross-platform command execution
//!
//! This module provides a platform-agnostic way to execute shell commands,
//! supporting different shells on Unix and Windows platforms.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Supported shell types for command execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[allow(clippy::enum_variant_names)] // PowerShell is the correct name for this shell
pub enum Shell {
    /// POSIX-compatible shell (default on Unix)
    #[default]
    Sh,
    /// Bash shell
    Bash,
    /// Zsh shell
    Zsh,
    /// Fish shell
    Fish,
    /// Windows Command Prompt
    Cmd,
    /// PowerShell (cross-platform)
    #[serde(alias = "pwsh")]
    PowerShell,
}

impl Shell {
    /// Returns the default shell for the current platform
    #[cfg(unix)]
    pub fn default_for_platform() -> Self {
        Shell::Sh
    }

    /// Returns the default shell for the current platform
    #[cfg(windows)]
    pub fn default_for_platform() -> Self {
        Shell::Cmd
    }

    /// Returns the shell program name/path
    pub fn program(&self) -> &'static str {
        match self {
            Shell::Sh => "sh",
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Cmd => "cmd",
            Shell::PowerShell => {
                // pwsh is the cross-platform PowerShell, powershell is Windows-only
                #[cfg(windows)]
                {
                    "powershell"
                }
                #[cfg(not(windows))]
                {
                    "pwsh"
                }
            }
        }
    }

    /// Returns the arguments needed to execute a command string
    pub fn exec_args(&self, command: &str) -> Vec<String> {
        match self {
            Shell::Sh | Shell::Bash | Shell::Zsh => {
                vec!["-c".to_string(), command.to_string()]
            }
            Shell::Fish => {
                vec!["-c".to_string(), command.to_string()]
            }
            Shell::Cmd => {
                vec!["/C".to_string(), command.to_string()]
            }
            Shell::PowerShell => {
                vec!["-Command".to_string(), command.to_string()]
            }
        }
    }

    /// Creates a tokio Command configured to run the given command string
    pub fn command(&self, cmd: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(self.program());
        command.args(self.exec_args(cmd));
        command
    }

    /// Creates a std Command configured to run the given command string
    #[allow(dead_code)] // Available for future use (e.g., spawn commands)
    pub fn std_command(&self, cmd: &str) -> std::process::Command {
        let mut command = std::process::Command::new(self.program());
        command.args(self.exec_args(cmd));
        command
    }
}

/// Prevents a spawned command from creating a console window on Windows.
///
/// The supervisor is created with `DETACHED_PROCESS | CREATE_NO_WINDOW`, so it
/// has no console of its own. On Windows the loader gives every
/// console-subsystem child of a console-less parent a brand new *visible*
/// console. Redirecting the child's stdio to pipes or NUL does not suppress
/// that, because the allocation is decided from the PE subsystem and the
/// creation flags rather than from the handles, so anything spawned from
/// inside the supervisor has to opt out explicitly.
///
/// Opting out does not leave the child without a console: `CREATE_NO_WINDOW`
/// gives it one of its own that simply has no window, so console APIs keep
/// working. Measured on Windows 11 — a child spawned with the flag reports
/// `GetConsoleCP() = 932` and `GetConsoleProcessList() = 1`, both of which fail
/// for a process with no console. What changes is only that the console is not
/// drawn, and that `GetConsoleWindow` returns null for it.
///
/// Implemented for both `std::process::Command` and `tokio::process::Command`,
/// and returns `&mut Self` so it drops into the existing fluent chains. The
/// non-Windows impls are no-ops, which keeps the call sites free of `cfg`.
///
/// The flag is only applied when this process has no console, because that is
/// the only case where a child would get one of its own. See
/// `child_would_get_its_own_console`.
///
/// Note: `creation_flags` *replaces* a command's creation flags rather than
/// OR-ing into them. Call this once per command, and after any other
/// `creation_flags` call, or those flags are silently dropped.
pub(crate) trait HideConsoleWindow {
    fn hide_console_window(&mut self) -> &mut Self;
}

/// Whether a console-subsystem child of this process would be given a console
/// of its own rather than inheriting one.
///
/// A child inherits the parent's console whenever the parent has one, and no
/// new window appears, so `CREATE_NO_WINDOW` is unnecessary there. It would
/// also be a behaviour change: the child would be put on a separate console
/// instead of the shared one, so a console control event sent to the parent's
/// console would no longer reach it. Detached processes such as the background
/// supervisor have no console, and only there does a child get a new — and
/// visible — one.
///
/// `GetConsoleWindow` reports the absence of a console *window*, which is not
/// quite the same as the absence of a console: it also returns null for a
/// console that has no window, such as a ConPTY session or a process started
/// with `CREATE_NO_WINDOW` itself. Those cases are counted as "no console"
/// here, and that costs nothing — the child is then given a console of its own
/// instead of sharing a console nobody can see, which is what every one of
/// these spawns did unconditionally before this check existed. What the check
/// is for is the case it does detect precisely: a supervisor running in the
/// foreground on a real console, whose children should keep sharing it.
#[cfg(windows)]
fn child_would_get_its_own_console() -> bool {
    let console = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
    console.is_null()
}

#[cfg(windows)]
impl HideConsoleWindow for std::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        if child_would_get_its_own_console() {
            self.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        } else {
            self
        }
    }
}

#[cfg(windows)]
impl HideConsoleWindow for tokio::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        // tokio exposes `creation_flags` as an inherent method on Windows;
        // `CommandExt` is not implemented for this type.
        if child_would_get_its_own_console() {
            self.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        } else {
            self
        }
    }
}

#[cfg(not(windows))]
impl HideConsoleWindow for std::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        self
    }
}

#[cfg(not(windows))]
impl HideConsoleWindow for tokio::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        self
    }
}

impl std::fmt::Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Shell::Sh => write!(f, "sh"),
            Shell::Bash => write!(f, "bash"),
            Shell::Zsh => write!(f, "zsh"),
            Shell::Fish => write!(f, "fish"),
            Shell::Cmd => write!(f, "cmd"),
            Shell::PowerShell => write!(f, "powershell"),
        }
    }
}

impl std::str::FromStr for Shell {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sh" => Ok(Shell::Sh),
            "bash" => Ok(Shell::Bash),
            "zsh" => Ok(Shell::Zsh),
            "fish" => Ok(Shell::Fish),
            "cmd" => Ok(Shell::Cmd),
            "powershell" | "pwsh" => Ok(Shell::PowerShell),
            _ => Err(format!("unknown shell: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_program() {
        assert_eq!(Shell::Sh.program(), "sh");
        assert_eq!(Shell::Bash.program(), "bash");
        assert_eq!(Shell::Zsh.program(), "zsh");
        assert_eq!(Shell::Fish.program(), "fish");
        assert_eq!(Shell::Cmd.program(), "cmd");
    }

    #[test]
    fn test_shell_exec_args() {
        assert_eq!(Shell::Sh.exec_args("echo hello"), vec!["-c", "echo hello"]);
        assert_eq!(
            Shell::Bash.exec_args("echo hello"),
            vec!["-c", "echo hello"]
        );
        assert_eq!(Shell::Cmd.exec_args("echo hello"), vec!["/C", "echo hello"]);
        assert_eq!(
            Shell::PowerShell.exec_args("echo hello"),
            vec!["-Command", "echo hello"]
        );
    }

    #[test]
    fn test_shell_from_str() {
        assert_eq!("sh".parse::<Shell>().unwrap(), Shell::Sh);
        assert_eq!("bash".parse::<Shell>().unwrap(), Shell::Bash);
        assert_eq!("BASH".parse::<Shell>().unwrap(), Shell::Bash);
        assert_eq!("powershell".parse::<Shell>().unwrap(), Shell::PowerShell);
        assert_eq!("pwsh".parse::<Shell>().unwrap(), Shell::PowerShell);
        assert!("unknown".parse::<Shell>().is_err());
    }

    #[test]
    fn test_shell_display() {
        assert_eq!(Shell::Sh.to_string(), "sh");
        assert_eq!(Shell::Bash.to_string(), "bash");
        assert_eq!(Shell::Cmd.to_string(), "cmd");
    }

    #[test]
    fn test_default_shell() {
        // Default should be Sh (or Cmd on Windows)
        let default = Shell::default_for_platform();
        #[cfg(unix)]
        assert_eq!(default, Shell::Sh);
        #[cfg(windows)]
        assert_eq!(default, Shell::Cmd);
    }

    /// Checks that `hide_console_window` is available for both command types,
    /// chains inside a builder expression, and leaves spawning intact.
    ///
    /// This does not assert that no console window appears: the reliable
    /// oracles for that are version-dependent Windows behaviour, so the
    /// absence of a window is verified manually instead.
    #[test]
    fn test_hide_console_window() {
        let program = if cfg!(windows) { "cmd" } else { "echo" };
        let args: Vec<&str> = if cfg!(windows) {
            vec!["/C", "echo hi"]
        } else {
            vec!["hi"]
        };

        let output = std::process::Command::new(program)
            .args(&args)
            .hide_console_window()
            .output()
            .expect("spawning the child should succeed");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hi");

        // Building a tokio command needs no runtime, so this pins the second
        // impl without making the test async.
        let mut async_command = tokio::process::Command::new(program);
        async_command.args(&args).hide_console_window();
    }
}
