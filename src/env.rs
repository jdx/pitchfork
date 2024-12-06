use once_cell::sync::Lazy;
use std::path::PathBuf;
use log::trace;

pub static HOME_DIR: Lazy<PathBuf> = Lazy::new(|| dirs::home_dir().unwrap_or(PathBuf::new()));
pub static PITCHFORK_STATE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    dirs::state_dir()
        .unwrap_or(HOME_DIR.join(".local").join("state"))
        .join("pitchfork")
});
pub static PITCHFORK_PID_FILE: Lazy<PathBuf> = Lazy::new(|| PITCHFORK_STATE_DIR.join("pid"));
pub static PITCHFORK_PID: Lazy<Option<u32>> = Lazy::new(|| {
    if PITCHFORK_PID_FILE.exists() {
        let pid = xx::file::read_to_string(&*PITCHFORK_PID_FILE).unwrap_or_default().parse().unwrap_or_default();
        trace!("Read pid from {}: {}", PITCHFORK_PID_FILE.display(), pid);
        if psutil::process::pid_exists(pid) {
            return Some(pid);
        }
    }
    None
});
