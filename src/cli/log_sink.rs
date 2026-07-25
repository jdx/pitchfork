use crate::Result;
use crate::daemon_id::DaemonId;
use crate::log_store::LogStore;
use crate::log_store::sqlite::LOG_STORE;
use tokio::io::AsyncBufReadExt;

/// Number of parsed lines to accumulate before writing them as one batch.
const BATCH_SIZE: usize = 100;

/// Longest a parsed line waits in the batch before being written.
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Reads a daemon's output on stdin and writes it to the log store
///
/// Spawned by the supervisor as a sibling of the daemon, holding the read end
/// of the daemon's output pipe. Keeping the reader in its own process is what
/// makes logging survive a supervisor crash: the pipe still has a reader, so
/// the daemon is neither killed by SIGPIPE nor blocked, and no output is lost.
/// Exits when the pipe reaches end of file, which happens once the daemon and
/// every descendant holding the write end have gone.
#[derive(Debug, clap::Args)]
#[clap(hide = true, verbatim_doc_comment)]
pub struct LogSink {
    /// Qualified id of the daemon whose output this is
    #[clap(long)]
    daemon_id: String,

    /// Log format to parse lines with (`json`, `logfmt`, `auto`, or `text`)
    #[clap(long, default_value = "text")]
    log_format: String,
}

impl LogSink {
    pub async fn run(&self) -> Result<()> {
        let id = DaemonId::parse(&self.daemon_id)?;
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        let mut batch: Vec<crate::log_parse::ParsedLog> = Vec::with_capacity(BATCH_SIZE);

        // Batch writes, but never hold a line longer than this. Waiting for a
        // full batch would make a daemon that logs a few lines a second appear
        // silent for many seconds.
        let mut flush_interval = tokio::time::interval(FLUSH_INTERVAL);
        flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            batch.push(crate::log_parse::parse(&line, &self.log_format));
                            if batch.len() >= BATCH_SIZE {
                                flush(&id, &mut batch);
                            }
                        }
                        // End of file: every write end is closed, so the daemon
                        // is gone and there will be nothing more to read.
                        Ok(None) => break,
                        Err(e) => {
                            // Invalid UTF-8 or a broken pipe: record it and
                            // stop rather than spin on an unreadable stream.
                            debug!("log sink for {id} stopping: {e}");
                            break;
                        }
                    }
                }
                _ = flush_interval.tick() => flush(&id, &mut batch),
            }
        }

        flush(&id, &mut batch);
        Ok(())
    }
}

fn flush(id: &DaemonId, batch: &mut Vec<crate::log_parse::ParsedLog>) {
    if batch.is_empty() {
        return;
    }
    if let Err(e) = LOG_STORE.append_structured_batch(id, batch) {
        // Nothing useful to do but report it: the supervisor is not
        // necessarily alive to be told, and dropping the batch is preferable
        // to stalling the daemon behind a full pipe.
        error!("log sink failed to write batch for {id}: {e}");
    }
    batch.clear();
}
