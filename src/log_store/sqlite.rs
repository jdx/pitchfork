use crate::Result;
use crate::daemon_id::DaemonId;
use crate::log_parse::ParsedLog;
use crate::log_store::{
    ArchiveHook, FieldFilter, LogEntry, LogQuery, LogStore, MessageFilter, escape_like_pattern,
};
use chrono::{DateTime, Local, TimeZone};
use log::error;
use miette::IntoDiagnostic;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

/// Registers a `regexp` SQL function backed by the `regex` crate.
///
/// SQLite does not ship a REGEXP implementation by default. This function
/// is registered on every new connection so that `message REGEXP ?` works
/// consistently across queries.
fn add_regexp_function(conn: &Connection) -> Result<()> {
    use std::cell::RefCell;

    // Cache compiled regexes per connection to avoid recompiling the same
    // pattern on every row evaluation. The cache is small and thread-local
    // because scalar functions are invoked on the connection's thread.
    let cache: RefCell<lru::LruCache<String, regex::Regex>> =
        RefCell::new(lru::LruCache::new(std::num::NonZeroUsize::new(32).unwrap()));

    conn.create_scalar_function(
        "regexp",
        2,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            let pattern: String = ctx.get(0)?;
            let text: String = ctx.get(1)?;

            let mut cache = cache.borrow_mut();
            let re = match cache.get(&pattern) {
                Some(re) => re.clone(),
                None => {
                    let re = regex::Regex::new(&pattern)
                        .map_err(|e| rusqlite::Error::UserFunctionError(e.to_string().into()))?;
                    cache.put(pattern.clone(), re.clone());
                    re
                }
            };
            Ok(re.is_match(&text))
        },
    )
    .into_diagnostic()
}

/// SQLite-backed log store with WAL mode for concurrent readers.
pub struct SqliteLogStore {
    conn: Mutex<Connection>,
    path: PathBuf,
}

/// Minimum result-set size to trigger parallel query.
///
/// Each parallel shard opens a new read-only SQLite connection (~5-12ms
/// overhead per connection for PRAGMA setup + regexp function registration).
/// Below this threshold, single-threaded is faster because the connection
/// overhead exceeds the query savings.
const PARALLEL_QUERY_THRESHOLD: usize = 200_000;

