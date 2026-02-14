use crate::daemon_id::DaemonId;
use crate::error::{ConfigParseError, FileError};
use crate::settings::Settings;
use crate::{Result, env};
use indexmap::IndexMap;
use miette::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

/// Internal structure for reading config files (uses String keys for short daemon names)
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PitchforkTomlRaw {
    #[serde(default)]
    pub daemons: IndexMap<String, PitchforkTomlDaemonRaw>,
    #[serde(default)]
    pub settings: Option<Settings>,
}

/// Internal daemon config for reading (uses String for depends)
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PitchforkTomlDaemonRaw {
    pub run: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub auto: Vec<PitchforkTomlAuto>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron: Option<PitchforkTomlCron>,
    #[serde(default)]
    pub retry: Retry,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_delay: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_http: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_cmd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub boot_start: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub watch: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
    // Hooks
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_ready: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_fail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_cron_trigger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_retry: Option<String>,
}

/// Configuration schema for pitchfork.toml daemon supervisor configuration files.
///
/// Note: When read from a file, daemon keys are short names (e.g., "api").
/// After merging, keys become qualified DaemonIds (e.g., "project/api").
#[derive(Debug, Default, JsonSchema)]
#[schemars(title = "Pitchfork Configuration")]
pub struct PitchforkToml {
    /// Map of daemon IDs to their configurations
    pub daemons: IndexMap<DaemonId, PitchforkTomlDaemon>,
    /// Settings configuration (merged from all config files)
    #[serde(default)]
    pub settings: Settings,
    #[schemars(skip)]
    pub path: Option<PathBuf>,
}

/// Extracts a namespace from a config file path.
///
/// - For user global config (`~/.config/pitchfork/config.toml`): returns "global"
/// - For system global config (`/etc/pitchfork/config.toml`): returns "global"
/// - For project configs: returns the parent directory name
///
/// If the directory name contains `--` (reserved for path encoding), it will be
/// replaced with `-` and a warning will be logged. This ensures safe roundtripping
/// between qualified format (namespace/name) and safe path format (namespace--name).
///
/// Examples:
/// - `~/.config/pitchfork/config.toml` → `"global"`
/// - `/etc/pitchfork/config.toml` → `"global"`
/// - `/home/user/project-a/pitchfork.toml` → `"project-a"`
/// - `/home/user/project-b/sub/pitchfork.toml` → `"sub"`
/// - `/home/user/my--project/pitchfork.toml` → `"my-project"` (with warning)
pub fn namespace_from_path(path: &Path) -> String {
    // Check if this is a global config
    if path == *env::PITCHFORK_GLOBAL_CONFIG_USER || path == *env::PITCHFORK_GLOBAL_CONFIG_SYSTEM {
        return "global".to_string();
    }

    // For project configs, use the parent directory name
    let raw_namespace = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Sanitize the namespace: replace "--" with "-" to avoid ambiguity
    // when converting between qualified (namespace/name) and safe path (namespace--name) formats
    if raw_namespace.contains("--") {
        let sanitized = raw_namespace.replace("--", "-");
        warn!(
            "Directory name '{}' contains '--' (reserved sequence). Using '{}' as namespace instead. \
             Consider renaming the directory to avoid potential conflicts.",
            raw_namespace, sanitized
        );
        sanitized
    } else {
        raw_namespace
    }
}

impl PitchforkToml {
    /// Resolves a user-provided daemon ID to qualified DaemonIds.
    ///
    /// If the ID is already qualified (contains '/'), parses and returns it.
    /// Otherwise, looks up the short ID in the config and returns
    /// matching qualified IDs.
    ///
    /// # Arguments
    /// * `user_id` - The daemon ID provided by the user
    ///
    /// # Returns
    /// A vector of matching DaemonIds (usually one, but could be multiple
    /// if the same short ID exists in multiple namespaces)
    pub fn resolve_daemon_id(&self, user_id: &str) -> Vec<DaemonId> {
        // If already qualified, parse and return
        if user_id.contains('/') {
            return match DaemonId::parse(user_id) {
                Ok(id) => vec![id],
                Err(_) => vec![], // Invalid format
            };
        }

        // Look for matching qualified IDs in the config
        let matches: Vec<DaemonId> = self
            .daemons
            .keys()
            .filter(|id| id.name() == user_id)
            .cloned()
            .collect();

        if matches.is_empty() {
            // If not in config, it might be an ad-hoc daemon
            // Return with "global" namespace as default
            vec![DaemonId::new("global", user_id)]
        } else {
            matches
        }
    }

