//! Out-of-process capture of a daemon's output.
//!
//! The supervisor used to hold the read end of every daemon's output pipe,
//! which put it in the data path: killing it left the pipe with no reader, so
//! the daemon's next write took `SIGPIPE` and the daemon usually died with it.
//! Anything it had written but the supervisor had not yet read was lost too.
//!
//! Instead a sibling process — `pitchfork log-sink`, this binary re-executed —
//! holds the read end and writes to the log store, so a supervisor crash is
//! invisible to logging. This is the arrangement runit uses, where `runsv`
//! starts a service alongside its own log service and stays out of the stream.
//!
//! The supervisor keeps a spare *read* end open. That serves two purposes: a
//! sink that dies cannot leave the pipe readerless and kill the daemon, and the
//! replacement sink can be handed the very same pipe. It deliberately keeps no
//! write end, which would stop the sink from ever seeing end of file.
//!
//! Both descriptors belong to one pipe carrying stdout and stderr together,
//! matching what the in-process path did (it merged both into a single channel)
//! and what runit does. Writes up to `PIPE_BUF` are atomic, so a daemon writing
//! a line per `write` cannot interleave the two streams mid-line.
//!
//! Known gap: a daemon adopted after a supervisor crash keeps the sink it
//! already had, and its logging continues, but the new supervisor holds no
//! retained read end and runs no replacement loop for it. A sink that dies
//! after adoption is therefore not replaced, and the daemon will take SIGPIPE
//! on its next write. Recovering a read end from `/proc/<pid>/fd/1` would close
//! this on Linux.

use crate::Result;
use crate::daemon::RunOptions;
use crate::daemon_id::DaemonId;
use crate::log_store::LogStore;
use crate::supervisor::SUPERVISOR;
use miette::IntoDiagnostic;
use std::io::PipeReader;

/// Whether `opts` describes a daemon whose output can be captured by a sink.
///
/// Only PTY daemons are excluded, because their output arrives on a terminal
/// master rather than a pipe. They keep the in-process path and remain
/// vulnerable to a supervisor crash.
///
/// Everything that used to require the supervisor to read the stream —
/// `ready_output` and the `on_output` hook — is evaluated by the sink, which
/// reports the lines that matter back over IPC.
pub(crate) fn is_supported(opts: &RunOptions) -> bool {
    !opts.pty.unwrap_or(false)
}

/// What a sink should watch a daemon's output for.
///
/// Empty for most daemons: nothing needs reporting, and the sink just stores
/// what it reads.
#[derive(Default)]
pub(crate) struct WatchFor {
    /// `ready_output`'s pattern.
    ready_pattern: Option<String>,
    /// The `on_output` hook, already validated.
    hook: Option<crate::config_types::OnOutputHook>,
}

impl WatchFor {
    /// Read from a daemon's options, dropping a hook the supervisor would
    /// refuse to fire anyway — the in-process path discards an invalid hook in
    /// the same way, rather than firing it on every line.
    pub(crate) fn from_opts(id: &DaemonId, opts: &RunOptions) -> Self {
        Self {
            ready_pattern: opts.ready_output.as_ref().map(|o| o.pattern.clone()),
            hook: opts
                .on_output_hook
                .as_ref()
                .filter(|hook| {
                    hook.validate(id.name())
                        .inspect_err(|e| error!("{e}"))
                        .is_ok()
                })
                .cloned(),
        }
    }

    /// Whether the sink has anything to report, and so whether this attempt
    /// needs to be reachable from one.
    pub(crate) fn is_empty(&self) -> bool {
        self.ready_pattern.is_none() && self.hook.is_none()
    }

    /// The arguments telling a sink what to watch for.
    fn args(&self, relay_token: u64) -> Vec<String> {
        let mut args = Vec::new();
        if self.is_empty() {
            return args;
        }
        args.push("--relay-token".into());
        args.push(relay_token.to_string());
        if let Some(ref pattern) = self.ready_pattern {
            args.push("--ready-pattern".into());
            args.push(pattern.clone());
        }
        if let Some(ref hook) = self.hook {
            args.push("--report-output".into());
            if let Some(ref filter) = hook.filter {
                args.push("--output-filter".into());
                args.push(filter.clone());
            }
            if let Some(ref regex) = hook.regex {
                args.push("--output-regex".into());
                args.push(regex.clone());
            }
            args.push("--output-debounce-ms".into());
            args.push(hook.debounce_duration().as_millis().to_string());
        }
        args
    }
}

