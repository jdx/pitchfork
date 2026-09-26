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
    //
    // A system boot service has no sudo environment, so `sudo pitchfork boot
    // enable` records the invoking user as `supervisor run --invoking-user`
    // instead. That explicit record takes precedence over SUDO_USER.
    #[cfg(unix)]
    if let Some(home) = invoking_home_dir(
        nix::unistd::Uid::effective().is_root(),
        INVOKING_USER.as_ref().ok().and_then(Option::as_ref),
        std::env::var("SUDO_USER").ok(),
    ) {
        return home;
    }
    dirs::home_dir().unwrap_or_else(|| {
        eprintln!("Warning: Could not determine home directory");
        PathBuf::from("/tmp")
    })
});
/// Flag that records, in a system boot registration, the user who installed it
/// through sudo. See [`InvokingUser`].
pub const INVOKING_USER_FLAG: &str = "--invoking-user";

/// The account a root supervisor acts on behalf of, recorded explicitly with
/// `supervisor run --invoking-user <user>`.
///
/// `sudo pitchfork boot enable` writes this flag into the system service
/// because launchd and systemd start the service without `SUDO_USER`,
/// `SUDO_UID` and `SUDO_GID`. The recorded account then stands in for those
/// variables: it supplies the home directory used for configuration and state
/// lookup, the owner of state files and IPC sockets, and the default daemon
/// identity, exactly as an interactive `sudo` invocation would.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokingUser {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

/// The recorded invoking user of this process, when it is a root
/// `supervisor run --invoking-user <user>`.
///
/// `Err` holds a message explaining why the recorded account could not be used;
/// `supervisor run` refuses to start in that case rather than falling back to
/// root's configuration and identity.
#[cfg(unix)]
pub static INVOKING_USER: Lazy<std::result::Result<Option<InvokingUser>, String>> =
    Lazy::new(|| {
        resolve_invoking_user(
            nix::unistd::Uid::effective().is_root(),
            invoking_user_arg(args_os()).as_deref(),
            lookup_user,
        )
    });

/// Extract the value of `--invoking-user` from a `supervisor run` command line.
///
/// Configuration and state paths are resolved before the CLI is parsed, so the
/// flag is read directly from argv. Only `supervisor run` (or its `sup` alias)
/// accepts it; any other command line yields `None`.
#[cfg(unix)]
pub fn invoking_user_arg(argv: impl IntoIterator<Item = std::ffi::OsString>) -> Option<String> {
    let argv: Vec<String> = argv
        .into_iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let [command, subcommand, rest @ ..] = argv.as_slice() else {
        return None;
    };
    if !matches!(command.as_str(), "supervisor" | "sup") || subcommand != "run" {
        return None;
    }
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        if arg == INVOKING_USER_FLAG {
            return rest.next().cloned();
        }
        if let Some(value) = arg
            .strip_prefix(INVOKING_USER_FLAG)
            .and_then(|v| v.strip_prefix('='))
        {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(unix)]
fn resolve_invoking_user(
    is_root: bool,
    recorded: Option<&str>,
    lookup: impl Fn(&str) -> Option<InvokingUser>,
) -> std::result::Result<Option<InvokingUser>, String> {
    let Some(recorded) = recorded else {
        return Ok(None);
    };
    let recorded = recorded.trim();
    if recorded.is_empty() {
        return Err(format!("{INVOKING_USER_FLAG} requires a user name or UID"));
    }
    if !is_root {
        return Err(format!(
            "{INVOKING_USER_FLAG} {recorded} requires the supervisor to run as root"
        ));
    }
    lookup(recorded).map(Some).ok_or_else(|| {
        format!(
            "the account '{recorded}' recorded by {INVOKING_USER_FLAG} no longer exists; \
            re-register boot start with `sudo pitchfork boot enable` from the account \
            that should own this supervisor"
        )
    })
}

#[cfg(unix)]
fn lookup_user(spec: &str) -> Option<InvokingUser> {
    let user = if spec.chars().all(|c| c.is_ascii_digit()) {
        let uid = spec.parse::<u32>().ok()?;
        nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
    } else {
        nix::unistd::User::from_name(spec)
    }
    .ok()
    .flatten()?;
    Some(InvokingUser {
        name: user.name,
        uid: user.uid.as_raw(),
        gid: user.gid.as_raw(),
        home: user.dir,
    })
}

/// The user a system boot registration should record, if any.
///
/// Inside a supervisor started from such a registration, this is the recorded
/// user, so re-registering (for example after a binary upgrade) keeps it.
/// Otherwise it is the non-root user who ran this process through sudo. A
/// root login shell records nothing and keeps a plain root service.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn boot_service_invoking_user() -> crate::Result<Option<String>> {
    if let Some(user) = INVOKING_USER.clone().map_err(|e| miette::miette!(e))? {
        return Ok(Some(service_user_spec(&user)));
    }
    if !nix::unistd::Uid::effective().is_root() {
        return Ok(None);
    }
    let Some(sudo_user) = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root")
    else {
        return Ok(None);
    };
    let user = lookup_user(&sudo_user)
        .ok_or_else(|| miette::miette!("could not look up the sudo-calling user '{sudo_user}'"))?;
    Ok(Some(service_user_spec(&user)))
}

