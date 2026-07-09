use crate::Result;
use crate::daemon_id::DaemonId;
use crate::log_parse::ParsedLog;
use chrono::{DateTime, Local};

/// A single log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub id: i64,
    pub daemon_id: String,
    pub timestamp: DateTime<Local>,
    pub message: String,
    /// Normalized log level (`error`/`warn`/`info`/`debug`/`trace`), or `None`
    /// for unstructured log lines.
    pub level: Option<String>,
    /// Extracted human-readable message (from `msg`/`message`/`event`/...).
    pub msg: Option<String>,
    /// Logger name (from `logger`/`name`/`component`/...).
    pub logger: Option<String>,
    /// The full parsed JSON object as a string, for `json_extract` queries.
    /// `None` for plain-text lines that were not parsed.
    pub fields_json: Option<String>,
}

/// A filter applied to the message text of log entries.
#[derive(Debug, Clone)]
pub enum MessageFilter {
    /// Case-insensitive substring match using SQLite LIKE.
    Contains {
        pattern: String,
        case_sensitive: bool,
    },
    /// Regular expression match using SQLite REGEXP.
    Regex { pattern: String },
}

/// A filter applied to structured log fields.
#[derive(Debug, Clone)]
pub enum FieldFilter {
    /// Match entries with a specific normalized level.
    LevelEq(String),
    /// Match entries where `json_extract(fields_json, '$.key') = value`.
    FieldEq { key: String, value: String },
}

impl MessageFilter {
    #[allow(dead_code)]
    pub fn contains(pattern: impl Into<String>) -> Self {
        Self::Contains {
            pattern: pattern.into(),
            case_sensitive: false,
        }
    }

    #[allow(dead_code)]
    pub fn contains_case_sensitive(pattern: impl Into<String>) -> Self {
        Self::Contains {
            pattern: pattern.into(),
            case_sensitive: true,
        }
    }

    #[allow(dead_code)]
    pub fn regex(pattern: impl Into<String>) -> Self {
        Self::Regex {
            pattern: pattern.into(),
        }
    }
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
    /// Filters applied to the message text. Multiple filters are combined with OR.
    pub message_filters: Vec<MessageFilter>,
    /// Filters applied to structured fields. Multiple filters are combined with AND.
    pub field_filters: Vec<FieldFilter>,
    /// Whether to SELECT the structured columns (level, msg, logger, fields_json).
    /// When false, those fields are NULL in the result, avoiding unnecessary
    /// string allocations for callers that only need the raw message.
    pub include_structured: bool,
}

/// Escape special LIKE wildcard characters so user-supplied substrings are matched literally.
pub fn escape_like_pattern(pattern: &str) -> String {
    pattern
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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

/// Hook invoked before log entries are pruned by retention.
#[derive(Debug, Clone)]
pub struct ArchiveHook {
    /// Shell command to run. It should read JSONL from stdin.
    pub command: String,
    /// Maximum number of log entries to pass to a single hook invocation.
    pub batch_size: usize,
}

impl ArchiveHook {
    /// Returns true if the hook is configured with a non-empty command.
    pub fn is_enabled(&self) -> bool {
        !self.command.trim().is_empty()
    }
}

/// Unified interface for log storage and retrieval.
pub trait LogStore: Send + Sync {
    /// Append a single log line (unstructured).
    fn append(&self, daemon_id: &DaemonId, message: &str) -> Result<()>;

    /// Append a single parsed log line with structured fields.
    ///
    /// The default implementation discards structured fields and falls back
    /// to `append`.
    fn append_structured(&self, daemon_id: &DaemonId, parsed: &ParsedLog) -> Result<()> {
        self.append(daemon_id, &parsed.message)
    }

    /// Append multiple parsed log lines in a single transaction.
    ///
    /// The default implementation calls `append_structured` for each line.
    fn append_structured_batch(&self, daemon_id: &DaemonId, entries: &[ParsedLog]) -> Result<()> {
        for entry in entries {
            self.append_structured(daemon_id, entry)?;
        }
        Ok(())
    }

    /// Append multiple log lines in a single transaction.
    ///
    /// The default implementation falls back to calling `append` for each line.
    #[allow(dead_code)]
    fn append_batch(&self, daemon_id: &DaemonId, messages: &[String]) -> Result<()> {
        for msg in messages {
            self.append(daemon_id, msg)?;
        }
        Ok(())
    }

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
        archive_hook: Option<&ArchiveHook>,
    ) -> Result<u64> {
        let _ = (policy, excluded_daemon_ids, archive_hook);
        Ok(0)
    }

    /// Apply a retention policy to a specific daemon's logs.
    fn apply_retention_for_daemon(
        &self,
        daemon_id: &DaemonId,
        policy: &RetentionPolicy,
        archive_hook: Option<&ArchiveHook>,
    ) -> Result<u64> {
        let _ = (daemon_id, policy, archive_hook);
        Ok(0)
    }

    /// Return the highest row id for the given daemon, or `None` if no
    /// entries exist. Used by tailing loops to advance the cursor past
    /// non-matching rows without re-scanning them on every poll.
    fn last_id(&self, daemon_id: &DaemonId) -> Result<Option<i64>> {
        let entries = self.query(&LogQuery {
            daemon_ids: vec![daemon_id.qualified()],
            from: None,
            to: None,
            limit: Some(1),
            order_desc: true,
            after_id: None,
            message_filters: Vec::new(),
            field_filters: Vec::new(),
            include_structured: false,
        })?;
        Ok(entries.first().map(|e| e.id))
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
