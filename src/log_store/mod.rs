use crate::Result;
use crate::daemon_id::DaemonId;
use chrono::{DateTime, Local};

/// A single log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub id: i64,
    pub daemon_id: String,
    pub timestamp: DateTime<Local>,
    pub message: String,
}

/// Options for querying logs.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    pub daemon_ids: Vec<String>,
    pub from: Option<DateTime<Local>>,
    pub to: Option<DateTime<Local>>,
    pub limit: Option<usize>,
    pub order_desc: bool,
    pub after_id: Option<i64>,
}

/// Parsed retention policy.
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    /// Maximum age of entries to keep.
    pub age: Option<chrono::Duration>,
    /// Maximum number of entries to keep.
    pub count: Option<u64>,
}

impl RetentionPolicy {
    #[allow(dead_code)]
    pub fn is_none(&self) -> bool {
        self.age.is_none() && self.count.is_none()
    }
}

/// Unified interface for log storage and retrieval.
pub trait LogStore: Send + Sync {
    /// Append a single log line.
    fn append(&self, daemon_id: &DaemonId, message: &str) -> Result<()>;

    /// Query logs according to the given options.
    fn query(&self, opts: &LogQuery) -> Result<Vec<LogEntry>>;

    /// Read new log entries for a daemon.
    /// When `after_id` is Some(id), returns only entries with row id > id.
    /// When `after_id` is None, returns all entries for the daemon.
    fn tail(&self, daemon_id: &DaemonId, after_id: Option<i64>) -> Result<Vec<LogEntry>>;

    /// Clear all logs for the given daemon(s).
    fn clear(&self, daemon_ids: &[DaemonId]) -> Result<()>;

    /// Apply a retention policy (age-based and/or count-based pruning).
    ///
    /// By default this applies to all daemons; `excluded_daemon_ids` can be
    /// used to skip daemons that have their own per-daemon overrides, so the
    /// global policy does not accidentally prune entries those daemons intend
    /// to keep.
    fn apply_retention(
        &self,
        policy: &RetentionPolicy,
        excluded_daemon_ids: &[DaemonId],
    ) -> Result<u64> {
        let _ = (policy, excluded_daemon_ids);
        Ok(0)
    }

    /// Apply a retention policy to a specific daemon's logs.
    fn apply_retention_for_daemon(
        &self,
        daemon_id: &DaemonId,
        policy: &RetentionPolicy,
    ) -> Result<u64> {
        let _ = (daemon_id, policy);
        Ok(0)
    }

    /// List all daemon IDs that have log entries.
    fn list_daemon_ids(&self) -> Result<Vec<String>>;

    /// Return the generation number for the daemon's last clear operation.
    /// Each call to `clear` bumps the generation, so SSE streams can detect
    /// when logs have been wiped and refresh their display.
    fn last_clear_generation(&self, daemon_id: &DaemonId) -> Result<Option<u64>> {
        let _ = daemon_id;
        Ok(None)
    }
}

pub mod sqlite;