/// The form of `user` written into a service definition: the user name when
/// it is safe to embed unquoted in a systemd `ExecStart=` line, else the UID.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn service_user_spec(user: &InvokingUser) -> String {
    let safe_name = !user.name.is_empty()
        && !user.name.starts_with('-')
        && !user.name.chars().all(|c| c.is_ascii_digit())
        && user
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if safe_name {
        user.name.clone()
    } else {
        user.uid.to_string()
    }
}

/// UID and GID of the user a root supervisor acts on behalf of: the recorded
/// invoking user, else `SUDO_UID`/`SUDO_GID`.
///
/// Returns `None` unless the effective UID is 0 (root). This prevents stale
/// `SUDO_UID`/`SUDO_GID` values inherited into non-sudo environments from
/// being used.
#[cfg(unix)]
pub fn invoking_user_ids() -> Option<(u32, u32)> {
    resolve_invoking_ids(
        nix::unistd::Uid::effective().is_root(),
        INVOKING_USER.as_ref().ok().and_then(Option::as_ref),
        std::env::var("SUDO_UID").ok(),
        std::env::var("SUDO_GID").ok(),
    )
}

/// Home directory of the user a root process acts on behalf of: the recorded
/// invoking user, else `SUDO_USER`. `None` means the process's own home.
#[cfg(unix)]
fn invoking_home_dir(
    is_root: bool,
    recorded: Option<&InvokingUser>,
    sudo_user: Option<String>,
) -> Option<PathBuf> {
    if !is_root {
        return None;
    }
    if let Some(user) = recorded {
        return Some(user.home.clone());
    }
    home_dir_for_user(&sudo_user?)
}

#[cfg(unix)]
fn resolve_invoking_ids(
    is_root: bool,
    recorded: Option<&InvokingUser>,
    sudo_uid: Option<String>,
    sudo_gid: Option<String>,
) -> Option<(u32, u32)> {
    if !is_root {
        return None;
    }
    if let Some(user) = recorded {
        return Some((user.uid, user.gid));
    }
    let uid: u32 = sudo_uid?.parse().ok()?;
    let gid: u32 = sudo_gid?.parse().ok()?;
    Some((uid, gid))
}

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

// Unix domain sockets only; Windows IPC uses named pipes, see `ipc::fs_name`.
#[cfg(unix)]
pub static IPC_SOCK_DIR: Lazy<PathBuf> = Lazy::new(|| PITCHFORK_STATE_DIR.join("sock"));
#[cfg(unix)]
pub static IPC_SOCK_MAIN: Lazy<PathBuf> = Lazy::new(|| IPC_SOCK_DIR.join("main.sock"));

