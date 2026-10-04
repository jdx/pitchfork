use crate::Result;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::env;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use miette::WrapErr;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

        if !supervisor_running().await? && self.stop_without_supervisor()? {
            return Ok(());
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
    ///
    /// Returns false, having changed nothing, if a supervisor started in the
    /// meantime, so the caller stops through it instead.
    fn stop_without_supervisor(&self) -> Result<bool> {
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
        // The check and the update happen under one state file lock, so a
        // supervisor starting concurrently cannot record a daemon in between
        // that this then marks stopped. A state file that cannot be read must
        // not pass for one with no daemons in it: the orphan check could not
        // be done.
        let path = &*env::PITCHFORK_STATE_FILE;
        let outcome = StateFile::update_in_file(path, |sf| {
            if sf
                .daemons
                .get(&DaemonId::pitchfork())
                .is_some_and(crate::supervisor::supervisor_record_is_live)
            {
                return (false, Unsupervised::SupervisorStarted);
            }
            let orphans = orphaned_daemons(sf, targets.as_deref());
            if !orphans.is_empty() {
                return (false, Unsupervised::Orphans(orphans));
            }
            // A failed daemon with retries left, or one that died with a
            // crashed supervisor, would be started again by the next
            // supervisor; the user asked for it to stay stopped.
            let canceled = cancel_pending_retries(sf, targets.as_deref());
            (!canceled.is_empty(), Unsupervised::Canceled(canceled))
        })
        .wrap_err_with(|| format!("failed to read state file {}", path.display()))?;
        let orphans = match outcome {
            Unsupervised::SupervisorStarted => return Ok(false),
            Unsupervised::Orphans(orphans) => orphans,
            Unsupervised::Canceled(canceled) => {
                for id in canceled {
                    info!("Marked {id} stopped so a supervisor does not restart it");
                }
                warn!("Supervisor is not running, nothing to stop");
                return Ok(true);
            }
        };
        let list = orphans
            .iter()
            .map(|o| format!("{} (pid {})", o.id, o.pid))
            .collect::<Vec<_>>()
            .join(", ");
        let mut help = "start the supervisor, which takes over or cleans up what a crashed \
                        supervisor left, then stop them again: pitchfork supervisor start"
            .to_string();
        // A supervisor only takes over a daemon through its recorded leader;
        // a group that outlived its leader has to be signalled directly.
        let leaderless: Vec<String> = orphans
            .iter()
            .filter(|o| !o.leader_alive)
            .map(|o| format!("kill -TERM -{}", o.pid))
            .collect();
        if !leaderless.is_empty() {
            help = format!(
                "{help}\nprocesses whose leader already exited are not taken over by the \
                 supervisor; signal their process groups directly: {}",
                leaderless.join("; ")
            );
        }
        Err(miette::miette!(
            help = help,
            "supervisor is not running, but daemons it started are still running: {list}"
        ))
    }

    /// Whether no daemon was named or selected, so the command picks
    /// interactively.
    fn no_target(&self) -> bool {
        self.id.is_empty() && self.group.is_none() && !self.local && !self.global && !self.all
    }
}

/// How long a supervisor that has recorded itself may take to open its IPC
/// socket before it is treated as not running.
const SUPERVISOR_STARTUP_WAIT: Duration = Duration::from_secs(5);