/// Source of relay tokens. Never reused within a supervisor's lifetime, and a
/// restarted supervisor has no relays to be confused about.
static NEXT_RELAY_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A daemon's registration entry: which start attempt it belongs to, and where
/// that attempt's output should go.
pub(crate) struct Relay {
    token: u64,
    tx: tokio::sync::mpsc::Sender<super::OutputLine>,
}

/// Registers where a daemon's sink-reported output should be delivered, and
/// stops delivery when dropped.
///
/// Tied to the monitoring task's lifetime: once that task is gone there is
/// nothing waiting on readiness, and a sink that outlives it — one still
/// draining a daemon's last output — must not be able to deliver into a channel
/// belonging to a later run of the same daemon.
pub(crate) struct OutputRelay {
    id: DaemonId,
    token: u64,
}

impl OutputRelay {
    /// Route lines reported for `id` under the returned token into `tx`, until
    /// this guard is dropped.
    ///
    /// Registered before the sink is started, so a match found in the daemon's
    /// very first line still has somewhere to go.
    pub(crate) fn register(
        id: &DaemonId,
        tx: tokio::sync::mpsc::Sender<super::OutputLine>,
    ) -> Self {
        let token = NEXT_RELAY_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SUPERVISOR
            .sink_output
            .lock()
            .expect("sink_output lock poisoned")
            .insert(id.clone(), Relay { token, tx });
        Self {
            id: id.clone(),
            token,
        }
    }

    /// The token a sink must quote for its reports to be delivered.
    pub(crate) fn token(&self) -> u64 {
        self.token
    }
}

impl Drop for OutputRelay {
    fn drop(&mut self) {
        let mut relays = SUPERVISOR
            .sink_output
            .lock()
            .expect("sink_output lock poisoned");
        // Only withdraw this registration, never a successor's. A retry
        // registers the next attempt's channel as soon as the previous attempt
        // reports failure, which can be before that attempt's monitoring task
        // has finished unwinding — an unconditional remove would leave the new
        // attempt with a sink reporting into nowhere.
        if relays
            .get(&self.id)
            .is_some_and(|current| current.token == self.token)
        {
            relays.remove(&self.id);
        }
    }
}

/// Hand a line reported by a sink to the daemon's monitoring task.
///
/// `token` identifies the start attempt the reporting sink belongs to. A sink
/// outlives its daemon — it exits only once every descendant has closed the
/// pipe — so the previous attempt's sink can still be draining, and reporting,
/// while a retry is starting. Without the token that line would be delivered to
/// the new attempt and could mark a process ready that has printed nothing.
///
/// Silently does nothing when the token has expired or nothing is listening:
/// both are ordinary, not errors.
pub(crate) async fn deliver_reported_line(
    id: &DaemonId,
    token: u64,
    fires_hook: bool,
    text: String,
) {
    let tx = SUPERVISOR
        .sink_output
        .lock()
        .expect("sink_output lock poisoned")
        .get(id)
        .filter(|relay| relay.token == token)
        .map(|relay| relay.tx.clone());
    let Some(tx) = tx else {
        debug!("dropping output reported for {id} by a sink from a finished start attempt");
        return;
    };
    if tx
        .send(super::OutputLine {
            text,
            source: super::OutputSource::Sink { fires_hook },
        })
        .await
        .is_err()
    {
        debug!("monitoring task for {id} stopped before its sink's output arrived");
    }
}