    /// Resolves a user-provided daemon ID to a qualified DaemonId, preferring the current directory's namespace.
    ///
    /// If the ID is already qualified (contains '/'), parses and returns it.
    /// Otherwise, tries to find a daemon in the current directory's namespace first.
    /// Falls back to any matching daemon if not found in current namespace.
    ///
    /// # Arguments
    /// * `user_id` - The daemon ID provided by the user
    /// * `current_dir` - The current working directory (used to determine namespace preference)
    ///
    /// # Returns
    /// The resolved DaemonId, or an error if the ID format is invalid
    ///
    /// # Errors
    /// Returns an error if `user_id` contains '/' but is not a valid qualified ID
    /// (e.g., "foo/bar/baz" with multiple slashes)
    pub fn resolve_daemon_id_prefer_local(
        &self,
        user_id: &str,
        current_dir: &Path,
    ) -> Result<DaemonId> {
        // If already qualified, parse and return (or error if invalid)
        if user_id.contains('/') {
            return DaemonId::parse(user_id);
        }

        // Determine the current directory's namespace
        // Find the nearest pitchfork.toml to the current directory
        let config_paths = PitchforkToml::list_paths_from(current_dir);
        let current_namespace = config_paths
            .iter()
            .rfind(|p| p.exists()) // Get the most specific (closest) config
            .map(|p| namespace_from_path(p))
            .unwrap_or_else(|| "global".to_string());

        // Try to find the daemon in the current namespace first
        let preferred_id = DaemonId::new(&current_namespace, user_id);
        if self.daemons.contains_key(&preferred_id) {
            return Ok(preferred_id);
        }

        // Fall back to any matching daemon
        let matches = self.resolve_daemon_id(user_id);
        Ok(matches
            .into_iter()
            .next()
            .unwrap_or_else(|| DaemonId::new("global", user_id)))
    }

    /// Convenience method: resolves a single user ID using the merged config and current directory.
    ///
    /// Equivalent to:
    /// ```ignore
    /// PitchforkToml::all_merged().resolve_daemon_id_prefer_local(user_id, &env::CWD)
    /// ```
    ///
    /// # Errors
    /// Returns an error if `user_id` contains '/' but is not a valid qualified ID
    pub fn resolve_id(user_id: &str) -> Result<DaemonId> {
        Self::all_merged().resolve_daemon_id_prefer_local(user_id, &env::CWD)
    }

    /// Convenience method: resolves multiple user IDs using the merged config and current directory.
    ///
    /// Equivalent to:
    /// ```ignore
    /// let config = PitchforkToml::all_merged();
    /// ids.iter().map(|s| config.resolve_daemon_id_prefer_local(s, &env::CWD)).collect()
    /// ```
    ///
    /// # Errors
    /// Returns an error if any ID is malformed
    pub fn resolve_ids<S: AsRef<str>>(user_ids: &[S]) -> Result<Vec<DaemonId>> {
        let config = Self::all_merged();
        user_ids
            .iter()
            .map(|s| config.resolve_daemon_id_prefer_local(s.as_ref(), &env::CWD))
            .collect()
    }

    /// List all configuration file paths from the current working directory.
    /// See `list_paths_from` for details on the search order.
    pub fn list_paths() -> Vec<PathBuf> {
        Self::list_paths_from(&env::CWD)
    }

