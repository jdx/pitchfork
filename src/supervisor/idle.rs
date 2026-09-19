//! Idle shutdown of daemons the proxy started.
//!
//! A daemon is eligible only while its record carries a
//! `proxy_idle_timeout_ms`, which only a proxy auto-start sets. Anything
//! started another way — and a proxy-started daemon that has since been
//! started explicitly, which [`Supervisor::claim_daemons`] records — is never
//! stopped here.
//!
//! The interval watcher calls [`Supervisor::check_idle_daemons`]. It stops an
//! eligible daemon once all of these hold:
//!
//! - the proxy has carried nothing for it (see [`crate::proxy::activity`]) for
//!   its grace period;
//! - nothing running or starting depends on it, other than daemons being
//!   stopped in the same pass;
//! - no tracked shell or project session is inside its directory.
//!
//! Daemons are stopped dependents first, so a dependency goes only after the
//! last daemon that needs it, and never while anything else still does.

use super::autostop::is_within;
use super::{SUPERVISOR, Supervisor};
use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::proxy::activity::ACTIVITY;
use log::LevelFilter::Info;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Set while a sweep's stops are under way, so a slow stop does not let the
/// next tick start a second, overlapping sweep.
static SWEEPING: AtomicBool = AtomicBool::new(false);

/// Holds [`SWEEPING`] for one sweep and clears it when dropped, so a sweep
/// that ends early — or panics — does not turn idle shutdown off for good.
struct Sweep;

impl Sweep {
    fn begin() -> Option<Self> {
        (!SWEEPING.swap(true, Ordering::SeqCst)).then_some(Sweep)
    }
}

impl Drop for Sweep {
    fn drop(&mut self) {
        SWEEPING.store(false, Ordering::SeqCst);
    }
}

/// A daemon's idle grace period, if it may be stopped for inactivity.
fn grace(daemon: &Daemon) -> Option<Duration> {
    daemon.proxy_idle_timeout_ms.map(Duration::from_millis)
}

/// Whether `daemon` still needs what it depends on: it is up, on its way up,
/// or on its way back. A restart (file watch, `--force`) stops first, and an
/// errored daemon with retries left is about to start again, so both keep
/// their dependencies as a running daemon does.
fn is_live(daemon: &Daemon) -> bool {
    let status = &daemon.status;
    status.is_running()
        || status.is_waiting()
        || status.is_stopping()
        || (status.is_errored() && daemon.retry_count < daemon.retry.count())
}

/// Whether a tracked shell or project session is inside `daemon`'s directory.
fn shell_inside(daemon: &Daemon, active_dirs: &[PathBuf]) -> bool {
    daemon
        .dir
        .as_deref()
        .is_some_and(|dir| active_dirs.iter().any(|d| is_within(dir, d)))
}

/// Live daemons that depend on `id`, other than those in `excluding`.
fn live_dependents<'a>(
    id: &'a DaemonId,
    daemons: &'a BTreeMap<DaemonId, Daemon>,
    excluding: &'a HashSet<DaemonId>,
) -> impl Iterator<Item = &'a DaemonId> + 'a {
    daemons
        .values()
        .filter(move |d| is_live(d) && d.depends.contains(id) && !excluding.contains(&d.id))
        .map(|d| &d.id)
}

