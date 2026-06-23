use crate::cli::json_output::{JsonLogEntry, print_json};
use crate::daemon_id::DaemonId;
use crate::log_store::sqlite::LOG_STORE;
use crate::log_store::{LogQuery, LogStore, MessageFilter};
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::state_file::StateFile;
use crate::ui::style::{edim, estyle, ndim};
use crate::{Result, env};
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeZone};
use console;
use itertools::Itertools;
use miette::IntoDiagnostic;
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Pager configuration for displaying logs
struct PagerConfig {
    command: String,
    args: Vec<String>,
}

impl PagerConfig {
    /// Select and configure the appropriate pager.
    /// Uses $PAGER environment variable if set, otherwise defaults to less.
    fn new(start_at_end: bool) -> Self {
        let command = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        let args = Self::build_args(&command, start_at_end);
        Self { command, args }
    }

    fn build_args(pager: &str, start_at_end: bool) -> Vec<String> {
        let mut args = vec![];
        if pager == "less" {
            args.push("-R".to_string());
            if start_at_end {
                args.push("+G".to_string());
            }
        }
        args
    }

    /// Spawn the pager with piped stdin
    fn spawn_piped(&self) -> io::Result<Child> {
        Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .spawn()
    }
}

/// Format a single log line for output.
/// When `single_daemon` is true, omits the daemon ID from the output.
/// `id_width` is the display width used to pad the daemon name column
/// so messages line up vertically across different daemon names.
/// When `strip_ansi` is true, strips ANSI escape codes from the message.
fn format_log_line(
    date: &str,
    id: &str,
    msg: &str,
    single_daemon: bool,
    id_width: usize,
    strip_ansi: bool,
    show_timestamp: bool,
) -> String {
    let msg = if strip_ansi {
        console::strip_ansi_codes(msg).to_string()
    } else {
        msg.to_string()
    };
    if single_daemon {
        if show_timestamp {
            format!("{} {}", ndim(date), msg)
        } else {
            msg
        }
    } else {
        let colors_on = !strip_ansi && console::colors_enabled();
        let colored = dimmed_id(id, colors_on);
        let padded = console::pad_str(&colored, id_width, console::Alignment::Left, None);
        if show_timestamp {
            format!("{}  {} {}", padded, ndim(date), msg)
        } else {
            format!("{}  {}", padded, msg)
        }
    }
}

/// Return a dimmed, colorized daemon ID string for display.
/// Each daemon gets a deterministic color via FNV-1a hash so that
/// multiple daemons are visually distinguishable while remaining subtle.
fn dimmed_id(id: &str, colors_enabled: bool) -> String {
    if !colors_enabled {
        return id.to_string();
    }
    let colors = [
        (180, 120, 120), // dim red
        (180, 160, 100), // dim yellow
        (120, 180, 120), // dim green
        (120, 180, 180), // dim cyan
        (180, 120, 180), // dim magenta
        (120, 160, 180), // dim blue
    ];
    let mut h: usize = 0x811C_9DC5; // FNV offset basis
    for b in id.bytes() {
        h = h.wrapping_mul(0x0100_0193).wrapping_add(b as usize);
    }
    let (r, g, b) = colors[h % colors.len()];
    format!("\x1b[2;38;2;{};{};{}m{}\x1b[0m", r, g, b, id)
}

/// Return a colorized `[namespace/id]` label for display in progress jobs.
/// Uses brighter colors than `dimmed_id` and includes the square brackets.
pub fn colored_id_label(id: &str, colors_enabled: bool) -> String {
    if !colors_enabled {
        return format!("[{}]", id);
    }
    // Same palette as mise: Blue, Magenta, Cyan, Green
    // Excludes Red/Yellow to avoid confusion with errors/warnings.
    let colors: [u8; 4] = [34, 35, 36, 32]; // ANSI: Blue, Magenta, Cyan, Green
    let mut h: usize = 0x811C_9DC5; // FNV offset basis
    for b in id.bytes() {
        h = h.wrapping_mul(0x0100_0193).wrapping_add(b as usize);
    }
    let color = colors[h % colors.len()];
    format!("\x1b[{color}m[{id}]\x1b[0m")
}

/// Displays logs for daemon(s)
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "l",
    verbatim_doc_comment,
    long_about = "\
Displays logs for daemon(s)

Shows logs from managed daemons. Logs are stored in the pitchfork logs directory
and include timestamps for filtering.