impl SqliteLogStore {
    /// Open or create the SQLite log store at the given path.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).into_diagnostic()?;
        }
        let conn = Connection::open(&path).into_diagnostic()?;
        add_regexp_function(&conn)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA mmap_size = 268435456;",
        )
        .into_diagnostic()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS log_entries (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                daemon_id   TEXT    NOT NULL,
                timestamp   INTEGER NOT NULL,
                message     TEXT    NOT NULL,
                level       TEXT,
                msg         TEXT,
                logger      TEXT,
                fields_json TEXT
            );",
            [],
        )
        .into_diagnostic()?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_daemon_ts ON log_entries(daemon_id, timestamp);",
            [],
        )
        .into_diagnostic()?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_daemon_id ON log_entries(daemon_id, id);",
            [],
        )
        .into_diagnostic()?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON log_entries(timestamp);",
            [],
        )
        .into_diagnostic()?;

        // Migrate existing tables: add columns introduced in this version.
        // Must run BEFORE creating indexes that reference the new columns.
        let existing_cols: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(log_entries)")
                .into_diagnostic()?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .into_diagnostic()?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for col in ["level", "msg", "logger", "fields_json"] {
            if !existing_cols.iter().any(|c| c == col) {
                conn.execute(
                    &format!("ALTER TABLE log_entries ADD COLUMN {col} TEXT"),
                    [],
                )
                .into_diagnostic()?;
            }
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_daemon_level_ts ON log_entries(daemon_id, level, timestamp);",
            [],
        )
        .into_diagnostic()?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS log_clear_generations (
                daemon_id TEXT PRIMARY KEY,
                generation INTEGER NOT NULL DEFAULT 0
            );",
            [],
        )
        .into_diagnostic()?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<LogEntry> {
        let id: i64 = row.get(0)?;
        let daemon_id: String = row.get(1)?;
        let ts_millis: i64 = row.get(2)?;
        let message: String = row.get(3)?;
        let level: Option<String> = row.get(4)?;
        let msg: Option<String> = row.get(5)?;
        let logger: Option<String> = row.get(6)?;
        let fields_json: Option<String> = row.get(7)?;
        let timestamp = Local
            .timestamp_millis_opt(ts_millis)
            .single()
            .unwrap_or_else(Local::now);
        Ok(LogEntry {
            id,
            daemon_id,
            timestamp,
            message,
            level,
            msg,
            logger,
            fields_json,
        })
    }

    fn archive_entries(
        &self,
        entries: &[LogEntry],
        archive_hook: &ArchiveHook,
        daemon_id: &DaemonId,
        reason: &str,
    ) -> Result<()> {
        use std::process::{Command, Stdio};

        if entries.is_empty() {
            return Ok(());
        }

        for chunk in entries.chunks(archive_hook.batch_size.max(1)) {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(&archive_hook.command)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .env("PITCHFORK_DAEMON_ID", daemon_id.qualified())
                .env("PITCHFORK_ARCHIVE_REASON", reason)
                .spawn()
                .into_diagnostic()
                .map_err(|e| miette::miette!("failed to spawn archive hook: {e}"))?;

            // Write entries to stdin. On error, kill and reap the child
            // before propagating — Child::drop does not call wait(), so
            // without this a crashed hook would leave a zombie process.
            let write_result = {
                let stdin = child.stdin.take().expect("piped stdin should be available");
                let mut stdin = std::io::BufWriter::new(stdin);
                let mut result = Ok(());
                for entry in chunk {
                    let line = serde_json::json!({
                        "id": entry.id,
                        "daemon_id": entry.daemon_id,
                        "timestamp": entry.timestamp.to_rfc3339(),
                        "message": entry.message,
                    });
                    if let Err(e) = writeln!(stdin, "{}", line) {
                        result = Err(miette::miette!(
                            "failed to write to archive hook stdin: {e}"
                        ));
                        break;
                    }
                }
                // Explicitly flush so a buffer-drain failure (e.g. the hook
                // exited early and closed stdin) is surfaced as an error
                // rather than being silently swallowed by BufWriter::drop.
                // On a small batch where all data fits in the 8 KB buffer no
                // individual writeln! has touched the underlying pipe yet, so
                // without this flush the failure would be lost and the entries
                // deleted without ever being delivered to the hook.
                if result.is_ok()
                    && let Err(e) = stdin.flush()
                {
                    result = Err(miette::miette!("failed to flush archive hook stdin: {e}"));
                }
                result
                // BufWriter + ChildStdin drop here, closing stdin (EOF signal).
            };

            if let Err(e) = write_result {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }

            let output = child.wait_with_output().into_diagnostic()?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(miette::miette!(
                    "archive hook failed with status {}: {stderr}",
                    output.status
                ));
            }
        }

        Ok(())
    }

    /// Delete rows matching the given IDs, returning the number of rows deleted.
    ///
    /// Chunks at 999 to stay within SQLite's `SQLITE_MAX_VARIABLE_NUMBER`
    /// limit on versions ≤ 3.31 (e.g. Ubuntu 20.04 LTS).
    fn delete_by_ids(&self, ids: &[i64]) -> Result<u64> {
        const SQLITE_MAX_VARS: usize = 999;

        let mut total = 0u64;
        let conn = self.conn.lock().unwrap();
        for chunk in ids.chunks(SQLITE_MAX_VARS) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "DELETE FROM log_entries WHERE id IN ({})",
                placeholders.join(", ")
            );
            total += conn
                .execute(&sql, rusqlite::params_from_iter(chunk.iter()))
                .into_diagnostic()? as u64;
        }
        Ok(total)
    }

    /// Rotate (delete) old log entries for a specific daemon based on retention policy.
    ///
    /// When an archive hook is configured, entries are fetched in batches of
    /// `batch_size`, the hook is invoked *without* holding the SQLite mutex
    /// (so concurrent log appends are not blocked), and then deleted by ID.
    /// Row-read errors are propagated rather than silently dropped.
    pub fn rotate_by_age(
        &self,
        daemon_id: &DaemonId,
        max_age: chrono::Duration,
        archive_hook: Option<&ArchiveHook>,
    ) -> Result<u64> {
        let cutoff = (Local::now() - max_age).timestamp_millis();
        let hook = archive_hook.filter(|h| h.is_enabled());

        if let Some(hook) = hook {
            let mut total_deleted = 0u64;
            loop {
                // Fetch one batch under lock, then release.
                let entries: Vec<LogEntry> = {
                    let conn = self.conn.lock().unwrap();
                    let mut stmt = conn
                        .prepare(
                            "SELECT id, daemon_id, timestamp, message, level, msg, logger, fields_json FROM log_entries
                             WHERE daemon_id = ?1 AND timestamp < ?2
                             ORDER BY timestamp ASC, id ASC
                             LIMIT ?3",
                        )
                        .into_diagnostic()?;
                    stmt.query_map(
                        params![daemon_id.qualified(), cutoff, hook.batch_size as i64],
                        Self::row_to_entry,
                    )
                    .into_diagnostic()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .into_diagnostic()?
                };

                if entries.is_empty() {
                    break;
                }

                let batch_ids: Vec<i64> = entries.iter().map(|e| e.id).collect();

                // Run the hook without holding the mutex.
                self.archive_entries(&entries, hook, daemon_id, "age")?;

                // Re-acquire lock and delete exactly the archived IDs.
                let deleted = self.delete_by_ids(&batch_ids)?;
                total_deleted += deleted;
            }
            Ok(total_deleted)
        } else {
            let conn = self.conn.lock().unwrap();
            let rows = conn
                .execute(
                    "DELETE FROM log_entries WHERE daemon_id = ?1 AND timestamp < ?2",
                    params![daemon_id.qualified(), cutoff],
                )
                .into_diagnostic()?;
            Ok(rows as u64)
        }
    }

    /// Rotate (delete) old log entries keeping only the most recent `max_count` rows
    /// for a specific daemon.
    ///
    /// When an archive hook is configured, entries are fetched in batches of
    /// `batch_size`, the hook is invoked *without* holding the SQLite mutex
    /// (so concurrent log appends are not blocked), and then deleted by ID.
    /// Row-read errors are propagated rather than silently dropped.
    pub fn rotate_by_count(
        &self,
        daemon_id: &DaemonId,
        max_count: u64,
        archive_hook: Option<&ArchiveHook>,
    ) -> Result<u64> {
        let hook = archive_hook.filter(|h| h.is_enabled());

        // Determine how many entries to delete.
        let to_delete: i64 = {
            let conn = self.conn.lock().unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM log_entries WHERE daemon_id = ?1",
                    [daemon_id.qualified()],
                    |row| row.get(0),
                )
                .into_diagnostic()?;
            count.saturating_sub(max_count as i64)
        };

        if to_delete <= 0 {
            return Ok(0);
        }

        if let Some(hook) = hook {
            let mut total_deleted = 0u64;
            let mut remaining = to_delete;
            loop {
                let batch_len = remaining.min(hook.batch_size as i64);

                // Fetch one batch under lock, then release.
                let entries: Vec<LogEntry> = {
                    let conn = self.conn.lock().unwrap();
                    let mut stmt = conn
                        .prepare(
                            "SELECT id, daemon_id, timestamp, message, level, msg, logger, fields_json FROM log_entries
                             WHERE daemon_id = ?1
                             ORDER BY timestamp ASC, id ASC
                             LIMIT ?2",
                        )
                        .into_diagnostic()?;
                    stmt.query_map(
                        params![daemon_id.qualified(), batch_len],
                        Self::row_to_entry,
                    )
                    .into_diagnostic()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .into_diagnostic()?
                };

                if entries.is_empty() {
                    break;
                }

                let fetched = entries.len() as i64;
                let batch_ids: Vec<i64> = entries.iter().map(|e| e.id).collect();

                // Run the hook without holding the mutex.
                self.archive_entries(&entries, hook, daemon_id, "count")?;

                // Re-acquire lock and delete exactly the archived IDs.
                let deleted = self.delete_by_ids(&batch_ids)?;
                total_deleted += deleted;
                remaining -= fetched;
            }
            Ok(total_deleted)
        } else {
            let conn = self.conn.lock().unwrap();
            let rows = conn
                .execute(
                    "DELETE FROM log_entries WHERE id IN (
                        SELECT id FROM log_entries WHERE daemon_id = ?1
                        ORDER BY timestamp ASC, id ASC LIMIT ?2
                    )",
                    params![daemon_id.qualified(), to_delete],
                )
                .into_diagnostic()?;
            Ok(rows as u64)
        }
    }

    /// Migrate existing text logs for a daemon into SQLite.
    ///
    /// Reads the legacy text file line-by-line (streaming) and inserts in
    /// batches of 1000 to avoid loading multi-GB files into memory at once.
    pub fn migrate_daemon_text_logs(&self, daemon_id: &DaemonId) -> Result<u64> {
        let text_path = daemon_id.log_path();
        if !text_path.exists() {
            return Ok(0);
        }

        let file = std::fs::File::open(&text_path).into_diagnostic()?;
        let reader = BufReader::new(file);
        let re = regex::Regex::new(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) ([\w./-]+) (.*)$")
            .expect("invalid regex");

        let mut current_timestamp: Option<DateTime<Local>> = None;
        let mut current_message = String::new();
        let mut entries = Vec::with_capacity(1000);
        let mut total_migrated: u64 = 0;

        for line in reader.lines() {
            let line = line.into_diagnostic()?;
            if let Some(caps) = re.captures(&line) {
                if let Some(ts) = current_timestamp.take() {
                    entries.push((ts, std::mem::take(&mut current_message)));
                }
                let ts_str = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
                let msg = caps.get(3).map(|m| m.as_str()).unwrap_or_default();
                if let Ok(naive) =
                    chrono::NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S")
                {
                    current_timestamp = Local.from_local_datetime(&naive).single();
                    current_message = msg.to_string();
                }
            } else if current_timestamp.is_some() {
                current_message.push('\n');
                current_message.push_str(&line);
            }

            if entries.len() >= 1000 {
                total_migrated += self.insert_batch(daemon_id, &entries)?;
                entries.clear();
            }
        }

        if let Some(ts) = current_timestamp {
            entries.push((ts, std::mem::take(&mut current_message)));
        }

        if !entries.is_empty() {
            total_migrated += self.insert_batch(daemon_id, &entries)?;
        }

        if total_migrated > 0
            && let Err(e) = std::fs::remove_file(&text_path)
        {
            log::warn!(
                "failed to remove legacy log file after migration {}: {e}",
                text_path.display()
            );
        }

        Ok(total_migrated)
    }

    fn insert_batch(
        &self,
        daemon_id: &DaemonId,
        entries: &[(DateTime<Local>, String)],
    ) -> Result<u64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().into_diagnostic()?;
        let mut count = 0u64;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO log_entries (daemon_id, timestamp, message) VALUES (?1, ?2, ?3)",
                )
                .into_diagnostic()?;
            for (ts, msg) in entries {
                stmt.execute(params![daemon_id.qualified(), ts.timestamp_millis(), msg])
                    .into_diagnostic()?;
                count += 1;
            }
        }
        tx.commit().into_diagnostic()?;
        Ok(count)
    }

    /// Build the SQL query string and parameters for the given options.
    ///
    /// `id_range` is used by `query_parallel` to shard the query by id range.
    /// When `Some((start, end))`, adds `id > start AND id <= end` to the WHERE clause.
    fn build_query_sql(
        opts: &LogQuery,
        id_range: Option<(i64, i64)>,
    ) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
        let mut conditions = Vec::new();
        let mut query_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if !opts.daemon_ids.is_empty() {
            let placeholders: Vec<String> = (1..=opts.daemon_ids.len())
                .map(|i| format!("?{}", i))
                .collect();
            conditions.push(format!("daemon_id IN ({})", placeholders.join(", ")));
            for id in &opts.daemon_ids {
                query_params.push(Box::new(id.clone()));
            }
        }

        if let Some(from) = opts.from {
            conditions.push(format!("timestamp >= ?{}", query_params.len() + 1));
            query_params.push(Box::new(from.timestamp_millis()));
        }

        if let Some(to) = opts.to {
            conditions.push(format!("timestamp <= ?{}", query_params.len() + 1));
            query_params.push(Box::new(to.timestamp_millis()));
        }

        if let Some(after_id) = opts.after_id {
            conditions.push(format!("id > ?{}", query_params.len() + 1));
            query_params.push(Box::new(after_id));
        }

        if let Some((start, end)) = id_range {
            conditions.push(format!("id > ?{}", query_params.len() + 1));
            query_params.push(Box::new(start));
            conditions.push(format!("id <= ?{}", query_params.len() + 1));
            query_params.push(Box::new(end));
        }

        let mut message_conditions = Vec::new();
        for filter in &opts.message_filters {
            match filter {
                MessageFilter::Contains {
                    pattern,
                    case_sensitive,
                } => {
                    let param_index = query_params.len() + 1;
                    if *case_sensitive {
                        message_conditions
                            .push(format!("INSTR(message, ?{idx}) > 0", idx = param_index));
                        query_params.push(Box::new(pattern.clone()));
                    } else {
                        let escaped = escape_like_pattern(pattern);
                        let param = format!("%{}%", escaped);
                        message_conditions.push(format!(
                            "LOWER(message) LIKE LOWER(?{idx}) ESCAPE '\\'",
                            idx = param_index
                        ));
                        query_params.push(Box::new(param));
                    }
                }
                MessageFilter::Regex { pattern } => {
                    let param_index = query_params.len() + 1;
                    message_conditions.push(format!("message REGEXP ?{param_index}"));
                    query_params.push(Box::new(pattern.clone()));
                }
            }
        }
        if !message_conditions.is_empty() {
            conditions.push(format!("({})", message_conditions.join(" OR ")));
        }

        for filter in &opts.field_filters {
            match filter {
                FieldFilter::LevelMin(level) => {
                    let matching = crate::log_store::levels_at_or_above(level);
                    if matching.is_empty() {
                        // Unknown level: no results
                        conditions.push("0".to_string());
                    } else {
                        let placeholders = matching
                            .iter()
                            .map(|l| {
                                let idx = query_params.len() + 1;
                                query_params.push(Box::new((*l).to_string()));
                                format!("?{idx}")
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        conditions.push(format!("level IN ({placeholders})"));
                    }
                }
                FieldFilter::FieldEq { key, value } => {
                    let param_index = query_params.len() + 1;
                    conditions.push(format!(
                        "json_extract(fields_json, '$.{key}') = ?{param_index}"
                    ));
                    query_params.push(Box::new(value.clone()));
                }
            }
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let order = if opts.order_desc { "DESC" } else { "ASC" };

        let limit_clause = opts
            .limit
            .map(|n| format!("LIMIT {}", n))
            .unwrap_or_default();

        let columns = if opts.include_structured {
            "id, daemon_id, timestamp, message, level, msg, logger, fields_json"
        } else {
            "id, daemon_id, timestamp, message, NULL, NULL, NULL, NULL"
        };

        let sql = format!(
            "SELECT {columns} FROM log_entries {} ORDER BY timestamp {}, id {} {}",
            where_clause, order, order, limit_clause
        );

        (sql, query_params)
    }

    /// Returns true if parallel query is beneficial for the given options.
    fn should_parallelize(opts: &LogQuery) -> bool {
        if opts.daemon_ids.len() != 1 {
            return false;
        }
        // Skip parallel for incremental tail polls (after_id set) — they
        // return small batches and the connection overhead dominates.
        if opts.after_id.is_some() {
            return false;
        }
        let limit = opts.limit.unwrap_or(usize::MAX);
        if limit < PARALLEL_QUERY_THRESHOLD {
            return false;
        }
        std::thread::available_parallelism()
            .map(|n| n.get() >= 2)
            .unwrap_or(false)
    }

    /// Query using multiple read-only connections, sharded by id range.
    ///
    /// For a single daemon, id order ≈ timestamp order (timestamps are
    /// assigned in insertion order within a single writer). This lets us
    /// shard by contiguous id ranges and merge by concatenating shards in
    /// the right order, without a global sort.
    fn query_parallel(&self, opts: &LogQuery) -> Result<Vec<LogEntry>> {
        // Cap parallelism: each shard opens a new read-only connection with
        // PRAGMA setup + regexp function registration (~5-12ms each). Beyond
        // 2 threads the connection overhead dominates the query savings for
        // typical log store sizes.
        let max_threads = 2;
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get().min(max_threads))
            .unwrap_or(1);

        let max_id: Option<i64> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT MAX(id) FROM log_entries WHERE daemon_id = ?1",
                params![&opts.daemon_ids[0]],
                |row| row.get(0),
            )
            .ok()
        };

        let Some(max_id) = max_id else {
            return Ok(Vec::new());
        };
        if max_id == 0 {
            return Ok(Vec::new());
        }

        let shard_size = (max_id as usize).div_ceil(num_threads);
        let path = self.path.clone();
        let opts = opts.clone();
        let needs_regexp = opts
            .message_filters
            .iter()
            .any(|f| matches!(f, MessageFilter::Regex { .. }));

        let shards: Vec<Result<Vec<LogEntry>>> = std::thread::scope(|s| {
            (0..num_threads)
                .map(|i| {
                    let start = (i * shard_size) as i64;
                    let end = if i == num_threads - 1 {
                        max_id
                    } else {
                        ((i + 1) * shard_size) as i64
                    };
                    let opts = opts.clone();
                    let path = path.clone();
                    s.spawn(move || -> Result<Vec<LogEntry>> {
                        let conn = Connection::open_with_flags(
                            &path,
                            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                        )
                        .into_diagnostic()?;
                        conn.execute_batch(
                            "PRAGMA mmap_size = 268435456;
                             PRAGMA query_only = 1;",
                        )
                        .into_diagnostic()?;
                        if needs_regexp {
                            add_regexp_function(&conn)?;
                        }

                        let (sql, query_params) = Self::build_query_sql(&opts, Some((start, end)));
                        Self::execute_built_query(&conn, &sql, &query_params)
                    })
                })
                .map(|h| h.join().unwrap())
                .collect()
        });

        let mut merged = Vec::new();
        if opts.order_desc {
            for shard in shards.into_iter().rev() {
                merged.extend(shard?);
            }
        } else {
            for shard in shards {
                merged.extend(shard?);
            }
        }

        if let Some(limit) = opts.limit
            && merged.len() > limit
        {
            merged.truncate(limit);
        }

        Ok(merged)
    }

    /// Execute a built SQL query and collect results into LogEntry.
    fn execute_built_query(
        conn: &Connection,
        sql: &str,
        query_params: &[Box<dyn rusqlite::ToSql>],
    ) -> Result<Vec<LogEntry>> {
        let mut stmt = conn.prepare(sql).into_diagnostic()?;
        let params_ref: Vec<&dyn rusqlite::ToSql> =
            query_params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(params_ref.as_slice(), Self::row_to_entry)
            .into_diagnostic()?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.into_diagnostic()?);
        }
        Ok(entries)
    }
}

