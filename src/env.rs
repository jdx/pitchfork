use once_cell::sync::Lazy;
pub use std::env::*;
use std::path::PathBuf;

pub static PITCHFORK_BIN: Lazy<PathBuf> = Lazy::new(|| {
    current_exe()
        .and_then(|p| p.canonicalize())
        .unwrap_or_else(|e| {
            eprintln!("Warning: Could not determine pitchfork binary path: {e}");
            args()
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("pitchfork"))
        })
});
pub static CWD: Lazy<PathBuf> = Lazy::new(|| current_dir().unwrap_or_else(|_| PathBuf::from(".")));

pub static HOME_DIR: Lazy<PathBuf> = Lazy::new(|| {
    dirs::home_dir().unwrap_or_else(|| {
        eprintln!("Warning: Could not determine home directory");
        PathBuf::from("/tmp")
    })
});
pub static PITCHFORK_CONFIG_DIR: Lazy<PathBuf> = Lazy::new(|| {
    var_path("PITCHFORK_CONFIG_DIR").unwrap_or(HOME_DIR.join(".config").join("pitchfork"))
});
pub static PITCHFORK_GLOBAL_CONFIG_USER: Lazy<PathBuf> =
    Lazy::new(|| PITCHFORK_CONFIG_DIR.join("config.toml"));
pub static PITCHFORK_GLOBAL_CONFIG_SYSTEM: Lazy<PathBuf> =
    Lazy::new(|| PathBuf::from("/etc/pitchfork/config.toml"));
pub static PITCHFORK_STATE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    var_path("PITCHFORK_STATE_DIR").unwrap_or(
        dirs::state_dir()
            .unwrap_or(HOME_DIR.join(".local").join("state"))
            .join("pitchfork"),
    )
});
pub static PITCHFORK_STATE_FILE: Lazy<PathBuf> =
    Lazy::new(|| PITCHFORK_STATE_DIR.join("state.toml"));
pub static PITCHFORK_LOG: Lazy<log::LevelFilter> =
    Lazy::new(|| var_log_level("PITCHFORK_LOG").unwrap_or(log::LevelFilter::Info));
pub static PITCHFORK_LOG_FILE_LEVEL: Lazy<log::LevelFilter> =
    Lazy::new(|| var_log_level("PITCHFORK_LOG_FILE_LEVEL").unwrap_or(*PITCHFORK_LOG));
pub static PITCHFORK_LOGS_DIR: Lazy<PathBuf> =
    Lazy::new(|| var_path("PITCHFORK_LOGS_DIR").unwrap_or(PITCHFORK_STATE_DIR.join("logs")));
pub static PITCHFORK_LOG_FILE: Lazy<PathBuf> =
    Lazy::new(|| PITCHFORK_LOGS_DIR.join("pitchfork").join("pitchfork.log"));
// pub static PITCHFORK_EXEC: Lazy<bool> = Lazy::new(|| var_true("PITCHFORK_EXEC"));

pub static IPC_SOCK_DIR: Lazy<PathBuf> = Lazy::new(|| PITCHFORK_STATE_DIR.join("sock"));
pub static IPC_SOCK_MAIN: Lazy<PathBuf> = Lazy::new(|| IPC_SOCK_DIR.join("main.sock"));

// Capture the PATH at startup so daemons can find user tools
pub static ORIGINAL_PATH: Lazy<Option<String>> = Lazy::new(|| var("PATH").ok());
pub static IPC_JSON: Lazy<bool> = Lazy::new(|| !var_false("IPC_JSON"));

// Delay in seconds before autostopping daemons when leaving a directory
// Set to 0 to disable the delay (stop immediately)
pub static PITCHFORK_AUTOSTOP_DELAY: Lazy<u64> =
    Lazy::new(|| var_u64("PITCHFORK_AUTOSTOP_DELAY").unwrap_or(60));

// Interval in seconds for the supervisor's background watcher
// Default: 10 seconds. Lower values useful for testing.
pub static PITCHFORK_INTERVAL_SECS: Lazy<u64> =
    Lazy::new(|| var_u64("PITCHFORK_INTERVAL_SECS").unwrap_or(10));

fn var_path(name: &str) -> Option<PathBuf> {
    var(name).map(PathBuf::from).ok()
}

fn var_u64(name: &str) -> Option<u64> {
    var(name).ok().and_then(|val| val.parse().ok())
}

fn var_log_level(name: &str) -> Option<log::LevelFilter> {
    var(name).ok().and_then(|level| level.parse().ok())
}

fn var_false(name: &str) -> bool {
    var(name)
        .map(|val| val.to_lowercase())
        .map(|val| val == "false" || val == "0")
        .unwrap_or(false)
}

// fn var_true(name: &str) -> bool {
//     var(name)
//         .map(|val| val.to_lowercase())
//         .map(|val| val == "true" || val == "1")
//         .unwrap_or(false)
// }