Examples:
  pitchfork logs api              Show all logs for 'api' (paged if needed)
  pitchfork logs api worker       Show logs for multiple daemons
  pitchfork logs                  Show logs for all daemons
  pitchfork logs api -n 50        Show last 50 lines
  pitchfork logs api --follow     Follow logs in real-time
  pitchfork logs api --since '2024-01-15 10:00:00'
                                  Show logs since a specific time (forward)
  pitchfork logs api --since '10:30:00'
                                  Show logs since 10:30:00 today
  pitchfork logs api --since '10:30' --until '12:00'
                                  Show logs since 10:30:00 until 12:00:00 today
  pitchfork logs api --since 5min Show logs from last 5 minutes
  pitchfork logs api --raw        Output raw log lines without formatting
  pitchfork logs api --raw -n 100 Output last 100 raw log lines
  pitchfork logs api --clear      Delete logs for 'api'
  pitchfork logs --clear          Delete logs for all daemons"
)]
pub struct Logs {
    /// Show only logs for the specified daemon(s)
    id: Vec<String>,

    /// Delete logs
    #[clap(short, long)]
    clear: bool,

    /// Show last N lines of logs
    ///
    /// Only applies when --since/--until is not used.
    /// Without this option, all logs are shown.
    #[clap(short)]
    n: Option<usize>,

    /// Show logs in real-time
    #[clap(short = 't', short_alias = 'f', long, visible_alias = "follow")]
    tail: bool,

    /// Show logs from this time
    ///
    /// Supports multiple formats:
    /// - Full datetime: "YYYY-MM-DD HH:MM:SS" or "YYYY-MM-DD HH:MM"
    /// - Time only: "HH:MM:SS" or "HH:MM" (uses today's date)
    /// - Relative time: "5min", "2h", "1d" (e.g., last 5 minutes)
    #[clap(short = 's', long)]
    since: Option<String>,

    /// Show logs until this time
    ///
    /// Supports multiple formats:
    /// - Full datetime: "YYYY-MM-DD HH:MM:SS" or "YYYY-MM-DD HH:MM"
    /// - Time only: "HH:MM:SS" or "HH:MM" (uses today's date)
    #[clap(short = 'u', long)]
    until: Option<String>,

    /// Disable pager even in interactive terminal
    #[clap(long)]
    no_pager: bool,

    /// Output raw log lines without color or formatting
    #[clap(long)]
    raw: bool,

    /// Output in JSON format
    #[clap(long, conflicts_with = "raw", conflicts_with = "tail")]
    json: bool,

    /// Filter logs by case-insensitive substring (can be repeated)
    ///
    /// Multiple --grep options are combined with OR.
    #[clap(long)]
    grep: Vec<String>,

    /// Filter logs by regular expression
    #[clap(long)]
    regex: Option<String>,

    /// Make --grep matching case-sensitive
    #[clap(long)]
    case_sensitive: bool,

    /// Omit timestamps from log output
    #[clap(long)]
    no_timestamp: bool,
}

impl Logs {
    pub async fn run(&self) -> Result<()> {
        migrate_legacy_log_dirs();

        let resolved_ids: Vec<DaemonId> = if self.id.is_empty() {
            get_all_daemon_ids()?
        } else {
            PitchforkToml::resolve_ids(&self.id)?
        };

        if self.clear {
            LOG_STORE.clear(&resolved_ids)?;
            return Ok(());
        }

        let from = if let Some(since) = self.since.as_ref() {
            Some(parse_time_input(since, true)?)
        } else {
            None
        };
        let to = if let Some(until) = self.until.as_ref() {
            Some(parse_time_input(until, false)?)
        } else {
            None
        };

        let message_filters = self.build_message_filters()?;

        if self.json {
            return self.output_json(&resolved_ids, from, to, message_filters);
        }

        let single_daemon = resolved_ids.len() == 1;
        let show_timestamp = settings().logs.timestamp && !self.no_timestamp;
        let log_lines = self.fetch_log_lines(&resolved_ids, from, to, message_filters.clone())?;
        let has_time_filter = from.is_some() || to.is_some();
        self.output_logs(
            log_lines,
            single_daemon,
            has_time_filter,
            self.tail,
            show_timestamp,
        )?;
        if self.tail {
            tail_logs(
                &resolved_ids,
                single_daemon,
                true,
                message_filters,
                show_timestamp,
            )
            .await?;
        }

        Ok(())
    }