/// Whether a supervisor is running, waiting out one that is starting up.
///
/// The socket is the ground truth for a running supervisor. A starting
/// supervisor records itself, and may already start daemons, before it opens
/// the socket, so a live record without a listening socket is waited on
/// rather than taken to mean nothing is running.
async fn supervisor_running() -> Result<bool> {
    if crate::ipc::supervisor_listening().await {
        return Ok(true);
    }
    let starting = read_state()?
        .daemons
        .get(&DaemonId::pitchfork())
        .is_some_and(crate::supervisor::supervisor_record_is_live);
    if !starting {
        return Ok(false);
    }
    let deadline = Instant::now() + SUPERVISOR_STARTUP_WAIT;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if crate::ipc::supervisor_listening().await {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Read the state file fresh, failing rather than falling back to an empty
/// state when it cannot be read.
fn read_state() -> Result<StateFile> {
    let path = &*env::PITCHFORK_STATE_FILE;
    StateFile::read(path).wrap_err_with(|| format!("failed to read state file {}", path.display()))
}

/// Mark daemons (restricted to `targets` when given) that a supervisor would
/// start again as stopped, returning their IDs: failed daemons with retries
/// left, and records still marked running, which a starting supervisor turns
/// into retries once it finds their process gone. Only called once no
/// targeted daemon is still running, so those records' processes are dead.
fn cancel_pending_retries(sf: &mut StateFile, targets: Option<&[DaemonId]>) -> Vec<DaemonId> {
    let ids: Vec<DaemonId> = sf
        .daemons
        .values()
        .filter(|d| d.id != DaemonId::pitchfork())
        .filter(|d| targets.is_none_or(|t| t.contains(&d.id)))
        .filter(|d| {
            (d.status.is_running()
                && d.pid
                    .is_none_or(|pid| recorded_process_is_gone(pid, d.start_time)))
                || (d.status.is_errored() && d.pid.is_none() && d.retry_count < d.retry.count())
        })
        .map(|d| d.id.clone())
        .collect();
    for id in &ids {
        if let Some(d) = sf.daemons.get_mut(id) {
            d.status = DaemonStatus::Stopped;
            d.pid = None;
        }
    }
    ids
}

/// Whether the process recorded at `pid` is known to have exited: the PID is
/// dead, or now belongs to a process with a different kernel start token.
/// Rechecked rather than trusting the orphan scan; a live PID whose identity
/// cannot be compared is left for the supervisor to sort out.
fn recorded_process_is_gone(pid: u32, recorded_start_time: Option<u64>) -> bool {
    if !PROCS.is_live(pid) {
        return true;
    }
    matches!(
        (recorded_start_time, PROCS.start_time(pid)),
        (Some(recorded), Some(current)) if recorded != current
    )
}

/// What `stop` found in the state file when no supervisor was listening.
enum Unsupervised {
    /// A supervisor has recorded itself since, so stopping goes through it.
    SupervisorStarted,
    /// Targeted daemons a crashed supervisor left running.
    Orphans(Vec<Orphan>),
    /// Nothing targeted is running; these records were marked stopped.
    Canceled(Vec<DaemonId>),
}

/// A daemon left running by a supervisor that is no longer running.
struct Orphan {
    id: DaemonId,
    /// The recorded PID, which is also the daemon's process group ID.
    pid: u32,
    /// Whether the group's leader is still alive, as opposed to only other
    /// members of its group.
    leader_alive: bool,
}

/// Daemons (restricted to `targets` when given) whose processes are still
/// alive with no supervisor to manage them. A PID now belonging to another
/// process, or one recorded in a previous boot, does not count.
fn orphaned_daemons(sf: &StateFile, targets: Option<&[DaemonId]>) -> Vec<Orphan> {
    let current_boot = PROCS.boot_time();
    // The reported boot time shifts with NTP steps and sleep/resume, so it
    // only rules a record out when it is clearly from another boot.
    let same_boot = |recorded: Option<u64>| {
        recorded
            .is_none_or(|b| b.abs_diff(current_boot) <= crate::supervisor::BOOT_TIME_TOLERANCE_SECS)
    };
    sf.daemons
        .values()
        .filter(|d| d.id != DaemonId::pitchfork())
        .filter(|d| targets.is_none_or(|t| t.contains(&d.id)))
        .filter_map(|d| {
            let pid = d.pid?;
            let leader_alive = PROCS.is_live(pid);
            let alive = if leader_alive {
                match (d.start_time, PROCS.start_time(pid)) {
                    // The kernel start token is the process's identity, but
                    // Linux counts it from boot, so it only holds within one.
                    (Some(recorded), Some(current)) => {
                        recorded == current && same_boot(d.boot_time)
                    }
                    _ => same_boot(d.boot_time),
                }
            } else {
                // Daemons lead their own process group (PGID == PID), and a
                // PID is not handed out while a group with that ID exists, so
                // members still in the group are the daemon's even after
                // the leader (e.g. a wrapping shell) has exited. Members that
                // have exited but were never reaped are not running. Without
                // a recorded boot time, nothing ties the group ID to this
                // boot, so the group is not attributed to the daemon.
                d.boot_time.is_some()
                    && same_boot(d.boot_time)
                    && PROCS.process_group_has_live_member(pid)
            };
            alive.then(|| Orphan {
                id: d.id.clone(),
                pid,
                leader_alive,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::Daemon;

    fn errored(name: &str, retry: u32, retry_count: u32) -> Daemon {
        Daemon {
            id: DaemonId::new("proj", name),
            status: DaemonStatus::Errored(1),
            retry: crate::config_types::Retry(retry),
            retry_count,
            ..Default::default()
        }
    }

    #[test]
    fn stopping_without_a_supervisor_cancels_pending_retries() {
        let mut sf = StateFile::new(std::path::PathBuf::from("state.toml"));
        for d in [
            errored("retrying", 3, 1),
            errored("exhausted", 3, 3),
            errored("other", 3, 0),
            Daemon {
                id: DaemonId::new("proj", "crashed"),
                status: DaemonStatus::Running,
                // Above any PID the kernel hands out, so never alive.
                pid: Some(2_000_000_000),
                ..Default::default()
            },
            Daemon {
                id: DaemonId::new("proj", "alive"),
                status: DaemonStatus::Running,
                pid: Some(std::process::id()),
                ..Default::default()
            },
            // A live PID whose start token differs: reused by another process.
            Daemon {
                id: DaemonId::new("proj", "recycled"),
                status: DaemonStatus::Running,
                pid: Some(std::process::id()),
                start_time: PROCS.start_time(std::process::id()).map(|t| t + 1),
                ..Default::default()
            },
        ] {
            sf.insert_daemon(&d.id.clone(), d);
        }
        let target = DaemonId::new("proj", "retrying");
        let exhausted = DaemonId::new("proj", "exhausted");
        let other = DaemonId::new("proj", "other");
        let crashed = DaemonId::new("proj", "crashed");
        let alive = DaemonId::new("proj", "alive");
        let recycled = DaemonId::new("proj", "recycled");

        let mut canceled = cancel_pending_retries(
            &mut sf,
            Some(&[
                target.clone(),
                exhausted.clone(),
                crashed.clone(),
                alive.clone(),
                recycled.clone(),
            ]),
        );
        canceled.sort_by_key(|id| id.to_string());

        assert_eq!(
            canceled,
            vec![crashed.clone(), recycled.clone(), target.clone()]
        );
        assert!(matches!(sf.daemons[&crashed].status, DaemonStatus::Stopped));
        assert_eq!(sf.daemons[&crashed].pid, None);
        assert!(matches!(sf.daemons[&alive].status, DaemonStatus::Running));
        assert!(matches!(sf.daemons[&target].status, DaemonStatus::Stopped));
        assert!(sf.daemons[&exhausted].status.is_errored());
        assert!(sf.daemons[&other].status.is_errored());
    }
}