/// Which daemons to stop for inactivity, and in what order.
///
/// Returns levels: every daemon in a level is stopped before any in the next,
/// and nothing in a later level depends on anything in an earlier one — so
/// dependents come first. A daemon is included only when it is eligible, idle
/// by `is_idle`, has no shell inside its directory, and every live daemon
/// depending on it is itself included.
pub(crate) fn plan_idle_stops(
    daemons: &BTreeMap<DaemonId, Daemon>,
    active_dirs: &[PathBuf],
    is_idle: impl Fn(&DaemonId, Duration) -> bool,
) -> Vec<Vec<DaemonId>> {
    let mut chosen: HashSet<DaemonId> = daemons
        .values()
        .filter(|d| d.status.is_running() && !shell_inside(d, active_dirs))
        .filter(|d| grace(d).is_some_and(|g| is_idle(&d.id, g)))
        .map(|d| d.id.clone())
        .collect();

    // Drop anything something outside the set still needs. Dropping one can
    // leave its own dependencies needed in turn, so repeat until nothing
    // changes.
    loop {
        let needed: Vec<DaemonId> = chosen
            .iter()
            .filter(|id| live_dependents(id, daemons, &chosen).next().is_some())
            .cloned()
            .collect();
        if needed.is_empty() {
            break;
        }
        for id in needed {
            chosen.remove(&id);
        }
    }

    // Peel off, level by level, the daemons nothing left in the set depends on.
    let mut levels = Vec::new();
    while !chosen.is_empty() {
        let mut level: Vec<DaemonId> = chosen
            .iter()
            .filter(|id| {
                !chosen
                    .iter()
                    .any(|other| daemons.get(other).is_some_and(|d| d.depends.contains(id)))
            })
            .cloned()
            .collect();
        if level.is_empty() {
            // A dependency cycle: nothing can go first, so stop them together.
            level = chosen.iter().cloned().collect();
        }
        level.sort();
        for id in &level {
            chosen.remove(id);
        }
        levels.push(level);
    }
    levels
}

impl Supervisor {
    /// Record that `ids` were started explicitly, so none of them is stopped
    /// for inactivity from now on.
    ///
    /// Sent by every start that is not the proxy's, for the daemons it names
    /// and everything they depend on, before anything is started: the result
    /// is the same as if the explicit start had come first. Waits out an idle
    /// stop already under way for any of them, so the caller then sees the
    /// daemon stopped and starts it, rather than skipping it as running while
    /// it goes away.
    pub(crate) async fn claim_daemons(&self, ids: &[DaemonId]) {
        let claimed: Vec<DaemonId> = {
            let mut state_file = self.state_file.lock().await;
            ids.iter()
                .filter(|id| state_file.clear_proxy_idle_timeout(id))
                .cloned()
                .collect()
        };
        for id in &claimed {
            info!("{id} was started explicitly; it will no longer be stopped when idle");
        }
        // An idle stop revalidates ownership under the daemon's stop lock, so
        // one that has not taken the lock yet will now call itself off; one
        // that holds it is waited for here.
        for id in ids {
            if ACTIVITY.is_idle_stopping(id) {
                drop(self.stop_lock(id).await.lock().await);
            }
        }
    }

    /// Stop proxy-started daemons that have been idle for their grace period.
    ///
    /// Cheap when nothing is eligible, which is the default. The stops run in
    /// a detached task so a slow one does not hold up the interval watcher.
    pub(crate) async fn check_idle_daemons(&self) {
        let daemons = {
            let state_file = self.state_file.lock().await;
            if !state_file
                .daemons
                .values()
                .any(|d| d.proxy_idle_timeout_ms.is_some() && d.status.is_running())
            {
                return;
            }
            state_file.daemons.clone()
        };
        let Some(sweep) = Sweep::begin() else {
            return;
        };
        let active_dirs = self.get_active_directories().await;
        let plan = plan_idle_stops(&daemons, &active_dirs, |id, grace| {
            let activity = ACTIVITY.snapshot(id);
            activity.in_flight == 0 && !activity.idle_stopping && activity.idle_for >= grace
        });
        if plan.is_empty() {
            return;
        }
        debug!("idle shutdown plan: {plan:?}");
        let graces: HashMap<DaemonId, Duration> = daemons
            .values()
            .filter_map(|d| grace(d).map(|g| (d.id.clone(), g)))
            .collect();
        tokio::spawn(async move {
            let _sweep = sweep;
            for level in plan {
                for id in level {
                    if let Some(&grace) = graces.get(&id) {
                        SUPERVISOR.idle_stop(&id, grace).await;
                    }
                }
            }
        });
    }