    fn build_message_filters(&self) -> Result<Vec<MessageFilter>> {
        if self.case_sensitive && self.grep.is_empty() {
            warn!("--case-sensitive has no effect without --grep");
        }
        let mut filters = Vec::new();
        for pattern in &self.grep {
            filters.push(MessageFilter::Contains {
                pattern: pattern.clone(),
                case_sensitive: self.case_sensitive,
            });
        }
        if let Some(pattern) = self.regex.as_ref() {
            // Validate the regex early so the user gets a clear CLI error
            // instead of a SQLite user-function failure at query time.
            let _ = regex::Regex::new(pattern)
                .into_diagnostic()
                .map_err(|e| miette::miette!("invalid regex pattern: {e}"))?;
            filters.push(MessageFilter::Regex {
                pattern: pattern.clone(),
            });
        }
        Ok(filters)
    }

    fn fetch_log_lines(
        &self,
        resolved_ids: &[DaemonId],
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
        message_filters: Vec<MessageFilter>,
    ) -> Result<Vec<(String, String, String)>> {
        let daemon_ids: Vec<String> = resolved_ids.iter().map(|id| id.qualified()).collect();
        let has_time_filter = from.is_some() || to.is_some();

        let opts = LogQuery {
            daemon_ids: daemon_ids.clone(),
            from,
            to,
            limit: if !has_time_filter { self.n } else { None },
            order_desc: !has_time_filter,
            after_id: None,
            message_filters,
        };
        let entries = LOG_STORE.query(&opts)?;
        let log_lines: Vec<(String, String, String)> = entries
            .into_iter()
            .map(|e| {
                let ts = e.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                (ts, e.daemon_id, e.message)
            })
            .collect();

        let log_lines = if has_time_filter {
            if let Some(n) = self.n {
                let len = log_lines.len();
                if len > n {
                    log_lines.into_iter().skip(len - n).collect_vec()
                } else {
                    log_lines
                }
            } else {
                log_lines
            }
        } else if let Some(n) = self.n {
            let len = log_lines.len();
            if len > n {
                log_lines.into_iter().skip(len - n).rev().collect_vec()
            } else {
                log_lines.into_iter().rev().collect_vec()
            }
        } else {
            log_lines.into_iter().rev().collect_vec()
        };

        Ok(log_lines)
    }

    fn output_json(
        &self,
        resolved_ids: &[DaemonId],
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
        message_filters: Vec<MessageFilter>,
    ) -> Result<()> {
        let log_lines = self.fetch_log_lines(resolved_ids, from, to, message_filters)?;

        let json_entries: Vec<JsonLogEntry> = log_lines
            .into_iter()
            .map(|(timestamp, daemon_id, message)| JsonLogEntry {
                timestamp,
                daemon_id,
                message: console::strip_ansi_codes(&message).to_string(),
            })
            .collect();

        print_json(&json_entries)
    }

    fn output_logs(
        &self,
        log_lines: Vec<(String, String, String)>,
        single_daemon: bool,
        has_time_filter: bool,
        force_no_pager: bool,
        show_timestamp: bool,
    ) -> Result<()> {
        if log_lines.is_empty() {
            return Ok(());
        }

        let id_width = log_lines
            .iter()
            .map(|(_, id, _)| id.len())
            .max()
            .unwrap_or(0);
        let strip_ansi = self.raw || !console::colors_enabled();

        if self.raw {
            for (date, id, msg) in log_lines {
                let line = format_log_line(
                    &date,
                    &id,
                    &msg,
                    single_daemon,
                    id_width,
                    strip_ansi,
                    show_timestamp,
                );
                println!("{line}");
            }
            return Ok(());
        }

        let use_pager = !force_no_pager && !self.no_pager && should_use_pager(log_lines.len());

        if use_pager {
            self.output_with_pager(
                log_lines,
                single_daemon,
                id_width,
                has_time_filter,
                strip_ansi,
                show_timestamp,
            )?;
        } else {
            for (date, id, msg) in log_lines {
                println!(
                    "{}",
                    format_log_line(
                        &date,
                        &id,
                        &msg,
                        single_daemon,
                        id_width,
                        strip_ansi,
                        show_timestamp,
                    )
                );
            }
        }

        Ok(())
    }

