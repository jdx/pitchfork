use crate::Result;
use crate::daemon_id::DaemonId;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use std::sync::Arc;

/// Sends a stop signal to a daemon
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = "\
Sends a stop signal to a daemon

Uses a graceful shutdown strategy:
1. Send the configured stop signal (SIGTERM by default) to the process group
   and wait for stop_signal.timeout or supervisor.stop_timeout (default: 5s)
2. If still running, send SIGKILL to force termination

Most processes exit promptly after the first signal. The escalation
ensures stubborn processes are eventually terminated while giving well-behaved
processes time to clean up resources.

When using --all/--local/--global, daemons are stopped in reverse dependency order:
dependents are stopped before the daemons they depend on.

If the supervisor is not running, there is nothing to stop: the command warns
and exits 0, so cleanup scripts can call it unconditionally. It does not start
the supervisor. If a supervisor that crashed left daemon processes behind, it
fails instead, naming them: start the supervisor, which takes over or cleans
up what a crashed supervisor left, and stop them again.

Examples:

    pitchfork stop api           Stop a single daemon
    pitchfork stop api worker    Stop multiple daemons
    pitchfork stop --group backend Stop all daemons in the 'backend' group
    pitchfork stop --all         Stop all running daemons in dependency order
    pitchfork stop -l            Stop all local daemons in pitchfork.toml
    pitchfork stop -g            Stop all global daemons in config.toml
    pitchfork kill api           Same as 'stop' (alias)"
)]
pub struct Stop {
    /// The name of the daemon(s) to stop
    #[usage(conflicts = "local", conflicts = "global", conflicts = "all")]
    id: Vec<String>,
    /// Stop all daemons in the named group
    #[usage(
        long,
        value_name = "GROUP",
        conflicts = "local",
        conflicts = "global",
        conflicts = "all"
    )]
    group: Option<String>,
    /// Stop all running daemons (in reverse dependency order)
    #[usage(long, short, conflicts = "local", conflicts = "global")]
    all: bool,
    /// Stop all local daemons in pitchfork.toml
    #[usage(
        long,
        short = 'l',
        visible_alias = "all-local",
        conflicts = "all",
        conflicts = "global"
    )]
    local: bool,
    /// Stop all global daemons in ~/.config/pitchfork/config.toml and /etc/pitchfork/config.toml
    #[usage(
        long,
        short = 'g',
        visible_alias = "all-global",
        conflicts = "local",
        conflicts = "all"
    )]
    global: bool,
}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        let no_target = self.no_target();

        if no_target {
            super::interactive::require_interactive_terminal()?;
        }

        if !crate::ipc::supervisor_listening().await {
            return self.stop_without_supervisor();
        }

        let ipc = Arc::new(IpcClient::connect(false).await?);

        let ids: Vec<DaemonId> = if self.all {
            ipc.get_running_daemons().await?
        } else if self.global || self.local {
            ipc.get_running_configured_daemons(self.global).await?
        } else if no_target {
            let candidates = ipc.get_running_daemons().await?;
            super::interactive::select_daemons_interactively(&candidates, "stop")?
        } else {
            PitchforkToml::resolve_ids_and_group(&self.id, self.group.as_deref())?
        };

        if ids.is_empty() {
            warn!("No daemons to stop");
            return Ok(());
        }

        let result = ipc.stop_daemons(&ids).await?;

        if result.any_failed {
            std::process::exit(1);
        }
        Ok(())
    }

    /// With no supervisor, nothing pitchfork manages is running unless a
    /// crashed supervisor left processes behind. Those cannot be stopped
    /// gracefully from here (no hooks, no dependency order), so they are
    /// reported rather than silently left running or killed.
    fn stop_without_supervisor(&self) -> Result<()> {
        let sf = StateFile::get();
        let targets: Option<Vec<DaemonId>> = if self.all || self.no_target() {
            None
        } else if self.global {
            Some(IpcClient::get_global_configured_daemons()?)
        } else if self.local {
            Some(IpcClient::get_local_configured_daemons()?)
        } else {
            Some(PitchforkToml::resolve_ids_and_group(
                &self.id,
                self.group.as_deref(),
            )?)
        };
        let orphans = orphaned_daemons(sf, targets.as_deref());
        if orphans.is_empty() {
            warn!("Supervisor is not running, nothing to stop");
            return Ok(());
        }
        let list = orphans
            .iter()
            .map(|(id, pid)| format!("{id} (pid {pid})"))
            .collect::<Vec<_>>()
            .join(", ");
        Err(miette::miette!(
            help = "start the supervisor, which takes over or cleans up what a crashed \
                    supervisor left, then stop them again: pitchfork supervisor start",
            "supervisor is not running, but daemons it started are still running: {list}"
        ))
    }

    fn no_target(&self) -> bool {
        self.id.is_empty() && self.group.is_none() && !self.local && !self.global && !self.all
    }
}

/// Daemons (restricted to `targets` when given) whose recorded process is
/// still alive with no supervisor to manage it. A PID from a previous boot,
/// or one now belonging to another process, does not count.
fn orphaned_daemons(sf: &StateFile, targets: Option<&[DaemonId]>) -> Vec<(DaemonId, u32)> {
    let current_boot = PROCS.boot_time();
    sf.daemons
        .values()
        .filter(|d| d.id != DaemonId::pitchfork())
        .filter(|d| targets.is_none_or(|t| t.contains(&d.id)))
        .filter_map(|d| {
            let pid = d.pid?;
            let same_boot = d.boot_time.is_none_or(|b| b == current_boot);
            let alive = same_boot
                && PROCS.is_running(pid)
                && crate::supervisor::signalling_pid_is_authorized(
                    d.start_time,
                    PROCS.start_time(pid),
                );
            alive.then(|| (d.id.clone(), pid))
        })
        .collect()
}