impl LogStore for SqliteLogStore {
    fn append(&self, daemon_id: &DaemonId, message: &str) -> Result<()> {
        let ts = Local::now().timestamp_millis();
        let id = daemon_id.qualified();
        let msg = message.to_string();

        let conn = self.conn.lock().unwrap();
        let _ = conn
            .execute(
                "INSERT INTO log_entries (daemon_id, timestamp, message) VALUES (?1, ?2, ?3)",
                params![id, ts, msg],
            )
            .into_diagnostic()?;
        Ok(())
    }

    fn append_batch(&self, daemon_id: &DaemonId, messages: &[String]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let base_ts = Local::now().timestamp_millis();
        let id = daemon_id.qualified();

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().into_diagnostic()?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO log_entries (daemon_id, timestamp, message) VALUES (?1, ?2, ?3)",
                )
                .into_diagnostic()?;
            for (idx, msg) in messages.iter().enumerate() {
                // Slightly stagger timestamps within a batch so ordering by
                // (timestamp, id) preserves insertion order without paying for
                // a separate per-row clock read.
                let ts = base_ts + idx as i64;
                stmt.execute(params![id, ts, msg]).into_diagnostic()?;
            }
        }
        tx.commit().into_diagnostic()?;
        Ok(())
    }

    fn append_structured(&self, daemon_id: &DaemonId, parsed: &ParsedLog) -> Result<()> {
        let ts = Local::now().timestamp_millis();
        let id = daemon_id.qualified();

        let conn = self.conn.lock().unwrap();
        let _ = conn
            .execute(
                "INSERT INTO log_entries (daemon_id, timestamp, message, level, msg, logger, fields_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, ts, parsed.message, parsed.level, parsed.msg, parsed.logger, parsed.fields_json],
            )
            .into_diagnostic()?;
        Ok(())
    }

    fn append_structured_batch(&self, daemon_id: &DaemonId, entries: &[ParsedLog]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let base_ts = Local::now().timestamp_millis();
        let id = daemon_id.qualified();

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().into_diagnostic()?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO log_entries (daemon_id, timestamp, message, level, msg, logger, fields_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .into_diagnostic()?;
            for (idx, entry) in entries.iter().enumerate() {
                let ts = base_ts + idx as i64;
                stmt.execute(params![
                    id,
                    ts,
                    entry.message,
                    entry.level,
                    entry.msg,
                    entry.logger,
                    entry.fields_json
                ])
                .into_diagnostic()?;
            }
        }
        tx.commit().into_diagnostic()?;
        Ok(())
    }

    fn query(&self, opts: &LogQuery) -> Result<Vec<LogEntry>> {
        // Delegate to parallel path for large single-daemon queries.
        if Self::should_parallelize(opts)
            && self.path.as_os_str() != ":memory:"
            && let Ok(entries) = self.query_parallel(opts)
        {
            return Ok(entries);
        }
        // Fall back to single-threaded on parallel failure.

        // Single-threaded path.
        let conn = self.conn.lock().unwrap();
        let (sql, query_params) = Self::build_query_sql(opts, None);
        Self::execute_built_query(&conn, &sql, &query_params)
    }

    fn tail(&self, daemon_id: &DaemonId, after_id: Option<i64>) -> Result<Vec<LogEntry>> {
        self.query(&LogQuery {
            daemon_ids: vec![daemon_id.qualified()],
            from: None,
            to: None,
            limit: None,
            order_desc: false,
            after_id,
            message_filters: Vec::new(),
            field_filters: Vec::new(),
            include_structured: false,
        })
    }

    fn clear(&self, daemon_ids: &[DaemonId]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().into_diagnostic()?;
        for id in daemon_ids {
            tx.execute(
                "DELETE FROM log_entries WHERE daemon_id = ?1",
                params![id.qualified()],
            )
            .into_diagnostic()?;
            tx.execute(
                "INSERT INTO log_clear_generations (daemon_id, generation)
                 VALUES (?1, 1)
                 ON CONFLICT(daemon_id) DO UPDATE SET generation = generation + 1",
                params![id.qualified()],
            )
            .into_diagnostic()?;
        }
        tx.commit().into_diagnostic()?;
        Ok(())
    }

    fn last_id(&self, daemon_id: &DaemonId) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        // MAX(id) returns NULL when no rows exist for the daemon; the
        // Option<i64> decode maps NULL to None automatically.
        let id: Option<i64> = conn
            .query_row(
                "SELECT MAX(id) FROM log_entries WHERE daemon_id = ?1",
                params![daemon_id.qualified()],
                |row| row.get(0),
            )
            .into_diagnostic()?;
        Ok(id)
    }

    fn list_daemon_ids(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT DISTINCT daemon_id FROM log_entries")
            .into_diagnostic()?;
        let ids = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                Ok(id)
            })
            .into_diagnostic()?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    fn apply_retention(
        &self,
        policy: &super::RetentionPolicy,
        excluded_daemon_ids: &[DaemonId],
        archive_hook: Option<&ArchiveHook>,
    ) -> Result<u64> {
        let daemon_ids = self.list_daemon_ids()?;
        let excluded: HashSet<String> = excluded_daemon_ids.iter().map(|d| d.qualified()).collect();
        let mut total = 0u64;
        for id_str in daemon_ids {
            if excluded.contains(&id_str) {
                continue;
            }
            let id = DaemonId::parse(&id_str).unwrap_or_else(|_| {
                DaemonId::try_new("global", &id_str).unwrap_or_else(|_| DaemonId::pitchfork())
            });
            if let Some(dur) = policy.age {
                total += self.rotate_by_age(&id, dur, archive_hook)?;
            }
            if let Some(n) = policy.count {
                total += self.rotate_by_count(&id, n, archive_hook)?;
            }
        }
        Ok(total)
    }

    fn apply_retention_for_daemon(
        &self,
        daemon_id: &DaemonId,
        policy: &super::RetentionPolicy,
        archive_hook: Option<&ArchiveHook>,
    ) -> Result<u64> {
        let mut total = 0u64;
        if let Some(dur) = policy.age {
            total += self.rotate_by_age(daemon_id, dur, archive_hook)?;
        }
        if let Some(n) = policy.count {
            total += self.rotate_by_count(daemon_id, n, archive_hook)?;
        }
        Ok(total)
    }

    fn last_clear_generation(&self, daemon_id: &DaemonId) -> Result<Option<u64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT generation FROM log_clear_generations WHERE daemon_id = ?1")
            .into_diagnostic()?;
        let generation: Option<i64> = stmt
            .query_row(params![daemon_id.qualified()], |row| row.get(0))
            .optional()
            .into_diagnostic()?;
        generation
            .map(|generation| {
                u64::try_from(generation)
                    .map_err(|_| miette::miette!("log clear generation cannot be negative"))
            })
            .transpose()
    }
}