/// How long to wait before trying again when a sink process cannot be spawned.
///
/// Nothing drains the pipe while no sink is running, so a chatty daemon will
/// block on its next write once the pipe fills. Blocking is the right outcome —
/// it is backpressure rather than discarded output, and it is what runit does
/// when a log service cannot keep up — but keep the gap short so a transient
/// spawn failure is barely noticeable.
const SPAWN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// Longest gap between attempts once they keep failing. A sink that cannot be
/// started at all — a missing binary, no memory for another process — should not
/// be retried five times a second for the life of the daemon.
const SPAWN_RETRY_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// Wait until a daemon's output from this run is queryable, or `timeout`
/// elapses.
///
/// A failed start is reported by querying the log store, and the in-process
/// capture path made that safe by flushing synchronously before signalling.
/// With a sink there is a short gap instead: the daemon can exit before its
/// output has been written. Waiting for the *sink process* to exit would be far
/// more patient than necessary — it also pays for process startup and opening
/// the store, hundreds of milliseconds, where the write itself lands in a few
/// dozen. So wait for the data.
///
/// A daemon that failed without printing anything has nothing to wait for and
/// pays the full timeout, which is why it is short.
pub(crate) async fn wait_for_output(
    id: &DaemonId,
    since: chrono::DateTime<chrono::Local>,
    timeout: std::time::Duration,
) {
    // The cap has to wrap the whole loop, not sit between polls: a single query
    // can itself wait out the store's busy timeout, which is far longer than any
    // caller wants to pause a failed start for.
    let _ = tokio::time::timeout(timeout, settle(id, since)).await;
}