    fn output_with_pager(
        &self,
        log_lines: Vec<(String, String, String)>,
        single_daemon: bool,
        id_width: usize,
        has_time_filter: bool,
        strip_ansi: bool,
        show_timestamp: bool,
    ) -> Result<()> {
        // When time filter is used, start at top; otherwise start at end
        let pager_config = PagerConfig::new(!has_time_filter);

        match pager_config.spawn_piped() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    for (date, id, msg) in log_lines {
                        let line = format!(
                            "{}\n",
                            format_log_line(
                                &date,
                                &id,
                                &msg,
                                single_daemon,
                                id_width,
                                strip_ansi,
                                show_timestamp,
                            )
                        );
                        if stdin.write_all(line.as_bytes()).is_err() {
                            break;
                        }
                    }
                    let _ = child.wait();
                } else {
                    debug!("Failed to get pager stdin, falling back to direct output");
                    for (date, id, msg) in log_lines {
                        println!(
                            "{}",
                            format_log_line(
                                &date,
                                &id,
                                &msg,
                                single_daemon,
                                id_width,
                                strip_ansi,
                                show_timestamp,
                            )
                        );
                    }
                }
            }
            Err(e) => {
                debug!("Failed to spawn pager: {e}, falling back to direct output");
                for (date, id, msg) in log_lines {
                    println!(
                        "{}",
                        format_log_line(
                            &date,
                            &id,
                            &msg,
                            single_daemon,
                            id_width,
                            strip_ansi,
                            show_timestamp,
                        )
                    );
                }
            }
        }

        Ok(())
    }
}

fn should_use_pager(line_count: usize) -> bool {
    if !io::stdout().is_terminal() {
        return false;
    }

    let terminal_height = get_terminal_height().unwrap_or(24);
    line_count > terminal_height
}

fn get_terminal_height() -> Option<usize> {
    if let Ok(rows) = std::env::var("LINES")
        && let Ok(h) = rows.parse::<usize>()
    {
        return Some(h);
    }

    crossterm::terminal::size().ok().map(|(_, h)| h as usize)
}

/// Rename legacy log directories that predate namespace-qualified daemon IDs.
///
/// Old layout: `PITCHFORK_LOGS_DIR/<name>/<name>.log`
/// New layout: `PITCHFORK_LOGS_DIR/legacy--<name>/legacy--<name>.log`
///
/// Only directories that clearly match the old layout are migrated:
/// - directory name does not contain `"--"`
/// - directory contains `<name>.log`
/// - `<name>` is a valid daemon short name under current DaemonId rules
fn migrate_legacy_log_dirs() {
    let known_safe_paths = known_daemon_safe_paths();
    let dirs = match xx::file::ls(&*env::PITCHFORK_LOGS_DIR) {
        Ok(d) => d,
        Err(_) => return,
    };
    for dir in dirs {
        if dir.starts_with(".") || !dir.is_dir() {
            continue;
        }
        let name = match dir.file_name().map(|f| f.to_string_lossy().to_string()) {
            Some(n) => n,
            None => continue,
        };
        // Skip the supervisor's own log directory.
        if name == "pitchfork" {
            continue;
        }
        // New-format directories usually contain "--". For safety, only treat
        // them as new-format if they match a known daemon ID safe-path.
        if name.contains("--") {
            // If it parses as a valid safe-path, treat it as already migrated
            // and keep idempotent behavior silent.
            if DaemonId::from_safe_path(&name).is_ok() {
                continue;
            }
            // Keep noisy warnings only for invalid/ambiguous names that cannot
            // be interpreted as new-format IDs.
            if known_safe_paths.contains(&name) {
                continue;
            }
            warn!(
                "Skipping invalid legacy log directory '{name}': contains '--' but is not a valid daemon safe-path"
            );
            continue;
        }

        // Migrate only explicit old-layout directories to avoid renaming
        // unrelated folders under logs/.
        let old_log = dir.join(format!("{name}.log"));
        if !old_log.exists() {
            continue;
        }
        if DaemonId::try_new("legacy", &name).is_err() {
            warn!("Skipping invalid legacy log directory '{name}': not a valid daemon ID");
            continue;
        }

        let new_name = format!("legacy--{name}");
        let new_dir = env::PITCHFORK_LOGS_DIR.join(&new_name);
        // Skip if a target directory already exists to avoid clobbering data.
        if new_dir.exists() {
            continue;
        }
        if std::fs::rename(&dir, &new_dir).is_err() {
            continue;
        }
        // Also rename the log file inside the directory.
        let old_log = new_dir.join(format!("{name}.log"));
        let new_log = new_dir.join(format!("{new_name}.log"));
        if old_log.exists() {
            let _ = std::fs::rename(&old_log, &new_log);
        }
        debug!("Migrated legacy log dir '{name}' → '{new_name}'");
    }
}