    /// List all configuration file paths starting from a given directory.
    ///
    /// Returns paths in order of precedence (lowest to highest):
    /// 1. System-level: /etc/pitchfork/config.toml
    /// 2. User-level: ~/.config/pitchfork/config.toml
    /// 3. Project-level: pitchfork.toml and pitchfork.local.toml files
    ///    from filesystem root to the given directory
    ///
    /// Within each directory, pitchfork.toml comes before pitchfork.local.toml,
    /// so local.toml values override the base config.
    pub fn list_paths_from(cwd: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        paths.push(env::PITCHFORK_GLOBAL_CONFIG_SYSTEM.clone());
        paths.push(env::PITCHFORK_GLOBAL_CONFIG_USER.clone());

        // Find both files in one call. Order is reversed so after .reverse():
        // - each directory has pitchfork.toml before pitchfork.local.toml
        // - directories go from root to cwd (later configs override earlier)
        let mut project_paths =
            xx::file::find_up_all(cwd, &["pitchfork.local.toml", "pitchfork.toml"]);
        project_paths.reverse();
        paths.extend(project_paths);

        paths
    }

    /// Merge all configuration files from the current working directory.
    /// See `all_merged_from` for details.
    pub fn all_merged() -> PitchforkToml {
        Self::all_merged_from(&env::CWD)
    }

    /// Merge all configuration files starting from a given directory.
    ///
    /// Reads and merges configuration files in precedence order.
    /// Each daemon ID is qualified with a namespace based on its config file location:
    /// - Global configs (`~/.config/pitchfork/config.toml`) use namespace "global"
    /// - Project configs use the parent directory name as namespace
    ///
    /// This prevents ID conflicts when multiple projects define daemons with the same name.
    pub fn all_merged_from(cwd: &Path) -> PitchforkToml {
        let mut pt = Self::default();
        for p in Self::list_paths_from(cwd) {
            match Self::read(&p) {
                Ok(pt2) => pt.merge(pt2),
                Err(e) => eprintln!("error reading {}: {}", p.display(), e),
            }
        }
        pt
    }
}