    /// Stop one daemon for inactivity, if it still qualifies.
    ///
    /// The claim on its activity keeps new proxy work from starting while the
    /// stop runs: a request arriving meanwhile waits and starts the daemon
    /// again once it has stopped. Everything the plan checked is checked again
    /// under the daemon's stop lock, since a request, an explicit start, a
    /// shell or a new dependent may have arrived since.
    async fn idle_stop(&self, id: &DaemonId, grace: Duration) {
        if !ACTIVITY.claim_idle_stop(id, grace) {
            debug!("idle stop of {id} called off: it was active again");
            return;
        }
        let lock = self.stop_lock(id).await;
        let stopped = {
            let _guard = lock.lock().await;
            match self.idle_stop_blocker(id).await {
                Some(reason) => {
                    debug!("idle stop of {id} called off: {reason}");
                    false
                }
                None => {
                    info!("stopping {id}: no proxy activity for {grace:?}");
                    match self.stop_locked(id).await {
                        Ok(_) => true,
                        Err(e) => {
                            error!("failed to stop idle daemon {id}: {e}");
                            false
                        }
                    }
                }
            }
        };
        ACTIVITY.release_idle_stop(id);
        if stopped {
            self.add_notification(Info, format!("stopped idle {id}"))
                .await;
        }
    }