fn known_daemon_safe_paths() -> BTreeSet<String> {
    let mut out = BTreeSet::new();

    match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
        Ok(state) => {
            for id in state.daemons.keys() {
                out.insert(id.safe_path());
            }
        }
        Err(e) => {
            warn!("Failed to read state while checking known daemon IDs: {e}");
        }
    }

    match PitchforkToml::all_merged() {
        Ok(config) => {
            for id in config.daemons.keys() {
                out.insert(id.safe_path());
            }
        }
        Err(e) => {
            warn!("Failed to read config while checking known daemon IDs: {e}");
        }
    }

    out
}

fn get_all_daemon_ids() -> Result<Vec<DaemonId>> {
    let mut ids = BTreeSet::new();

    match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
        Ok(state) => ids.extend(state.daemons.keys().cloned()),
        Err(e) => warn!("Failed to read state for log daemon discovery: {e}"),
    }

    match PitchforkToml::all_merged() {
        Ok(config) => ids.extend(config.daemons.keys().cloned()),
        Err(e) => warn!("Failed to read config for log daemon discovery: {e}"),
    }

    let logged_ids: std::collections::HashSet<String> =
        LOG_STORE.list_daemon_ids()?.into_iter().collect();
    Ok(ids
        .into_iter()
        .filter(|id| logged_ids.contains(&id.qualified()))
        .collect())
}

pub async fn tail_logs(
    names: &[DaemonId],
    single_daemon: bool,
    start_from_end: bool,
    message_filters: Vec<MessageFilter>,
    show_timestamp: bool,
) -> Result<()> {
    // Poll SQLite log store for new entries since last known row id.
    let id_width = names
        .iter()
        .map(|id| id.qualified().len())
        .max()
        .unwrap_or(0);

    let strip_ansi = !console::colors_enabled();

    let mut states: std::collections::HashMap<String, i64> = names
        .iter()
        .map(|id| {
            let since = if start_from_end {
                // Anchor to the last entry overall, not the last filtered entry,
                // so --tail combined with a filter does not rescan from row 1
                // on every poll when no message matches yet.
                match LOG_STORE.query(&LogQuery {
                    daemon_ids: vec![id.qualified()],
                    from: None,
                    to: None,
                    limit: Some(1),
                    order_desc: true,
                    after_id: None,
                    message_filters: Vec::new(),
                }) {
                    Ok(entries) => entries.first().map(|e| e.id).unwrap_or(0),
                    Err(_) => 0,
                }
            } else {
                0
            };
            (id.qualified(), since)
        })
        .collect();

    let interval = tokio::time::interval(Duration::from_millis(200));
    tokio::pin!(interval);

    loop {
        interval.tick().await;

        let mut out = vec![];
        for id in names {
            let after_id = states.get(&id.qualified()).copied();
            match LOG_STORE.query(&LogQuery {
                daemon_ids: vec![id.qualified()],
                from: None,
                to: None,
                limit: None,
                order_desc: false,
                after_id,
                message_filters: message_filters.clone(),
            }) {
                Ok(entries) => {
                    for entry in &entries {
                        let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                        out.push((ts, entry.daemon_id.clone(), entry.message.clone()));
                    }
                    if let Some(last) = entries.last() {
                        states.insert(id.qualified(), last.id);
                    }
                }
                Err(e) => {
                    error!("Failed to tail logs for {}: {e}", id.qualified());
                }
            }
        }

        if !out.is_empty() {
            let out = out
                .into_iter()
                .sorted_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)))
                .collect_vec();
            for (date, name, msg) in out {
                println!(
                    "{}",
                    format_log_line(
                        &date,
                        &name,
                        &msg,
                        single_daemon,
                        id_width,
                        strip_ansi,
                        show_timestamp,
                    )
                );
            }
        }
    }
}

