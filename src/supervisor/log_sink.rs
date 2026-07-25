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

use crate::Result;
use crate::daemon::RunOptions;
use crate::daemon_id::DaemonId;
use crate::supervisor::SUPERVISOR;
use miette::IntoDiagnostic;
use std::io::PipeReader;

/// Whether `opts` describes a daemon whose output can be captured by a sink.
///
/// Excluded for now, both because the supervisor itself has to read the stream
/// to serve them:
///
/// - `ready_output`, which decides readiness by matching output
/// - an `on_output` hook, which fires per matching line
///
/// and PTY daemons, whose output arrives on a terminal master rather than a
/// pipe. Those keep the in-process path, so they remain vulnerable to a
/// supervisor crash until the sink learns to evaluate them and report matches
/// back.
pub(crate) fn is_supported(opts: &RunOptions) -> bool {
    opts.ready_output.is_none() && opts.on_output_hook.is_none() && !opts.pty.unwrap_or(false)
}

/// Resolves once a sink has read its pipe to end of file and written what it
/// read. Used to order start-failure diagnostics after the sink's final write,
/// which the in-process capture path guaranteed by flushing synchronously.
pub(crate) struct Drained(tokio::sync::oneshot::Receiver<()>);

impl Drained {
    /// Wait for the final write, giving up after `timeout` so a daemon whose
    /// descendants still hold the pipe open cannot stall the caller.
    pub(crate) async fn wait(self, timeout: std::time::Duration) {
        let _ = tokio::time::timeout(timeout, self.0).await;
    }
}

/// The read end the supervisor retains so it can respawn a sink.
pub(crate) struct SinkPipe {
    reader: PipeReader,
    log_format: String,
}

impl SinkPipe {
    /// Create the pipe a daemon will write to, returning the retained read end
    /// and the write end to hand the daemon.
    pub(crate) fn new(log_format: String) -> Result<(Self, std::io::PipeWriter)> {
        let (reader, writer) = std::io::pipe().into_diagnostic()?;
        Ok((Self { reader, log_format }, writer))
    }

    /// Start a sink on this pipe and keep one running for as long as the daemon
    /// identified by `token` is being monitored.
    ///
    /// A sink that exits while the daemon is still supervised is replaced: it
    /// would otherwise stop draining, and the daemon would block once the pipe
    /// filled. This mirrors `runsv` restarting a service's log service.
    ///
    /// The returned `Drained` resolves once a sink has read to end of file and
    /// written what it had, which is what start-failure diagnostics wait for
    /// before querying the log store.
    pub(crate) fn supervise(self, id: DaemonId, token: u64) -> Drained {
        let (drained_tx, drained_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut drained_tx = Some(drained_tx);
            loop {
                if !SUPERVISOR.monitor_token_valid(&id, token) {
                    break;
                }
                let child = match self.spawn(&id) {
                    Ok(child) => child,
                    Err(e) => {
                        error!("failed to start log sink for {id}: {e}");
                        break;
                    }
                };
                match child.wait_with_output().await {
                    Ok(out) if out.status.success() => {
                        // A clean exit means end of file: the daemon and every
                        // descendant closed the pipe, so there is nothing left
                        // to capture, and everything read has been written.
                        debug!("log sink for {id} finished");
                        if let Some(tx) = drained_tx.take() {
                            let _ = tx.send(());
                        }
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
            }
        });
        Drained(drained_rx)
    }

    /// Spawn one sink process reading a duplicate of the retained read end.
    fn spawn(&self, id: &DaemonId) -> Result<tokio::process::Child> {
        let reader = self.reader.try_clone().into_diagnostic()?;
        tokio::process::Command::new(&*crate::env::PITCHFORK_BIN)
            .arg("log-sink")
            .arg("--daemon-id")
            .arg(id.qualified())
            .arg("--log-format")
            .arg(&self.log_format)
            .stdin(std::process::Stdio::from(reader))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .into_diagnostic()
    }
}
