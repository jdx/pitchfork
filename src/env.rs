use once_cell::sync::Lazy;
pub use std::env::*;
use std::path::PathBuf;

pub static PITCHFORK_BIN: Lazy<PathBuf> =
    Lazy::new(|| current_exe().unwrap().canonicalize().unwrap());
pub static CWD: Lazy<PathBuf> = Lazy::new(|| current_dir().unwrap_or_default());

pub static HOME_DIR: Lazy<PathBuf> = Lazy::new(|| dirs::home_dir().unwrap_or_default());
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
pub static IPC_JSON: Lazy<bool> = Lazy::new(|| !var_false("IPC_JSON"));

fn var_path(name: &str) -> Option<PathBuf> {
    var(name).map(PathBuf::from).ok()
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