// Capture the PATH at startup so daemons can find user tools
pub static ORIGINAL_PATH: Lazy<Option<String>> = Lazy::new(|| var("PATH").ok());

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

    #[cfg(unix)]
    fn argv(args: &[&str]) -> Vec<std::ffi::OsString> {
        std::iter::once("pitchfork")
            .chain(args.iter().copied())
            .map(Into::into)
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn invoking_user_arg_reads_supervisor_run_flag() {
        for args in [
            &["supervisor", "run", "--boot", "--invoking-user", "alice"][..],
            &["supervisor", "run", "--invoking-user=alice", "--boot"],
            &["sup", "run", "--invoking-user", "alice"],
        ] {
            assert_eq!(invoking_user_arg(argv(args)).as_deref(), Some("alice"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn invoking_user_arg_ignores_other_commands() {
        for args in [
            &["supervisor", "run", "--boot"][..],
            &["supervisor", "start", "--invoking-user", "alice"],
            &["run", "--invoking-user", "alice"],
            &["supervisor", "run", "--", "--invoking-user", "alice"],
            &[],
        ] {
            assert_eq!(invoking_user_arg(argv(args)), None);
        }
    }

    #[cfg(unix)]
    fn alice() -> InvokingUser {
        InvokingUser {
            name: "alice".into(),
            uid: 501,
            gid: 20,
            home: PathBuf::from("/Users/alice"),
        }
    }

    #[cfg(unix)]
    fn lookup_alice(spec: &str) -> Option<InvokingUser> {
        (spec == "alice" || spec == "501").then(alice)
    }

    /// A boot service started by launchd or systemd has no SUDO_* variables;
    /// the recorded user alone must supply the home, ownership, and identity
    /// that an interactive sudo invocation would.
    #[cfg(unix)]
    #[test]
    fn recorded_user_replaces_missing_sudo_environment() {
        let user = resolve_invoking_user(true, Some("alice"), lookup_alice)
            .unwrap()
            .unwrap();
        assert_eq!(user, alice());
        assert_eq!(
            invoking_home_dir(true, Some(&user), None),
            Some(PathBuf::from("/Users/alice"))
        );
        assert_eq!(
            resolve_invoking_ids(true, Some(&user), None, None),
            Some((501, 20))
        );
        assert_eq!(
            resolve_invoking_user(true, Some("501"), lookup_alice).unwrap(),
            Some(alice())
        );
    }

    #[cfg(unix)]
    #[test]
    fn recorded_user_takes_precedence_over_sudo_environment() {
        let user = alice();
        assert_eq!(
            resolve_invoking_ids(true, Some(&user), Some("502".into()), Some("30".into())),
            Some((501, 20))
        );
        assert_eq!(
            invoking_home_dir(true, Some(&user), Some("root".into())),
            Some(PathBuf::from("/Users/alice"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn sudo_environment_still_applies_without_recorded_user() {
        assert_eq!(
            resolve_invoking_ids(true, None, Some("502".into()), Some("30".into())),
            Some((502, 30))
        );
    }

    /// A service registered from a root login shell records no user and has no
    /// sudo environment: it keeps running entirely as root.
    #[cfg(unix)]
    #[test]
    fn root_shell_service_has_no_invoking_user() {
        assert_eq!(resolve_invoking_user(true, None, lookup_alice), Ok(None));
        assert_eq!(invoking_home_dir(true, None, None), None);
        assert_eq!(resolve_invoking_ids(true, None, None, None), None);
    }

    #[cfg(unix)]
    #[test]
    fn missing_recorded_user_fails_instead_of_falling_back_to_root() {
        let err = resolve_invoking_user(true, Some("bob"), lookup_alice).unwrap_err();
        assert!(err.contains("'bob'"), "{err}");
        assert!(err.contains("no longer exists"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn recorded_user_requires_root() {
        let err = resolve_invoking_user(false, Some("alice"), lookup_alice).unwrap_err();
        assert!(
            err.contains("requires the supervisor to run as root"),
            "{err}"
        );
        let user = alice();
        assert_eq!(invoking_home_dir(false, Some(&user), None), None);
        assert_eq!(resolve_invoking_ids(false, Some(&user), None, None), None);
    }

    #[cfg(unix)]
    #[test]
    fn empty_recorded_user_is_rejected() {
        assert!(resolve_invoking_user(true, Some(" "), lookup_alice).is_err());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn service_user_spec_prefers_name_and_falls_back_to_uid() {
        assert_eq!(service_user_spec(&alice()), "alice");
        for name in ["alice smith", "-alice", "1234", "al$ice", ""] {
            let user = InvokingUser {
                name: name.into(),
                ..alice()
            };
            assert_eq!(service_user_spec(&user), "501", "{name:?}");
        }
    }

    #[test]
    fn expand_tilde_for_user_leaves_non_tilde_unchanged() {
        assert_eq!(
            expand_tilde_for_user("/srv/api", Some("postgres")),
            Path::new("/srv/api")
        );
    }
}