fn parse_datetime(s: &str) -> Result<DateTime<Local>> {
    let naive_dt = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").into_diagnostic()?;
    Local
        .from_local_datetime(&naive_dt)
        .single()
        .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'. ", s))
}

/// Parse time input string into DateTime.
///
/// `is_since` indicates whether this is for --since (true) or --until (false).
/// The "yesterday fallback" only applies to --since: if the time is in the future,
/// assume the user meant yesterday. For --until, future times are kept as-is.
fn parse_time_input(s: &str, is_since: bool) -> Result<DateTime<Local>> {
    let s = s.trim();

    // Try full datetime first (YYYY-MM-DD HH:MM:SS)
    if let Ok(dt) = parse_datetime(s) {
        return Ok(dt);
    }

    // Try datetime without seconds (YYYY-MM-DD HH:MM)
    if let Ok(naive_dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Local
            .from_local_datetime(&naive_dt)
            .single()
            .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s));
    }

    // Try time-only format (HH:MM:SS or HH:MM)
    // Note: This branch won't be reached for inputs like "10:30" that could match
    // parse_datetime, because parse_datetime expects a full date prefix and will fail.
    if let Ok(time) = parse_time_only(s) {
        let now = Local::now();
        let today = now.date_naive();
        let mut naive_dt = NaiveDateTime::new(today, time);
        let mut dt = Local
            .from_local_datetime(&naive_dt)
            .single()
            .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s))?;

        // If the interpreted time for today is in the future, assume the user meant yesterday
        // BUT only for --since. For --until, a future time today is valid.
        if is_since
            && dt > now
            && let Some(yesterday) = today.pred_opt()
        {
            naive_dt = NaiveDateTime::new(yesterday, time);
            dt = Local
                .from_local_datetime(&naive_dt)
                .single()
                .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s))?;
        }
        return Ok(dt);
    }

    if let Ok(duration) = humantime::parse_duration(s) {
        let now = Local::now();
        let target = now - chrono::Duration::from_std(duration).into_diagnostic()?;
        return Ok(target);
    }

    Err(miette::miette!(
        "Invalid time format: '{}'. Expected formats:\n\
         - Full datetime: \"YYYY-MM-DD HH:MM:SS\" or \"YYYY-MM-DD HH:MM\"\n\
         - Time only: \"HH:MM:SS\" or \"HH:MM\" (uses today's date)\n\
         - Relative time: \"5min\", \"2h\", \"1d\" (e.g., last 5 minutes)",
        s
    ))
}

fn parse_time_only(s: &str) -> Result<NaiveTime> {
    if let Ok(time) = NaiveTime::parse_from_str(s, "%H:%M:%S") {
        return Ok(time);
    }

    if let Ok(time) = NaiveTime::parse_from_str(s, "%H:%M") {
        return Ok(time);
    }

    Err(miette::miette!("Invalid time format: '{}'", s))
}

/// Prints error log lines in a styled block matching the startup logs format.
///
/// Format:
/// ```text
///  ERROR LOGS
///  12:00:00 error message
/// ```
///
/// Timestamps use dimmed red. The tag uses white text on red background.
pub fn print_error_logs_block(log_lines: &[(String, String, String)]) {
    if log_lines.is_empty() {
        return;
    }

    let is_tty = std::io::stderr().is_terminal();
    let format_msg = |msg: &str| -> String {
        let stripped = strip_pty_controls(msg);
        if is_tty {
            stripped
        } else {
            console::strip_ansi_codes(&stripped).to_string()
        }
    };

    let tag = estyle(" ERROR LOGS ").white().on_red();
    eprintln!("\n{tag}");

    // Determine if we need to show daemon IDs (same logic as startup logs)
    let unique_ids: BTreeSet<&str> = log_lines.iter().map(|(_, id, _)| id.as_str()).collect();
    let show_id = unique_ids.len() > 1;

    if show_id {
        let id_width = log_lines
            .iter()
            .map(|(_, id, _)| console::measure_text_width(id))
            .max()
            .unwrap_or(0);
        for (date, id, msg) in log_lines {
            let time = date.split(' ').nth(1).unwrap_or(date);
            let colored = dimmed_id(id, is_tty && console::colors_enabled_stderr());
            let padded = console::pad_str(&colored, id_width, console::Alignment::Left, None);
            eprintln!(
                "{}  {} {}",
                padded,
                estyle(time).red().dim(),
                format_msg(msg)
            );
        }
    } else {
        for (date, _, msg) in log_lines {
            let time = date.split(' ').nth(1).unwrap_or(date);
            eprintln!("{} {}", estyle(time).red().dim(), format_msg(msg));
        }
    }
}

