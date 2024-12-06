pub use std::env;
use once_cell::sync::Lazy;
use std::path::PathBuf;
use log::trace;

pub static HOME_DIR: Lazy<PathBuf> = Lazy::new(|| dirs::home_dir().unwrap_or(PathBuf::new()));
pub static PITCHFORK_STATE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    dirs::state_dir()
        .unwrap_or(HOME_DIR.join(".local").join("state"))
        .join("pitchfork")
});
pub static PITCHFORK_PID_FILE: Lazy<PathBuf> = Lazy::new(|| PITCHFORK_STATE_DIR.join("pids.toml"));
