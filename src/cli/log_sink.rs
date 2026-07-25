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
        let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
        let mut batch: Vec<crate::log_parse::ParsedLog> = Vec::with_capacity(BATCH_SIZE);
        // Read bytes and convert lossily rather than requiring valid UTF-8. A
        // daemon that emits a stray non-UTF-8 byte must not be able to stop its
        // own logging, and an error here would be reported as a clean exit —
        // which the supervisor reads as end of file and stops replacing the
        // sink, eventually blocking the daemon behind a full pipe.
        let mut buf: Vec<u8> = Vec::new();

        // Batch writes, but never hold a line longer than this. Waiting for a
        // full batch would make a daemon that logs a few lines a second appear
        // silent for many seconds.
        let mut flush_interval = tokio::time::interval(FLUSH_INTERVAL);
        flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                read = reader.read_until(b'\n', &mut buf) => {
                    match read {
                        // End of file: every write end is closed, so the daemon
                        // is gone and there will be nothing more to read.
                        Ok(0) => break,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buf);
                            let line = line.trim_end_matches(['\n', '\r']);
                            batch.push(crate::log_parse::parse(line, &self.log_format));
                            buf.clear();
                            if batch.len() >= BATCH_SIZE {
                                flush(&id, &mut batch);
                            }
                        }
                        Err(e) => {
                            // A real read failure, not a decoding problem. Write
                            // what we have and exit non-zero so the supervisor
                            // replaces this sink rather than treating the stream
                            // as finished.
                            flush(&id, &mut batch);
                            return Err(miette::miette!(
                                "log sink for {id} could not read the daemon's output: {e}"
                            ));
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
