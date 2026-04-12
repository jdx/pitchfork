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
    // When running under `sudo`, HOME points to /var/root (macOS) or /root (Linux).
    // Resolve the *original* user's home via SUDO_USER so all derived paths
    // (state file, IPC socket, config, logs) remain consistent with the
    // non-sudo invocation. This prevents a second supervisor instance from
    // being spawned in a separate directory tree.
    //
    // Guard: only honour SUDO_USER when the effective UID is 0 (i.e. we are
    // actually running as root). SUDO_USER can leak into non-sudo environments
    // (e.g. inherited env, containers) and would misdirect all state paths.
    #[cfg(unix)]
    if nix::unistd::Uid::effective().is_root() {
        if let Ok(sudo_user) = std::env::var("SUDO_USER") {
            if let Some(home) = home_dir_for_user(&sudo_user) {
                return home;
            }
        }
    }
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
    if let Some(p) = var_path("PITCHFORK_STATE_DIR") {
        return p;
    }
    // Under sudo, dirs::state_dir() would resolve against root's HOME,
    // bypassing our SUDO_USER correction. Use HOME_DIR directly instead.
    #[cfg(unix)]
    if nix::unistd::Uid::effective().is_root() {
        return HOME_DIR.join(".local").join("state").join("pitchfork");
    }
    dirs::state_dir()
        .unwrap_or_else(|| HOME_DIR.join(".local").join("state"))
        .join("pitchfork")
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

/// Look up a user's home directory via the system password database.
/// Returns `None` if the user does not exist or the lookup fails.
#[cfg(unix)]
fn home_dir_for_user(username: &str) -> Option<PathBuf> {
    nix::unistd::User::from_name(username)
        .ok()
        .flatten()
        .map(|u| u.dir)
}