/// Poll until the daemon's output for this run stops growing.
async fn settle(id: &DaemonId, since: chrono::DateTime<chrono::Local>) {
    const POLL: std::time::Duration = std::time::Duration::from_millis(20);
    // Returning at the first row would report a daemon's first line while the
    // rest of its output was still batched in the sink, so keep going until
    // nothing new has arrived for a moment.
    const SETTLED_FOR: std::time::Duration = std::time::Duration::from_millis(80);

    let mut newest: Option<i64> = None;
    let mut last_change = tokio::time::Instant::now();

    loop {
        match newest_entry_id(id, since).await {
            Ok(latest) if latest != newest && latest.is_some() => {
                newest = latest;
                last_change = tokio::time::Instant::now();
            }
            Ok(_) => {
                if newest.is_some() && last_change.elapsed() >= SETTLED_FOR {
                    return;
                }
            }
            Err(e) => {
                // An unreadable store says nothing about whether the sink has
                // finished writing, so do not mistake it for a settled stream —
                // keep polling and let the caller's timeout end the wait.
                debug!("could not check {id}'s captured output: {e}");
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Id of the most recent entry for `id` since `since`, if any.
///
/// Deliberately fetches one row rather than counting: polling every few
/// milliseconds while a chatty daemon logs would otherwise materialize its whole
/// history each time.
async fn newest_entry_id(
    id: &DaemonId,
    since: chrono::DateTime<chrono::Local>,
) -> Result<Option<i64>> {
    let query = crate::log_store::LogQuery {
        daemon_ids: vec![id.qualified()],
        from: Some(since),
        limit: Some(1),
        order_desc: true,
        ..Default::default()
    };
    tokio::task::spawn_blocking(move || {
        crate::log_store::sqlite::LOG_STORE
            .query(&query)
            .map(|entries| entries.first().map(|entry| entry.id))
    })
    .await
    .map_err(|e| miette::miette!("log store check did not run: {e}"))?
}

/// Holds a sink that has been started but not yet handed to `supervise`, and
/// terminates it if that never happens.
///
/// `run_once` can bail out at several points between starting the sink and
/// taking charge of it — the daemon's stdio failing to wire up, its spawn
/// failing, its PID being unreadable — and each one would otherwise leave a sink
/// running with nothing writing to it. Tying cleanup to the value's lifetime
/// covers those paths, and any added later, without each having to remember.
pub(crate) struct PendingSink(Option<tokio::process::Child>);

impl PendingSink {
    pub(crate) fn new(child: tokio::process::Child) -> Self {
        Self(Some(child))
    }

    /// Give up ownership, for handing the sink to `supervise`.
    pub(crate) fn take(&mut self) -> Option<tokio::process::Child> {
        self.0.take()
    }

    /// Whether a sink is still held.
    pub(crate) fn is_some(&self) -> bool {
        self.0.is_some()
    }
}

impl Drop for PendingSink {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            // Signal synchronously rather than spawning the kill: dropping can
            // happen while the runtime is going away, and `tokio::spawn` panics
            // with no runtime to spawn onto — or silently abandons the task if
            // shutdown has already begun. `start_kill` needs neither, so the
            // signal is always delivered; collecting the exit status is left to
            // tokio's orphan reaping, since a destructor cannot await.
            let _ = child.start_kill();
        }
    }
}

/// The read end the supervisor retains so it can respawn a sink.
pub(crate) struct SinkPipe {
    reader: PipeReader,
    log_format: String,
    /// What the sink reports back, and the relay token its reports must quote.
    /// Passed to every sink started on this pipe, including replacements: a
    /// daemon that is not ready yet when its sink dies still needs its pattern
    /// watched for, and its hook still needs firing.
    watch_for: WatchFor,
    relay_token: u64,
}

impl SinkPipe {
    /// Create the pipe a daemon will write to, returning the retained read end
    /// and the write end to hand the daemon.
    pub(crate) fn new(
        log_format: String,
        watch_for: WatchFor,
        relay_token: u64,
    ) -> Result<(Self, std::io::PipeWriter)> {
        let (reader, writer) = std::io::pipe().into_diagnostic()?;
        Ok((
            Self {
                reader,
                log_format,
                watch_for,
                relay_token,
            },
            writer,
        ))
    }

    /// Start the first sink on this pipe.
    ///
    /// Called before the daemon is spawned so the reader is already running by
    /// the time output appears — and, more importantly, so a daemon that exits
    /// immediately does not have to wait for a sink to start before its output
    /// can be written and reported.
    pub(crate) fn start(&self, id: &DaemonId) -> Result<tokio::process::Child> {
        self.spawn(id)
    }

    /// Keep a sink running for as long as the daemon identified by `token` is
    /// being monitored, beginning with `first` — the process `start` returned.
    ///
    /// A sink that exits while the daemon is still supervised is replaced: it
    /// would otherwise stop draining, and the daemon would block once the pipe
    /// filled. This mirrors `runsv` restarting a service's log service.
    pub(crate) fn supervise(self, id: DaemonId, token: u64, first: tokio::process::Child) {
        tokio::spawn(async move {
            let mut child = first;
            loop {
                // Always wait on the sink already in hand before consulting the
                // registry: a daemon that exits immediately drops its monitor
                // entry before this task first runs, and checking the token
                // first would abandon the sink without ever reporting that it
                // had drained.
                match child.wait_with_output().await {
                    Ok(out) if out.status.success() => {
                        // A clean exit means end of file: the daemon and every
                        // descendant closed the pipe, so there is nothing left
                        // to capture, and everything read has been written.
                        debug!("log sink for {id} finished");
                        break;
                    }
                    Ok(out) => {
                        if !SUPERVISOR.monitor_token_valid(&id, token) {
                            break;
                        }
                        warn!(
                            "log sink for {id} exited unexpectedly ({}); restarting it",
                            out.status
                        );
                    }
                    Err(e) => {
                        if !SUPERVISOR.monitor_token_valid(&id, token) {
                            break;
                        }
                        warn!("lost track of the log sink for {id}: {e}; restarting it");
                    }
                }

                // Replace it. Keep trying while the daemon is monitored: this
                // task owns the retained read end, and giving up would leave the
                // pipe with no reader, blocking the daemon once it filled and
                // killing it with SIGPIPE once the sink's own end went too.
                let mut delay = SPAWN_RETRY_DELAY;
                child = loop {
                    match self.spawn(&id) {
                        Ok(child) => break child,
                        Err(e) => {
                            error!("failed to start log sink for {id}: {e}; retrying in {delay:?}");
                            tokio::time::sleep(delay).await;
                            delay = (delay * 2).min(SPAWN_RETRY_MAX_DELAY);
                            if !SUPERVISOR.monitor_token_valid(&id, token) {
                                return;
                            }
                        }
                    }
                };
            }
        });
    }

    /// Spawn one sink process reading a duplicate of the retained read end.
    fn spawn(&self, id: &DaemonId) -> Result<tokio::process::Child> {
        let reader = self.reader.try_clone().into_diagnostic()?;
        let mut cmd = tokio::process::Command::new(&*crate::env::PITCHFORK_BIN);
        cmd.arg("log-sink")
            .arg("--daemon-id")
            .arg(id.qualified())
            .arg("--log-format")
            .arg(&self.log_format);
        cmd.args(self.watch_for.args(self.relay_token));
        cmd.stdin(std::process::Stdio::from(reader))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .into_diagnostic()
    }
}
