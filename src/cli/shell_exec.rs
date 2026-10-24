use crate::Result;

/// Execute a shell script after mise has activated the daemon environment.
///
/// This bridge preserves cmd.exe's raw command line on mise versions without
/// native cmd forwarding. It inherits the daemon's console and stdio, and
/// returns the shell's exit code.
#[derive(Debug, usage_rs::Args)]
pub struct ShellExec {
    /// Shell, shell options, and the script as the last argument
    #[usage(double_dash = "required")]
    command: Vec<String>,
}

impl ShellExec {
    pub async fn run(&self) -> Result<()> {
        #[cfg(windows)]
        {
            use crate::shell::ShellScript;
            use miette::IntoDiagnostic;
            use windows_sys::Win32::System::Console::{CTRL_C_EVENT, SetConsoleCtrlHandler};

            let Some((program, args)) = self.command.split_first() else {
                miette::bail!("shell-exec requires a shell and a script");
            };
            let Some((script, options)) = args.split_last() else {
                miette::bail!("shell-exec requires a script");
            };

            // Keep the bridge alive while the shell handles Ctrl+C. A handler
            // is local to this process, unlike the inheritable ignore flag.
            unsafe extern "system" fn handle_interrupt(event: u32) -> i32 {
                i32::from(event == CTRL_C_EVENT)
            }
            if unsafe { SetConsoleCtrlHandler(Some(handle_interrupt), 1) } == 0 {
                return Err(std::io::Error::last_os_error()).into_diagnostic();
            }

            // Inherit the existing console, including an invisible console.
            // Creating another one would prevent the daemon's Ctrl+C reaching
            // the shell and its children.
            let status = tokio::process::Command::new(program)
                .shell_script(program, options, script)
                .status()
                .await
                .into_diagnostic()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        #[cfg(not(windows))]
        {
            miette::bail!("shell-exec is only used on Windows");
        }
    }
}