/// Describes the type of ready check being performed for display purposes.
pub enum ReadyCheckType {
    Output(String),
    Http(String),
    Port(u16),
    Cmd(String),
    Delay(u64),
    Default,
}

impl std::fmt::Display for ReadyCheckType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadyCheckType::Output(pattern) => write!(f, "output matching '{pattern}'"),
            ReadyCheckType::Http(url) => write!(f, "HTTP {url}"),
            ReadyCheckType::Port(port) => write!(f, "TCP port {port}"),
            ReadyCheckType::Cmd(cmd) => write!(f, "command '{cmd}'"),
            ReadyCheckType::Delay(secs) => write!(f, "delay ({secs}s)"),
            ReadyCheckType::Default => write!(f, "default readiness check"),
        }
    }
}

/// Creates a progress job showing a spinner while waiting for a ready check.
///
/// Returns a `Arc<ProgressJob>` that the caller should update:
/// - Set body to success message and status to `Done` when the daemon is ready
/// - Set body to failure message and status to `Failed` when the daemon fails
pub fn create_ready_check_job(
    daemon_id: &DaemonId,
    check_type: &ReadyCheckType,
) -> std::sync::Arc<clx::progress::ProgressJob> {
    use clx::progress::{ProgressJobBuilder, ProgressJobDoneBehavior, ProgressStatus};

    let is_tty = std::io::stderr().is_terminal();
    let colors_enabled = is_tty && console::colors_enabled_stderr();
    let id_label = colored_id_label(&daemon_id.qualified(), colors_enabled);
    let show_ts = crate::settings::settings().general.startup_log_timestamps;

    // When timestamps are off, {{spinner()}} renders as an animated spinner
    // (1 char wide) matching the "•" prefix used by println.  When on,
    // we show a dim timestamp instead.
    let prefix = if show_ts {
        // The timestamp updates each refresh via the now() tera function.
        // We use a fixed-width format (HH:MM:SS = 8 chars) for alignment.
        edim(chrono::Local::now().format("%H:%M:%S").to_string()).to_string()
    } else {
        "{{spinner()}}".to_string()
    };

    ProgressJobBuilder::new()
        .body(format!(
            "{} {} waiting for {{{{ check_type }}}}...",
            prefix, id_label
        ))
        .prop("check_type", &check_type.to_string())
        .status(ProgressStatus::Running)
        .on_done(ProgressJobDoneBehavior::Keep)
        .start()
}

/// Collects startup log lines for a single daemon (does not print).
///
/// Returns a list of `(time, daemon_id_qualified, message)` tuples for log
/// entries written after `from`.
pub fn collect_startup_logs(
    daemon_id: &DaemonId,
    from: DateTime<Local>,
) -> Result<Vec<(String, String, String)>> {
    let entries = LOG_STORE.query(&LogQuery {
        daemon_ids: vec![daemon_id.qualified()],
        from: Some(from),
        to: None,
        limit: None,
        order_desc: false,
        after_id: None,
        message_filters: Vec::new(),
    })?;
    let log_lines = entries
        .into_iter()
        .map(|e| {
            let ts = e.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
            (ts, e.daemon_id, e.message)
        })
        .collect();

    Ok(log_lines)
}

