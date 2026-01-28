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
        let default = Shell::default();
        #[cfg(unix)]
        assert_eq!(default, Shell::Sh);
        #[cfg(windows)]
        assert_eq!(default, Shell::Cmd);
    }
}