impl PitchforkToml {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            settings: Settings::default(),
            path: Some(path),
        }
    }

    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }
        let _lock = xx::fslock::get(path, false)
            .wrap_err_with(|| format!("failed to acquire lock on {}", path.display()))?;
        let raw = std::fs::read_to_string(path).map_err(|e| FileError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;

        // Parse into raw structure first
        let raw_config: PitchforkTomlRaw = toml::from_str(&raw)
            .map_err(|e| ConfigParseError::from_toml_error(path, raw.clone(), e))?;

        // Convert to PitchforkToml with placeholder namespace (will be qualified during merge)
        let namespace = namespace_from_path(path);
        let mut pt = Self::new(path.to_path_buf());

        for (short_name, raw_daemon) in raw_config.daemons {
            let id = DaemonId::new(&namespace, &short_name);

            // Convert depends - support both same-namespace and cross-namespace dependencies
            // - "api" -> same namespace (e.g., "project/api")
            // - "global/postgres" -> cross-namespace reference
            let mut depends = Vec::new();
            for dep in raw_daemon.depends {
                let dep_id = if dep.contains('/') {
                    // Cross-namespace dependency - parse as qualified ID
                    match DaemonId::parse(&dep) {
                        Ok(id) => id,
                        Err(e) => {
                            warn!(
                                "Invalid cross-namespace dependency '{}' in daemon '{}': {}. Skipping.",
                                dep, short_name, e
                            );
                            continue;
                        }
                    }
                } else {
                    // Same namespace dependency
                    DaemonId::new(&namespace, &dep)
                };
                depends.push(dep_id);
            }

            let daemon = PitchforkTomlDaemon {
                run: raw_daemon.run,
                auto: raw_daemon.auto,
                cron: raw_daemon.cron,
                retry: raw_daemon.retry,
                ready_delay: raw_daemon.ready_delay,
                ready_output: raw_daemon.ready_output,
                ready_http: raw_daemon.ready_http,
                ready_port: raw_daemon.ready_port,
                ready_cmd: raw_daemon.ready_cmd,
                boot_start: raw_daemon.boot_start,
                depends,
                watch: raw_daemon.watch,
                dir: raw_daemon.dir,
                env: raw_daemon.env,
                path: Some(path.to_path_buf()),
                on_ready: raw_daemon.on_ready,
                on_fail: raw_daemon.on_fail,
                on_cron_trigger: raw_daemon.on_cron_trigger,
                on_retry: raw_daemon.on_retry,
            };
            pt.daemons.insert(id, daemon);
        }

        // Copy settings if present
        if let Some(settings) = raw_config.settings {
            pt.settings = settings;
        }

        Ok(pt)
    }

    pub fn write(&self) -> Result<()> {
        if let Some(path) = &self.path {
            let _lock = xx::fslock::get(path, false)
                .wrap_err_with(|| format!("failed to acquire lock on {}", path.display()))?;

            // Determine the namespace for this config file
            let config_namespace = namespace_from_path(path);

            // Convert back to raw format for writing (use short names as keys)
            let mut raw = PitchforkTomlRaw::default();
            for (id, daemon) in &self.daemons {
                let raw_daemon = PitchforkTomlDaemonRaw {
                    run: daemon.run.clone(),
                    auto: daemon.auto.clone(),
                    cron: daemon.cron.clone(),
                    retry: daemon.retry,
                    ready_delay: daemon.ready_delay,
                    ready_output: daemon.ready_output.clone(),
                    ready_http: daemon.ready_http.clone(),
                    ready_port: daemon.ready_port,
                    ready_cmd: daemon.ready_cmd.clone(),
                    boot_start: daemon.boot_start,
                    // Preserve cross-namespace dependencies: use qualified ID if namespace differs,
                    // otherwise use short name
                    depends: daemon
                        .depends
                        .iter()
                        .map(|d| {
                            if d.namespace() == config_namespace {
                                d.name().to_string()
                            } else {
                                d.qualified()
                            }
                        })
                        .collect(),
                    watch: daemon.watch.clone(),
                    dir: daemon.dir.clone(),
                    env: daemon.env.clone(),
                    on_ready: daemon.on_ready.clone(),
                    on_fail: daemon.on_fail.clone(),
                    on_cron_trigger: daemon.on_cron_trigger.clone(),
                    on_retry: daemon.on_retry.clone(),
                };
                raw.daemons.insert(id.name().to_string(), raw_daemon);
            }

            let raw_str = toml::to_string(&raw).map_err(|e| FileError::SerializeError {
                path: path.clone(),
                source: e,
            })?;
            xx::file::write(path, &raw_str).map_err(|e| FileError::WriteError {
                path: path.clone(),
                details: Some(e.to_string()),
            })?;
            Ok(())
        } else {
            Err(FileError::NoPath.into())
        }
    }

    /// Simple merge without namespace re-qualification.
    /// Used primarily for testing or when merging configs from the same namespace.
    /// Since read() already qualifies daemon IDs with namespace, this just inserts them.
    /// Settings are also merged - later values override earlier ones.
    pub fn merge(&mut self, pt: Self) {
        for (id, d) in pt.daemons {
            self.daemons.insert(id, d);
        }
        // Merge settings - pt's values override self's values
        self.settings.merge_from(&pt.settings);
    }
}

