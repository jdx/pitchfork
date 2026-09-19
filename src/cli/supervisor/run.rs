use crate::Result;
use crate::cli::supervisor::{
    KillOrStopOutcome, resolve_existing_supervisor, unidentified_supervisor_error,
};
use crate::ipc::socket_display;
use crate::supervisor::SUPERVISOR;

/// Runs the internal pitchfork daemon in the foreground
#[derive(Debug, usage_rs::Args)]
pub struct Run {
    /// kill existing daemon
    #[usage(short, long)]
    force: bool,
    /// run as boot start (auto-start boot_start daemons)
    #[usage(long)]
    boot: bool,
    /// Enable container/PID1 mode (reap zombies, forward signals)
    #[usage(long, env = "PITCHFORK_CONTAINER")]
    container: bool,
    /// Enable web UI on specified port (tries up to 10 ports if in use)
    #[usage(long, env = "PITCHFORK_WEB_PORT")]
    web_port: Option<u16>,
    /// Serve web UI under a path prefix (e.g. "ps" serves at /ps/)
    #[usage(long, env = "PITCHFORK_WEB_PATH")]
    web_path: Option<String>,
    /// Run as root on behalf of this user, as if started with sudo by them
    ///
    /// `sudo pitchfork boot enable` records this flag in the system service,
    /// because launchd and systemd do not pass SUDO_USER to it. The user's home
    /// locates configuration, state, and the IPC socket; state is owned by the
    /// user; and daemons run as the user unless `supervisor.user` or a daemon's
    /// `user` says otherwise. Accepts a user name or numeric UID. Requires root,
    /// and the supervisor refuses to start if the account does not exist.
    #[usage(long, value_name = "USER")]
    invoking_user: Option<String>,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        self.check_invoking_user()?;
        let (existing_pid, outcome) = resolve_existing_supervisor(self.force).await?;
        match outcome {
            KillOrStopOutcome::Unidentified if self.force => {
                return Err(unidentified_supervisor_error());
            }
            KillOrStopOutcome::Unidentified => {
                warn!(
                    "A pitchfork supervisor is already running (listening on {}).",
                    socket_display()
                );
                return Ok(());
            }
            KillOrStopOutcome::StillRunning => {
                let pid = existing_pid.expect("StillRunning implies a pid exists");
                warn!(
                    "Pitchfork supervisor is already running with pid {pid}. Use `--force` to replace it."
                );
                return Ok(());
            }
            KillOrStopOutcome::Killed => {
                let pid = existing_pid.expect("Killed implies a pid exists");
                // The old supervisor keeps serving IPC until it has stopped
                // its daemons; starting before it lets go would be refused.
                crate::supervisor::wait_for_ipc_socket_release().await?;
                info!("Killed existing supervisor with pid {pid}");
            }
            KillOrStopOutcome::AlreadyDead => {}
        }

        SUPERVISOR
            .start(
                self.boot,
                self.container,
                self.web_port,
                self.web_path.clone(),
            )
            .await
    }
}

impl Run {
    /// Refuse to start when the recorded invoking user cannot be used, rather
    /// than falling back to root's configuration and identity.
    #[cfg(unix)]
    fn check_invoking_user(&self) -> Result<()> {
        let user = crate::env::INVOKING_USER
            .clone()
            .map_err(|e| miette::miette!(e))?;
        // Paths are resolved from argv before the CLI is parsed; both must see
        // the same flag.
        if self.invoking_user.is_some() != user.is_some() {
            miette::bail!(
                "could not read {flag} from the command line; pass it as \
                `pitchfork supervisor run {flag} <USER>`",
                flag = crate::env::INVOKING_USER_FLAG
            );
        }
        if let Some(user) = user {
            info!(
                "running on behalf of {} (uid={}, home={})",
                user.name,
                user.uid,
                user.home.display()
            );
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn check_invoking_user(&self) -> Result<()> {
        if self.invoking_user.is_some() {
            miette::bail!(
                "{} is only supported on Unix",
                crate::env::INVOKING_USER_FLAG
            );
        }
        Ok(())
    }
}
