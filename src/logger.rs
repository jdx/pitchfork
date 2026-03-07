use crate::Result;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::{env, ui};
use log::{Level, LevelFilter, Metadata, Record};
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;

/// Atomic level storage so settings can update log levels after init.
static TERM_LEVEL: AtomicUsize = AtomicUsize::new(LevelFilter::Info as usize);
static FILE_LEVEL: AtomicUsize = AtomicUsize::new(LevelFilter::Info as usize);

fn usize_to_level_filter(n: usize) -> LevelFilter {
    match n {
        0 => LevelFilter::Off,
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        5 => LevelFilter::Trace,
        _ => LevelFilter::Info, // unreachable in practice
    }
}

fn load_term_level() -> LevelFilter {
    usize_to_level_filter(TERM_LEVEL.load(Ordering::Relaxed))
}

fn load_file_level() -> LevelFilter {
    usize_to_level_filter(FILE_LEVEL.load(Ordering::Relaxed))
}

#[derive(Debug)]
struct Logger {
    log_file: Option<Mutex<File>>,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let max_level = std::cmp::max(load_term_level(), load_file_level());
        metadata.level() <= max_level
    }

    fn log(&self, record: &Record) {
        let file_level = load_file_level();
        let term_level = load_term_level();
        if record.level() <= file_level
            && let Some(log_file) = &self.log_file
        {
            let mut log_file = match log_file.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let out = format!(
                "{now} {level} {args}",
                now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                level = self.styled_level(record.level()),
                args = record.args()
            );
            let _ = writeln!(log_file, "{}", console::strip_ansi_codes(&out));
        }
        if record.level() <= term_level {
            let out = self.render(record, term_level);
            if !out.is_empty() {
                eprintln!("{out}");
            }
        }
    }

    fn flush(&self) {}
}

static LOGGER: Lazy<Logger> = Lazy::new(Logger::init);

impl Logger {
    fn init() -> Self {
        let term_level = *env::PITCHFORK_LOG;
        let file_level = *env::PITCHFORK_LOG_FILE_LEVEL;

        // Store initial levels (from env vars) into atomics.
        TERM_LEVEL.store(term_level as usize, Ordering::Relaxed);
        FILE_LEVEL.store(file_level as usize, Ordering::Relaxed);

        let mut logger = Logger { log_file: None };

        let log_file = &*env::PITCHFORK_LOG_FILE;
        if let Ok(log_file) = init_log_file(log_file) {
            logger.log_file = Some(Mutex::new(log_file));
        } else {
            warn!("could not open log file: {log_file:?}");
        }

        logger
    }

    /// Re-apply log levels from the settings system.
    ///
    /// The logger is initialised very early (before config files are parsed),
    /// so it can only see environment variables at that point. After
    /// `Settings::load()` has merged env + config-file values, call this
    /// function to pick up any `log_level` / `log_file_level` that was set
    /// in a pitchfork.toml `[settings]` section.
    fn apply_settings_levels(&self) {
        use std::sync::atomic::Ordering;
        let s = crate::settings::settings();

        let term_level: LevelFilter = s.general.log_level.parse().unwrap_or(LevelFilter::Info);
        let file_level: LevelFilter = s.general.log_file_level.parse().unwrap_or(term_level);
        let max_level = std::cmp::max(term_level, file_level);

        // Update the cached levels inside LOGGER.
        // Safety: these are only read by the `log` trait methods which
        // tolerate momentary inconsistency (worst case: one extra or
        // one missing log line during the switch).
        //
        // We use AtomicUsize fields so we can update them after init.
        TERM_LEVEL.store(term_level as usize, Ordering::Relaxed);
        FILE_LEVEL.store(file_level as usize, Ordering::Relaxed);

        // Also update the global max level so the `log` crate's
        // fast-path filter reflects the new configuration.
        log::set_max_level(max_level);
    }

    fn render(&self, record: &Record, level: LevelFilter) -> String {
        match level {
            LevelFilter::Off => "".to_string(),
            LevelFilter::Trace => {
                let file = record.file().unwrap_or("<unknown>");
                let ignore_crates = ["/notify-debouncer-full-", "/notify-"];
                if record.level() == Level::Trace && ignore_crates.iter().any(|c| file.contains(c))
                {
                    return "".to_string();
                }
                let meta = ui::style::edim(format!(
                    "{thread_id:>2} [{file}:{line}]",
                    thread_id = thread_id(),
                    line = record.line().unwrap_or(0),
                ));
                format!(
                    "{level} {meta} {args}",
                    level = self.styled_level(record.level()),
                    args = record.args()
                )
            }
            LevelFilter::Debug => format!(
                "{level} {args}",
                level = self.styled_level(record.level()),
                args = record.args()
            ),
            _ => {
                let pitchfork = match record.level() {
                    Level::Error => ui::style::ered("pitchfork"),
                    Level::Warn => ui::style::eyellow("pitchfork"),
                    _ => ui::style::edim("pitchfork"),
                };
                match record.level() {
                    Level::Info => format!("{pitchfork} {args}", args = record.args()),
                    _ => format!(
                        "{pitchfork} {level} {args}",
                        level = self.styled_level(record.level()),
                        args = record.args()
                    ),
                }
            }
        }
    }

    fn styled_level(&self, level: Level) -> String {
        let level = match level {
            Level::Error => ui::style::ered("ERROR").to_string(),
            Level::Warn => ui::style::eyellow("WARN").to_string(),
            Level::Info => ui::style::ecyan("INFO").to_string(),
            Level::Debug => ui::style::emagenta("DEBUG").to_string(),
            Level::Trace => ui::style::edim("TRACE").to_string(),
        };
        console::pad_str(&level, 5, console::Alignment::Left, None).to_string()
    }
}

pub fn thread_id() -> String {
    let id = format!("{:?}", thread::current().id());
    let id = id.replace("ThreadId(", "");
    id.replace(")", "")
}

pub fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let max_level = std::cmp::max(load_term_level(), load_file_level());
        if let Err(err) = log::set_logger(&*LOGGER).map(|()| log::set_max_level(max_level)) {
            eprintln!("pitchfork: could not initialize logger: {err}");
        }
    });
}

/// Re-apply log levels from the loaded settings.
///
/// Call this once after `Settings::load()` has run so that log levels
/// configured in pitchfork.toml `[settings.general]` take effect.
pub fn apply_settings() {
    LOGGER.apply_settings_levels();
}

fn init_log_file(log_file: &Path) -> Result<File> {
    if let Some(log_dir) = log_file.parent() {
        xx::file::mkdirp(log_dir)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .into_diagnostic()
}
