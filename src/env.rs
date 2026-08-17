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
    if nix::unistd::Uid::effective().is_root()
        && let Ok(sudo_user) = std::env::var("SUDO_USER")
        && let Some(home) = home_dir_for_user(&sudo_user)
    {
        return home;
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
    #[cfg(unix)]
    if nix::unistd::Uid::effective().is_root()
        && let Some(home) = configured_supervisor_user_home_dir()
    {
        return home.join(".local").join("state").join("pitchfork");
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
/// Path to the hosts file managed by the proxy's hosts sync.
///
/// `PITCHFORK_HOSTS_FILE` overrides the platform default; tests use it to
/// keep the sync away from the real system hosts file.
pub static PITCHFORK_HOSTS_FILE: Lazy<PathBuf> = Lazy::new(|| {
    if let Some(p) = var_path("PITCHFORK_HOSTS_FILE") {
        return p;
    }
    if cfg!(windows) {
        let system_root = var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        PathBuf::from(system_root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
});
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

/// Expand a leading `~` path component to the current Pitchfork user's home.
///
/// This intentionally supports only `~` and `~/...`, not `~user` or shell
/// expansions such as `$HOME`. Pitchfork's home resolution accounts for the
/// original user when running under `sudo`.
pub fn expand_tilde(path: impl AsRef<std::path::Path>) -> PathBuf {
    expand_tilde_for_user(path, None)
}

/// Expand a leading `~` to the home directory of `user`.
///
/// When `user` is `None`, empty, or the system lookup fails, falls back to
/// `HOME_DIR` (the supervisor's home). This matches Unix semantics where `~`
/// in a process's working directory refers to that process's effective user.
///
/// Only `~` and `~/...` are supported — not `~user` or shell expansions.
pub fn expand_tilde_for_user(path: impl AsRef<std::path::Path>, user: Option<&str>) -> PathBuf {
    let path = path.as_ref();
    match path.strip_prefix("~") {
        Ok(rest) => home_dir_for_effective_user(user).join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn var_path(name: &str) -> Option<PathBuf> {
    var(name).map(expand_tilde).ok()
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

/// Look up a home directory by username or numeric UID string.
#[cfg(unix)]
fn home_dir_by_user_spec(user: &str) -> Option<PathBuf> {
    if user.chars().all(|c| c.is_ascii_digit()) {
        let uid = user.parse::<u32>().ok()?;
        nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
            .ok()
            .flatten()
            .map(|u| u.dir)
    } else {
        home_dir_for_user(user)
    }
}

/// Resolve the home directory for an effective daemon user.
///
/// Returns `HOME_DIR` when `user` is `None`, empty, or the lookup fails.
#[cfg(unix)]
pub(crate) fn home_dir_for_effective_user(user: Option<&str>) -> PathBuf {
    let user = user.map(str::trim).filter(|u| !u.is_empty());
    match user {
        Some(u) => home_dir_by_user_spec(u).unwrap_or_else(|| HOME_DIR.clone()),
        None => HOME_DIR.clone(),
    }
}

#[cfg(not(unix))]
pub(crate) fn home_dir_for_effective_user(_user: Option<&str>) -> PathBuf {
    HOME_DIR.clone()
}

#[cfg(unix)]
fn configured_supervisor_user_home_dir() -> Option<PathBuf> {
    let s = crate::settings::settings();
    let user = s.supervisor.user.trim();
    if user.is_empty() {
        return None;
    }
    home_dir_by_user_spec(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn expand_tilde_replaces_home_prefix() {
        assert_eq!(
            expand_tilde("~/projects/api"),
            HOME_DIR.join("projects/api")
        );
        assert_eq!(expand_tilde("~"), *HOME_DIR);
    }

    #[test]
    fn expand_tilde_leaves_other_paths_unchanged() {
        assert_eq!(
            expand_tilde("/srv/projects/api"),
            Path::new("/srv/projects/api")
        );
        assert_eq!(expand_tilde("projects/api"), Path::new("projects/api"));
        assert_eq!(expand_tilde("~other/api"), Path::new("~other/api"));
    }

    #[test]
    fn expand_tilde_for_user_none_uses_supervisor_home() {
        assert_eq!(expand_tilde_for_user("~/data", None), HOME_DIR.join("data"));
    }

    #[test]
    fn expand_tilde_for_user_empty_uses_supervisor_home() {
        assert_eq!(
            expand_tilde_for_user("~/data", Some("")),
            HOME_DIR.join("data")
        );
    }

    #[test]
    fn expand_tilde_for_user_nonexistent_falls_back_to_supervisor_home() {
        assert_eq!(
            expand_tilde_for_user("~/data", Some("nonexistent_user_xyz")),
            HOME_DIR.join("data")
        );
    }

    #[test]
    fn expand_tilde_for_user_leaves_non_tilde_unchanged() {
        assert_eq!(
            expand_tilde_for_user("/srv/api", Some("postgres")),
            Path::new("/srv/api")
        );
    }
}