/// Stream startup logs for a daemon to a progress job in real-time.
///
/// Spawns a background tokio task that polls the daemon's log store
/// and calls `job.println()` for each new line. Returns a watch sender
/// that stops the streaming when sent `true`.
pub fn stream_startup_logs(
    daemon_id: &DaemonId,
    from: DateTime<Local>,
    job: std::sync::Arc<clx::progress::ProgressJob>,
) -> (
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    let id = daemon_id.clone();

    let show_ts = crate::settings::settings().general.startup_log_timestamps;

    let handle = tokio::spawn(async move {
        let is_tty = std::io::stderr().is_terminal();
        let colors_enabled = is_tty && console::colors_enabled_stderr();
        let id_label = colored_id_label(&id.qualified(), colors_enabled);
        let prefix = if show_ts {
            String::new()
        } else {
            edim("•").to_string()
        };

        let mut last_id: i64 = 0;

        // Initial fetch: all logs since daemon start time
        let initial_entries = LOG_STORE.query(&LogQuery {
            daemon_ids: vec![id.qualified()],
            from: Some(from),
            to: None,
            limit: None,
            order_desc: false,
            after_id: None,
            message_filters: Vec::new(),
        });

        if let Ok(entries) = initial_entries {
            for entry in &entries {
                let time = entry.timestamp.format("%H:%M:%S").to_string();
                let msg = strip_pty_controls(&entry.message);
                let msg = if is_tty {
                    msg
                } else {
                    console::strip_ansi_codes(&msg).to_string()
                };
                let line_prefix = if show_ts {
                    edim(time).to_string()
                } else {
                    prefix.clone()
                };
                job.println(&format!("{} {} {}", line_prefix, id_label, msg));
            }
            if let Some(last) = entries.last() {
                last_id = last.id;
            }
        }

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    if let Ok(entries) = LOG_STORE.tail(&id, Some(last_id)) {
                        for entry in &entries {
                            let time = entry.timestamp.format("%H:%M:%S").to_string();
                            let msg = strip_pty_controls(&entry.message);
                            let msg = if is_tty {
                                msg
                            } else {
                                console::strip_ansi_codes(&msg).to_string()
                            };
                            let line_prefix = if show_ts {
                                edim(time).to_string()
                            } else {
                                prefix.clone()
                            };
                            job.println(&format!("{} {} {}", line_prefix, id_label, msg));
                        }
                        if let Some(last) = entries.last() {
                            last_id = last.id;
                        }
                    }
                }
                _ = rx.changed() => {
                    break;
                }
            }
        }

        // Final drain
        if let Ok(entries) = LOG_STORE.tail(&id, Some(last_id)) {
            for entry in &entries {
                let time = entry.timestamp.format("%H:%M:%S").to_string();
                let msg = strip_pty_controls(&entry.message);
                let msg = if is_tty {
                    msg
                } else {
                    console::strip_ansi_codes(&msg).to_string()
                };
                let line_prefix = if show_ts {
                    edim(time).to_string()
                } else {
                    prefix.clone()
                };
                job.println(&format!("{} {} {}", line_prefix, id_label, msg));
            }
        }
    });

    (tx, handle)
}

/// Strips PTY control sequences from a string while preserving SGR (color/style) codes.
///
/// Removes CSI sequences that control cursor movement, screen clearing, erasing, etc.,
/// but keeps `\x1b[...m` (SGR) sequences so colors are retained.
fn strip_pty_controls(s: &str) -> String {
    struct Stripper {
        result: String,
    }

    impl vte::Perform for Stripper {
        fn print(&mut self, c: char) {
            self.result.push(c);
        }

        fn execute(&mut self, byte: u8) {
            // Keep \n and \t; drop other control characters (BEL, BS, CR, etc.)
            if byte == b'\n' || byte == b'\t' {
                self.result.push(byte as char);
            }
        }

        fn csi_dispatch(
            &mut self,
            params: &vte::Params,
            _intermediates: &[u8],
            _ignore: bool,
            action: char,
        ) {
            // Keep SGR sequences (final byte 'm')
            if action == 'm' {
                self.result.push_str("\x1b[");
                let mut first = true;
                for sub in params.iter() {
                    if !first {
                        self.result.push(';');
                    }
                    first = false;
                    for (i, &p) in sub.iter().enumerate() {
                        if i > 0 {
                            self.result.push(':');
                        }
                        self.result.push_str(&p.to_string());
                    }
                }
                self.result.push('m');
            }
            // All other CSI sequences (cursor move, clear, erase, etc.) are dropped
        }

        fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
            // Drop OSC sequences (e.g. window title)
        }

        fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {
            // Drop ESC sequences (e.g. ESC c = reset terminal)
        }

        fn hook(
            &mut self,
            _params: &vte::Params,
            _intermediates: &[u8],
            _ignore: bool,
            _action: char,
        ) {
            // Drop DCS hooks
        }

        fn put(&mut self, _byte: u8) {
            // Drop DCS data
        }

        fn unhook(&mut self) {
            // Drop DCS unhook
        }
    }

    let mut parser = vte::Parser::new();
    let mut stripper = Stripper {
        result: String::with_capacity(s.len()),
    };
    parser.advance(&mut stripper, s.as_bytes());
    stripper.result
}