    /// Why `id` must not be stopped for inactivity right now, if anything.
    async fn idle_stop_blocker(&self, id: &DaemonId) -> Option<&'static str> {
        let active_dirs = self.get_active_directories().await;
        let state_file = self.state_file.lock().await;
        let Some(daemon) = state_file.daemons.get(id) else {
            return Some("it is no longer known");
        };
        if !daemon.status.is_running() {
            return Some("it is no longer running");
        }
        if daemon.proxy_idle_timeout_ms.is_none() {
            return Some("it was started explicitly");
        }
        if shell_inside(daemon, &active_dirs) {
            return Some("a shell is inside its directory");
        }
        if live_dependents(id, &state_file.daemons, &HashSet::new())
            .next()
            .is_some()
        {
            return Some("a running daemon depends on it");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_status::DaemonStatus;

    fn id(name: &str) -> DaemonId {
        DaemonId::new("proj", name)
    }

    struct Fixture(BTreeMap<DaemonId, Daemon>);

    impl Fixture {
        fn new() -> Self {
            Self(BTreeMap::new())
        }

        fn add(mut self, name: &str, idle_ms: Option<u64>, depends: &[&str]) -> Self {
            self.0.insert(
                id(name),
                Daemon {
                    id: id(name),
                    status: DaemonStatus::Running,
                    dir: Some(PathBuf::from("/work/proj")),
                    depends: depends.iter().map(|d| id(d)).collect(),
                    proxy_idle_timeout_ms: idle_ms,
                    ..Default::default()
                },
            );
            self
        }

        fn status(mut self, name: &str, status: DaemonStatus) -> Self {
            self.0.get_mut(&id(name)).unwrap().status = status;
            self
        }

        fn plan(&self, active_dirs: &[&str], idle: &[&str]) -> Vec<Vec<String>> {
            let dirs: Vec<PathBuf> = active_dirs.iter().map(PathBuf::from).collect();
            let idle: HashSet<DaemonId> = idle.iter().map(|n| id(n)).collect();
            plan_idle_stops(&self.0, &dirs, |d, _| idle.contains(d))
                .into_iter()
                .map(|l| l.into_iter().map(|d| d.name().to_string()).collect())
                .collect()
        }
    }

    const G: Option<u64> = Some(60_000);

    #[test]
    fn only_proxy_started_daemons_are_eligible() {
        let f = Fixture::new().add("web", G, &[]).add("manual", None, &[]);
        assert_eq!(f.plan(&[], &["web", "manual"]), vec![vec!["web"]]);
    }

    #[test]
    fn active_daemons_are_kept() {
        let f = Fixture::new().add("web", G, &[]);
        assert!(f.plan(&[], &[]).is_empty());
    }

    #[test]
    fn dependencies_stop_after_their_dependents() {
        let f = Fixture::new()
            .add("db", G, &[])
            .add("cache", G, &[])
            .add("api", G, &["db", "cache"])
            .add("web", G, &["api"]);
        assert_eq!(
            f.plan(&[], &["db", "cache", "api", "web"]),
            vec![vec!["web"], vec!["api"], vec!["cache", "db"]]
        );
    }

    #[test]
    fn a_dependency_stays_while_a_busy_dependent_needs_it() {
        let f = Fixture::new().add("db", G, &[]).add("api", G, &["db"]);
        // db has no traffic of its own, but api is still busy.
        assert!(f.plan(&[], &["db"]).is_empty());
    }

    #[test]
    fn a_shared_dependency_stays_for_an_explicitly_started_consumer() {
        let f =
            Fixture::new()
                .add("db", G, &[])
                .add("api", G, &["db"])
                .add("worker", None, &["db"]);
        assert_eq!(f.plan(&[], &["db", "api"]), vec![vec!["api"]]);
    }

    #[test]
    fn a_starting_dependent_keeps_its_dependency() {
        let f = Fixture::new()
            .add("db", G, &[])
            .add("worker", None, &["db"])
            .status("worker", DaemonStatus::Waiting);
        assert!(f.plan(&[], &["db"]).is_empty());
    }

    #[test]
    fn a_dependent_on_its_way_back_keeps_its_dependency() {
        // Stopping for a restart.
        let f = Fixture::new()
            .add("db", G, &[])
            .add("api", None, &["db"])
            .status("api", DaemonStatus::Stopping);
        assert!(f.plan(&[], &["db"]).is_empty());

        // Crashed, with a retry still to come.
        let mut f = Fixture::new()
            .add("db", G, &[])
            .add("api", None, &["db"])
            .status("api", DaemonStatus::Errored(1));
        f.0.get_mut(&id("api")).unwrap().retry = crate::config_types::Retry(3);
        assert!(f.plan(&[], &["db"]).is_empty());

        // Out of retries: it is not coming back.
        f.0.get_mut(&id("api")).unwrap().retry_count = 3;
        assert_eq!(f.plan(&[], &["db"]), vec![vec!["db"]]);
    }

    #[test]
    fn a_stopped_dependent_does_not_keep_its_dependency() {
        let f = Fixture::new()
            .add("db", G, &[])
            .add("worker", None, &["db"])
            .status("worker", DaemonStatus::Stopped);
        assert_eq!(f.plan(&[], &["db"]), vec![vec!["db"]]);
    }

    #[test]
    fn a_proxied_daemon_depending_on_a_busy_one_keeps_it() {
        // `admin` depends on `api`; `api` is idle, but `admin` is serving
        // traffic, so `api` must stay even though it saw none itself.
        let f = Fixture::new().add("api", G, &[]).add("admin", G, &["api"]);
        assert!(f.plan(&[], &["api"]).is_empty());
    }

    #[test]
    fn a_shell_inside_the_directory_keeps_the_stack() {
        let f = Fixture::new().add("db", G, &[]).add("api", G, &["db"]);
        assert!(f.plan(&["/work/proj/src"], &["db", "api"]).is_empty());
        // A shell elsewhere does not.
        assert_eq!(
            f.plan(&["/work/other"], &["db", "api"]),
            vec![vec!["api"], vec!["db"]]
        );
    }

    #[test]
    fn a_dependency_cycle_stops_together() {
        let f = Fixture::new().add("a", G, &["b"]).add("b", G, &["a"]);
        assert_eq!(f.plan(&[], &["a", "b"]), vec![vec!["a", "b"]]);
    }
}