/// Global singleton log store.
use once_cell::sync::Lazy;
use std::sync::Arc;

pub static LOG_STORE: Lazy<Arc<SqliteLogStore>> = Lazy::new(|| {
    let path = crate::env::PITCHFORK_LOGS_DIR.join("logs.db");
    let mut is_fallback = false;
    let store = Arc::new(SqliteLogStore::open(&path).unwrap_or_else(|e| {
        error!(
            "failed to open log store at {}: {e}. Falling back to in-memory store; logs will not persist across restarts.",
            path.display()
        );
        is_fallback = true;
        SqliteLogStore::open(":memory:").expect("in-memory SQLite should always open")
    }));

    // Auto-migrate any legacy text log files into SQLite on first access.
    // This runs once per process startup and is idempotent.
    // Skip migration when using the in-memory fallback to prevent data loss.
    if !is_fallback {
        if let Err(e) = auto_migrate_legacy_logs(&store) {
            warn!("legacy log auto-migration failed: {e}");
        }
    } else {
        warn!(
            "skipping legacy log auto-migration because log store is in-memory (no durable destination)"
        );
    }

    store
});

/// Auto-migrate legacy text log files into the SQLite log store.
///
/// Scans the logs directory for directories matching the new-format layout
/// (`namespace--name/namespace--name.log`), attempts to parse the directory
/// name as a valid safe-path daemon ID, and imports the content into SQLite.
/// This is idempotent: re-running it on already-migrated data is a no-op
/// because the legacy text files are deleted after successful import.
fn auto_migrate_legacy_logs(store: &SqliteLogStore) -> Result<()> {
    let logs_dir = &*crate::env::PITCHFORK_LOGS_DIR;
    if !logs_dir.exists() {
        return Ok(());
    }

    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return Ok(());
    };

    let mut total_migrated = 0u64;
    let mut migrated_ids = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip the supervisor's own log directory.
        let file_name = path
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().to_string());
        if file_name == "pitchfork" {
            continue;
        }

        // Only consider directories that look like new-format safe-paths
        if !file_name.contains("--") {
            continue;
        }
        let log_file = path.join(format!("{file_name}.log"));
        if !log_file.exists() {
            continue;
        }

        let daemon_id = match DaemonId::from_safe_path(&file_name) {
            Ok(id) => id,
            Err(_) => continue,
        };

        // Skip the supervisor's own daemon; its log directory may be present
        // under logs/ but should never be imported into the user-facing log store.
        if daemon_id == DaemonId::pitchfork() {
            continue;
        }

        match store.migrate_daemon_text_logs(&daemon_id) {
            Ok(0) => {}
            Ok(n) => {
                total_migrated += n;
                migrated_ids.push(daemon_id.qualified());
            }
            Err(e) => {
                warn!(
                    "failed to migrate text logs for {}: {e}",
                    daemon_id.qualified()
                );
            }
        }
    }

    if total_migrated > 0 {
        warn!(
            "auto-migrated {total_migrated} legacy log entries from {count} daemon(s): {ids}",
            count = migrated_ids.len(),
            ids = migrated_ids.join(", ")
        );
    }

    Ok(())
}