/// Configuration for a single daemon (internal representation with DaemonId)
#[derive(Debug, Clone, JsonSchema)]
pub struct PitchforkTomlDaemon {
    /// The command to run. Prepend with 'exec' to avoid shell process overhead.
    #[schemars(example = example_run_command())]
    pub run: String,
    /// Automatic start/stop behavior based on shell hooks
    pub auto: Vec<PitchforkTomlAuto>,
    /// Cron scheduling configuration for periodic execution
    pub cron: Option<PitchforkTomlCron>,
    /// Number of times to retry if the daemon fails.
    /// Can be a number (e.g., `3`) or `true` for infinite retries.
    pub retry: Retry,
    /// Delay in seconds before considering the daemon ready
    pub ready_delay: Option<u64>,
    /// Regex pattern to match in stdout/stderr to determine readiness
    pub ready_output: Option<String>,
    /// HTTP URL to poll for readiness (expects 2xx response)
    pub ready_http: Option<String>,
    /// TCP port to check for readiness (connection success = ready)
    #[schemars(range(min = 1, max = 65535))]
    pub ready_port: Option<u16>,
    /// Shell command to poll for readiness (exit code 0 = ready)
    pub ready_cmd: Option<String>,
    /// Whether to start this daemon automatically on system boot
    pub boot_start: Option<bool>,
    /// List of daemon IDs that must be started before this one
    pub depends: Vec<DaemonId>,
    /// File patterns to watch for changes
    pub watch: Vec<String>,
    /// Working directory for the daemon. Relative paths are resolved from the pitchfork.toml location.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dir: Option<String>,
    /// Environment variables to set for the daemon process
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
    #[schemars(skip)]
    pub path: Option<PathBuf>,
    // Hooks
    /// Command to run when the daemon passes its ready check
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_ready: Option<String>,
    /// Command to run when the daemon fails (exits with non-zero code)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_fail: Option<String>,
    /// Command to run when cron triggers (before starting the daemon)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_cron_trigger: Option<String>,
    /// Command to run before each retry attempt
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_retry: Option<String>,
}

fn example_run_command() -> &'static str {
    "exec node server.js"
}

/// Cron scheduling configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct PitchforkTomlCron {
    /// Cron expression (e.g., '0 * * * *' for hourly, '*/5 * * * *' for every 5 minutes)
    #[schemars(example = example_cron_schedule())]
    pub schedule: String,
    /// Behavior when cron triggers while previous run is still active
    #[serde(default = "default_retrigger")]
    pub retrigger: CronRetrigger,
}

fn default_retrigger() -> CronRetrigger {
    CronRetrigger::Finish
}

fn example_cron_schedule() -> &'static str {
    "0 * * * *"
}

/// Retrigger behavior for cron-scheduled daemons
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CronRetrigger {
    /// Retrigger only if the previous run has finished (success or error)
    Finish,
    /// Always retrigger, stopping the previous run if still active
    Always,
    /// Retrigger only if the previous run succeeded
    Success,
    /// Retrigger only if the previous run failed
    Fail,
}

/// Automatic behavior triggered by shell hooks
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PitchforkTomlAuto {
    /// Automatically start when entering the directory
    Start,
    /// Automatically stop when leaving the directory
    Stop,
}

/// Retry configuration that accepts either a boolean or a count.
/// - `true` means retry indefinitely (u32::MAX)
/// - `false` or `0` means no retries
/// - A number means retry that many times
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
pub struct Retry(pub u32);

impl std::fmt::Display for Retry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_infinite() {
            write!(f, "infinite")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl Retry {
    pub const INFINITE: Retry = Retry(u32::MAX);

    pub fn count(&self) -> u32 {
        self.0
    }

    pub fn is_infinite(&self) -> bool {
        self.0 == u32::MAX
    }
}

impl From<u32> for Retry {
    fn from(n: u32) -> Self {
        Retry(n)
    }
}

impl From<bool> for Retry {
    fn from(b: bool) -> Self {
        if b { Retry::INFINITE } else { Retry(0) }
    }
}

impl Serialize for Retry {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize infinite as true, otherwise as number
        if self.is_infinite() {
            serializer.serialize_bool(true)
        } else {
            serializer.serialize_u32(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Retry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct RetryVisitor;

        impl Visitor<'_> for RetryVisitor {
            type Value = Retry;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a boolean or non-negative integer")
            }

            fn visit_bool<E>(self, v: bool) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Retry::from(v))
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    Err(de::Error::custom("retry count cannot be negative"))
                } else if v > u32::MAX as i64 {
                    Ok(Retry::INFINITE)
                } else {
                    Ok(Retry(v as u32))
                }
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v > u32::MAX as u64 {
                    Ok(Retry::INFINITE)
                } else {
                    Ok(Retry(v as u32))
                }
            }
        }

        deserializer.deserialize_any(RetryVisitor)
    }
}
