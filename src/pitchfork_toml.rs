use crate::daemon_id::DaemonId;
use crate::error::{ConfigParseError, DependencyError, FileError, find_similar_daemon};
use crate::settings::SettingsPartial;
use crate::settings::settings;
use crate::state_file::StateFile;
use crate::{Result, env};
use indexmap::IndexMap;
use miette::Context;
use schemars::JsonSchema;
use std::path::{Path, PathBuf};

// Re-export config value types so existing `use crate::pitchfork_toml::X` paths keep working.
pub use crate::config_types::{
    CpuLimit, CronRetrigger, Dir, MemoryLimit, OnOutputHook, PitchforkTomlAuto, PitchforkTomlCron,
    PitchforkTomlHooks, PortBump, PortConfig, ReadyHttp, ReadyPort, Retry, StopConfig, StopSignal,
    WatchMode,
};

/// Raw slug entry as read from TOML (uses String for dir path).
/// Format in global config:
/// ```toml
/// [slugs]
/// api = { dir = "/home/user/my-api", daemon = "server" }
/// docs = { dir = "/home/user/docs-site" }  # daemon defaults to slug name
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SlugEntryRaw {
    /// Project directory containing the pitchfork.toml
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Namespace reference (alternative to dir)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Daemon name within that project (defaults to slug name if omitted)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub daemon: Option<String>,
}

/// Resolved slug entry with PathBuf.
#[derive(Debug, Clone)]
pub struct SlugEntry {
    /// Project directory containing the pitchfork.toml
    pub dir: Option<PathBuf>,
    /// Namespace reference (alternative to dir)
    pub namespace: Option<String>,
    /// Daemon name within that project (defaults to slug name if omitted)
    pub daemon: Option<String>,
}

impl SlugEntry {
    /// Resolve the project directory.
    /// If `dir` is set, use it. Otherwise look up `namespace` in the global namespace registry.
    pub fn resolve_dir(&self) -> Option<PathBuf> {
        self.dir.clone().or_else(|| {
            self.namespace.as_ref().and_then(|ns| {
                let namespaces = PitchforkToml::read_global_namespaces();
                namespaces.get(ns).map(|entry| entry.dir.clone())
            })
        })
    }

    /// Resolve the namespace name.
    /// If `namespace` is set, use it. Otherwise derive from `dir` via `namespace_for_dir`.
    pub fn resolve_namespace(&self) -> Option<String> {
        self.namespace.clone().or_else(|| {
            self.resolve_dir()
                .and_then(|dir| PitchforkToml::namespace_for_dir(&dir).ok())
        })
    }
}

/// Raw group entry as read from TOML.
/// ```toml
/// [groups.backend]
/// daemons = ["api", "worker"]
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupEntryRaw {
    pub daemons: Vec<String>,
}

/// Resolved group entry with qualified DaemonIds.
#[derive(Debug, Clone)]
pub struct GroupEntry {
    pub daemons: Vec<DaemonId>,
}

/// Raw namespace entry as read from TOML.
/// ```toml
/// [namespaces.myproject]
/// dir = "/home/user/projects/myproject"
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NamespaceEntryRaw {
    /// Project directory containing the pitchfork.toml
    pub dir: String,
}

/// Resolved namespace entry with PathBuf.
#[derive(Debug, Clone)]
pub struct NamespaceEntry {
    /// Project directory containing the pitchfork.toml
    pub dir: PathBuf,
}

/// Internal structure for reading config files (uses String keys for short daemon names)
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PitchforkTomlRaw {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub daemons: IndexMap<String, PitchforkTomlDaemonRaw>,
    #[serde(default)]
    pub settings: Option<SettingsPartial>,
    /// Slug registry (only meaningful in global config).
    /// Maps slug names to their configuration (dir + optional daemon name).
    #[serde(skip_serializing_if = "IndexMap::is_empty", default)]
    pub slugs: IndexMap<String, SlugEntryRaw>,
    /// Named groups of daemons for batch operations.
    #[serde(skip_serializing_if = "IndexMap::is_empty", default)]
    pub groups: IndexMap<String, GroupEntryRaw>,
    /// Namespace registry (only meaningful in global config).
    /// Maps namespace names to their project directory.
    #[serde(skip_serializing_if = "IndexMap::is_empty", default)]
    pub namespaces: IndexMap<String, NamespaceEntryRaw>,
}

/// Per-daemon log configuration sub-table `[daemons.<name>.logs]`.
///
/// Fields here override the top-level daemon fields (`time_retention`,
/// `line_retention`, `archive_hook`) and the global `[settings.logs]` defaults.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct PitchforkTomlDaemonLogs {
    /// Log line format: `json`, `logfmt`, or `text`.
    /// Defaults to `text` (no parsing).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub log_format: Option<String>,
    /// Maximum age of log entries to keep (e.g. "7d", "30d").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub time_retention: Option<String>,
    /// Maximum number of log entries to keep per daemon.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line_retention: Option<i64>,
    /// Archive hook command invoked before retention prunes this daemon's logs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archive_hook: Option<String>,
}

/// Internal daemon config for reading (uses String for depends).
///
/// Note: This struct mirrors `PitchforkTomlDaemon` but uses `Vec<String>` for `depends`
/// (before namespace resolution) and has serde attributes for TOML serialization.
/// When adding new fields, remember to update both structs and the conversion code
/// in `read()` and `write()`.
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
    pub ready_http: Option<ReadyHttp>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_port: Option<ReadyPort>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_cmd: Option<String>,
    /// New port configuration (preferred)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub port: Option<PortConfig>,
    /// Deprecated: use `port` instead
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub expected_port: Vec<u16>,
    /// Deprecated: use `port.bump` instead
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auto_bump_port: Option<bool>,
    /// Deprecated: use `port.bump` instead
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub port_bump_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub boot_start: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub watch: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub watch_mode: Option<WatchMode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hooks: Option<PitchforkTomlHooks>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mise: Option<bool>,
    /// Unix user to run this daemon as.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user: Option<String>,
    /// Memory limit for the daemon process (e.g. "50MB", "1GiB")
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_limit: Option<MemoryLimit>,
    /// CPU usage limit as a percentage (e.g. 80 for 80%, 200 for 2 cores)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cpu_limit: Option<CpuLimit>,
    /// Unix signal to send for graceful shutdown (default: SIGTERM)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stop_signal: Option<StopConfig>,
    /// Allocate a pseudo-terminal for the daemon process.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pty: Option<bool>,
    /// Maximum age of log entries to keep (e.g. "7d", "30d").
    /// Overrides the global `settings.logs.time_retention` when set.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub time_retention: Option<String>,
    /// Maximum number of log entries to keep per daemon.
    /// Overrides the global `settings.logs.line_retention` when set.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line_retention: Option<i64>,
    /// Archive hook command invoked before retention prunes this daemon's logs.
    /// Overrides the global `settings.logs.archive_hook.command` when set.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archive_hook: Option<String>,
    /// Per-daemon log configuration sub-table.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub logs: Option<PitchforkTomlDaemonLogs>,
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
    /// Optional explicit namespace declared in this file.
    ///
    /// This applies to per-file read/write flows. Merged configs may contain
    /// daemons from multiple namespaces and leave this as `None`.
    pub namespace: Option<String>,
    /// Settings configuration (merged from all config files).
    ///
    /// **Note:** This field exists for serialization round-trips and for
    /// `PitchforkToml::merge()` to collect per-file overrides.  It is **not**
    /// consumed by the global `settings()` singleton, which is populated
    /// independently by `Settings::load()` to avoid a circular dependency
    /// between `PitchforkToml` and `Settings`.  Do not rely on mutations to
    /// this field being reflected in `settings()`.
    #[serde(default)]
    pub(crate) settings: SettingsPartial,
    /// Slug registry (merged from global config files).
    /// Maps slug names to their project directory and optional daemon name.
    /// Only populated from global config files (`~/.config/pitchfork/config.toml`
    /// or `/etc/pitchfork/config.toml`).
    #[schemars(skip)]
    pub slugs: IndexMap<String, SlugEntry>,
    /// Named groups of daemons for batch operations.
    #[schemars(skip)]
    pub groups: IndexMap<String, GroupEntry>,
    /// Namespace registry (merged from global config files).
    /// Maps namespace names to their project directory.
    #[schemars(skip)]
    pub namespaces: IndexMap<String, NamespaceEntry>,
    #[schemars(skip)]
    pub path: Option<PathBuf>,
}

pub(crate) fn is_global_config(path: &Path) -> bool {
    path == *env::PITCHFORK_GLOBAL_CONFIG_USER || path == *env::PITCHFORK_GLOBAL_CONFIG_SYSTEM
}

fn is_local_config(path: &Path) -> bool {
    path.file_name()
        .map(|n| n == "pitchfork.local.toml")
        .unwrap_or(false)
}

pub(crate) fn is_dot_config_pitchfork(path: &Path) -> bool {
    path.ends_with(".config/pitchfork.toml") || path.ends_with(".config/pitchfork.local.toml")
}

fn sibling_base_config(path: &Path) -> Option<PathBuf> {
    if !is_local_config(path) {
        return None;
    }
    path.parent().map(|p| p.join("pitchfork.toml"))
}

fn parse_namespace_override_from_content(path: &Path, content: &str) -> Result<Option<String>> {
    use toml::Value;

    let doc: Value = toml::from_str(content)
        .map_err(|e| ConfigParseError::from_toml_error(path, content.to_string(), e))?;
    let Some(value) = doc.get("namespace") else {
        return Ok(None);
    };

    match value {
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(ConfigParseError::InvalidNamespace {
            path: path.to_path_buf(),
            namespace: value.to_string(),
            reason: "top-level 'namespace' must be a string".to_string(),
        }
        .into()),
    }
}

fn read_namespace_override_from_file(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).map_err(|e| FileError::ReadError {
        path: path.to_path_buf(),
        source: e,
    })?;
    parse_namespace_override_from_content(path, &content)
}

fn validate_namespace(path: &Path, namespace: &str) -> Result<String> {
    if let Err(e) = DaemonId::try_new(namespace, "probe") {
        return Err(ConfigParseError::InvalidNamespace {
            path: path.to_path_buf(),
            namespace: namespace.to_string(),
            reason: e.to_string(),
        }
        .into());
    }
    Ok(namespace.to_string())
}

fn derive_namespace_from_dir(path: &Path) -> Result<String> {
    let dir_for_namespace = if is_dot_config_pitchfork(path) {
        path.parent().and_then(|p| p.parent())
    } else {
        path.parent()
    };

    let raw_namespace = dir_for_namespace
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .ok_or_else(|| miette::miette!("cannot derive namespace from path '{}'", path.display()))?
        .to_string();

    validate_namespace(path, &raw_namespace).map_err(|e| {
        ConfigParseError::InvalidNamespace {
            path: path.to_path_buf(),
            namespace: raw_namespace,
            reason: format!(
                "{e}. Set a valid top-level namespace, e.g. namespace = \"my-project\""
            ),
        }
        .into()
    })
}

fn namespace_from_path_with_override(path: &Path, explicit: Option<&str>) -> Result<String> {
    if is_global_config(path) {
        if let Some(ns) = explicit
            && ns != "global"
        {
            return Err(ConfigParseError::InvalidNamespace {
                path: path.to_path_buf(),
                namespace: ns.to_string(),
                reason: "global config files must use namespace 'global'".to_string(),
            }
            .into());
        }
        return Ok("global".to_string());
    }

    if let Some(ns) = explicit {
        return validate_namespace(path, ns);
    }

    derive_namespace_from_dir(path)
}

fn namespace_from_file(path: &Path) -> Result<String> {
    let explicit = read_namespace_override_from_file(path)?;
    let base_explicit = sibling_base_config(path)
        .and_then(|p| if p.exists() { Some(p) } else { None })
        .map(|p| read_namespace_override_from_file(&p))
        .transpose()?
        .flatten();

    if let (Some(local_ns), Some(base_ns)) = (explicit.as_deref(), base_explicit.as_deref())
        && local_ns != base_ns
    {
        return Err(ConfigParseError::InvalidNamespace {
            path: path.to_path_buf(),
            namespace: local_ns.to_string(),
            reason: format!(
                "namespace '{local_ns}' does not match sibling pitchfork.toml namespace '{base_ns}'"
            ),
        }
        .into());
    }

    let effective_explicit = explicit.as_deref().or(base_explicit.as_deref());
    namespace_from_path_with_override(path, effective_explicit)
}

/// Extracts a namespace from a config file path.
///
/// - For user global config (`~/.config/pitchfork/config.toml`): returns "global"
/// - For system global config (`/etc/pitchfork/config.toml`): returns "global"
/// - For project configs: uses top-level `namespace` if present, otherwise parent directory name
///
/// Examples:
/// - `~/.config/pitchfork/config.toml` → `"global"`
/// - `/etc/pitchfork/config.toml` → `"global"`
/// - `/home/user/project-a/pitchfork.toml` → `"project-a"`
/// - `/home/user/project-b/sub/pitchfork.toml` → `"sub"`
/// - `/home/user/中文目录/pitchfork.toml` → error unless `namespace = "..."` is set
pub fn namespace_from_path(path: &Path) -> Result<String> {
    namespace_from_file(path)
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
    /// A Result containing a vector of matching DaemonIds (usually one, but could be multiple
    /// if the same short ID exists in multiple namespaces), or an error if the ID is invalid.
    pub fn resolve_daemon_id(&self, user_id: &str) -> Result<Vec<DaemonId>> {
        // If already qualified, parse and return
        if user_id.contains('/') {
            return match DaemonId::parse(user_id) {
                Ok(id) => Ok(vec![id]),
                Err(e) => Err(e), // Invalid format - propagate error
            };
        }

        // Check for slug match in global slugs registry
        let global_slugs = Self::read_global_slugs();
        if let Some(entry) = global_slugs.get(user_id) {
            // Load the project's config from the slug's dir to find the daemon ID
            let daemon_name = entry.daemon.as_deref().unwrap_or(user_id);
            if let Some(dir) = entry.resolve_dir() {
                if let Ok(project_config) = Self::all_merged_from(&dir) {
                    // Find daemon by short name in that project
                    let matches: Vec<DaemonId> = project_config
                        .daemons
                        .keys()
                        .filter(|id| id.name() == daemon_name)
                        .cloned()
                        .collect();
                    match matches.as_slice() {
                        [] => {}
                        [id] => return Ok(vec![id.clone()]),
                        _ => {
                            let mut candidates: Vec<String> =
                                matches.iter().map(|id| id.qualified()).collect();
                            candidates.sort();
                            return Err(miette::miette!(
                                "slug '{}' maps to daemon '{}' which matches multiple daemons: {}",
                                user_id,
                                daemon_name,
                                candidates.join(", ")
                            ));
                        }
                    }
                }
            }
        }

        // Look for matching qualified IDs in the config
        let matches: Vec<DaemonId> = self
            .daemons
            .keys()
            .filter(|id| id.name() == user_id)
            .cloned()
            .collect();

        if matches.is_empty() {
            // No config matches. Search state file for any daemon with matching short name.
            let state_matches = Self::find_in_state_file(user_id);
            match state_matches.as_slice() {
                [] => {}
                [id] => return Ok(vec![id.clone()]),
                _ => {
                    let mut candidates: Vec<String> =
                        state_matches.iter().map(|id| id.qualified()).collect();
                    candidates.sort();
                    return Err(miette::miette!(
                        "daemon '{}' is ambiguous; matches: {}. Use a qualified daemon ID (namespace/name)",
                        user_id,
                        candidates.join(", ")
                    ));
                }
            }
            // No config or state matches. Validate short ID format and return no matches.
            let _ = DaemonId::try_new("global", user_id)?;
        }
        Ok(matches)
    }

    /// Finds all daemons in the persisted state file whose short name matches `short_name`.
    ///
    /// Logs a warning if the state file exists but cannot be read or parsed.
    ///
    /// Returns the matching `DaemonId`s. The caller must handle zero / one / many cases.
    fn find_in_state_file(short_name: &str) -> Vec<DaemonId> {
        match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
            Ok(state) => state
                .daemons
                .keys()
                .filter(|id| id.name() == short_name)
                .cloned()
                .collect(),
            Err(e) => {
                warn!("cannot read state file: {e}");
                Vec::new()
            }
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
    /// (e.g., "foo/bar/baz" with multiple slashes), or if `user_id` contains invalid characters.
    ///
    /// # Warnings
    /// If multiple daemons match the short name and none is in the current namespace,
    /// a warning is logged to stderr indicating the ambiguity.
    #[allow(dead_code)]
    pub fn resolve_daemon_id_prefer_local(
        &self,
        user_id: &str,
        current_dir: &Path,
    ) -> Result<DaemonId> {
        // If already qualified, parse and return (or error if invalid)
        if user_id.contains('/') {
            return DaemonId::parse(user_id);
        }

        // Determine the current directory's namespace by finding the nearest
        // pitchfork.toml. Cache the namespace in the caller when resolving
        // multiple IDs to avoid repeated filesystem traversal.
        let current_namespace = Self::namespace_for_dir(current_dir)?;

        self.resolve_daemon_id_with_namespace(user_id, &current_namespace)
    }

    /// Like `resolve_daemon_id_prefer_local` but accepts a pre-computed namespace,
    /// avoiding redundant filesystem traversal when resolving multiple IDs.
    fn resolve_daemon_id_with_namespace(
        &self,
        user_id: &str,
        current_namespace: &str,
    ) -> Result<DaemonId> {
        // Check for slug match in global slugs registry
        let global_slugs = Self::read_global_slugs();
        if let Some(entry) = global_slugs.get(user_id) {
            let daemon_name = entry.daemon.as_deref().unwrap_or(user_id);
            if let Some(dir) = entry.resolve_dir() {
                if let Ok(project_config) = Self::all_merged_from(&dir) {
                    let matches: Vec<DaemonId> = project_config
                        .daemons
                        .keys()
                        .filter(|id| id.name() == daemon_name)
                        .cloned()
                        .collect();
                    match matches.as_slice() {
                        [] => {}
                        [id] => return Ok(id.clone()),
                        _ => {
                            let mut candidates: Vec<String> =
                                matches.iter().map(|id| id.qualified()).collect();
                            candidates.sort();
                            return Err(miette::miette!(
                                "slug '{}' maps to daemon '{}' which matches multiple daemons: {}",
                                user_id,
                                daemon_name,
                                candidates.join(", ")
                            ));
                        }
                    }
                }
            }
        }

        // Try to find the daemon in the current namespace first
        // Use try_new to validate user input
        let preferred_id = DaemonId::try_new(current_namespace, user_id)?;
        if self.daemons.contains_key(&preferred_id) {
            return Ok(preferred_id);
        }

        // Fall back to any matching daemon
        let matches = self.resolve_daemon_id(user_id)?;

        // Error on ambiguity instead of implicitly preferring global.
        if matches.len() > 1 {
            let mut candidates: Vec<String> = matches.iter().map(|id| id.qualified()).collect();
            candidates.sort();
            return Err(miette::miette!(
                "daemon '{}' is ambiguous; matches: {}. Use a qualified daemon ID (namespace/name)",
                user_id,
                candidates.join(", ")
            ));
        }

        if let Some(id) = matches.into_iter().next() {
            return Ok(id);
        }

        // If not found in current namespace or merged config matches, only fall back
        // to global when it is explicitly configured.
        let global_id = DaemonId::try_new("global", user_id)?;
        if self.daemons.contains_key(&global_id) {
            return Ok(global_id);
        }

        let suggestion = find_similar_daemon(user_id, self.daemons.keys().map(|id| id.name()));
        Err(DependencyError::DaemonNotFound {
            name: user_id.to_string(),
            suggestion,
        }
        .into())
    }

    /// Returns the effective namespace for the given directory by finding
    /// the nearest config file. Traverses the filesystem at most once per call.
    pub fn namespace_for_dir(dir: &Path) -> Result<String> {
        Ok(Self::list_paths_from(dir)
            .iter()
            .rfind(|p| p.exists()) // most specific (closest) config
            .map(|p| namespace_from_path(p))
            .transpose()?
            .unwrap_or_else(|| "global".to_string()))
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
        if user_id.contains('/') {
            return DaemonId::parse(user_id);
        }

        // Compute the namespace once and reuse it — avoids a second traversal
        // inside resolve_daemon_id_prefer_local.
        let config = Self::all_merged()?;
        let ns = Self::namespace_for_dir(&env::CWD)?;
        config.resolve_daemon_id_with_namespace(user_id, &ns)
    }

    /// Like `resolve_id`, but allows ad-hoc short IDs by falling back to
    /// `global/<id>` when no configured daemon matches.
    ///
    /// This is intended for commands such as `pitchfork run` that create
    /// managed daemons without requiring prior config entries.
    pub fn resolve_id_allow_adhoc(user_id: &str) -> Result<DaemonId> {
        if user_id.contains('/') {
            return DaemonId::parse(user_id);
        }

        let config = Self::all_merged()?;
        let ns = Self::namespace_for_dir(&env::CWD)?;

        let preferred_id = DaemonId::try_new(&ns, user_id)?;
        if config.daemons.contains_key(&preferred_id) {
            return Ok(preferred_id);
        }

        let matches = config.resolve_daemon_id(user_id)?;
        if matches.len() > 1 {
            let mut candidates: Vec<String> = matches.iter().map(|id| id.qualified()).collect();
            candidates.sort();
            return Err(miette::miette!(
                "daemon '{}' is ambiguous; matches: {}. Use a qualified daemon ID (namespace/name)",
                user_id,
                candidates.join(", ")
            ));
        }
        if let Some(id) = matches.into_iter().next() {
            return Ok(id);
        }

        DaemonId::try_new("global", user_id)
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
        // Fast path: all IDs are already qualified and can be parsed directly.
        if user_ids.iter().all(|s| s.as_ref().contains('/')) {
            return user_ids
                .iter()
                .map(|s| DaemonId::parse(s.as_ref()))
                .collect();
        }

        let config = Self::all_merged()?;
        // Compute namespace once for all IDs
        let ns = Self::namespace_for_dir(&env::CWD)?;
        user_ids
            .iter()
            .map(|s| {
                let id = s.as_ref();
                if id.contains('/') {
                    DaemonId::parse(id)
                } else {
                    config.resolve_daemon_id_with_namespace(id, &ns)
                }
            })
            .collect()
    }

    /// Resolve explicit daemon IDs and/or a group name into a deduplicated list of DaemonIds.
    ///
    /// This is more efficient than calling `resolve_ids` and `resolve_group` separately
    /// because it reads the merged config only once.
    pub fn resolve_ids_and_group<S: AsRef<str>>(
        user_ids: &[S],
        group_name: Option<&str>,
    ) -> Result<Vec<DaemonId>> {
        let config = Self::all_merged()?;
        let ns = Self::namespace_for_dir(&env::CWD)?;
        let mut ids = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for id in user_ids {
            let id_str = id.as_ref();
            let daemon_id = if id_str.contains('/') {
                DaemonId::parse(id_str)?
            } else {
                config.resolve_daemon_id_with_namespace(id_str, &ns)?
            };
            if seen.insert(daemon_id.clone()) {
                ids.push(daemon_id);
            }
        }

        if let Some(name) = group_name {
            match config.groups.get(name) {
                Some(group) => {
                    let missing: Vec<String> = group
                        .daemons
                        .iter()
                        .filter(|id| !config.daemons.contains_key(*id))
                        .map(|id| id.qualified())
                        .collect();
                    if !missing.is_empty() {
                        return Err(miette::miette!(
                            "group '{}' references undefined daemon{}: {}",
                            name,
                            if missing.len() > 1 { "s" } else { "" },
                            missing.join(", ")
                        ));
                    }
                    for daemon_id in &group.daemons {
                        if seen.insert(daemon_id.clone()) {
                            ids.push(daemon_id.clone());
                        }
                    }
                }
                None => {
                    let suggestion =
                        find_similar_daemon(name, config.groups.keys().map(|s| s.as_str()));
                    return Err(miette::miette!(
                        "group '{}' not found in configuration{}",
                        name,
                        suggestion.map(|s| format!(", {s}")).unwrap_or_default()
                    ));
                }
            }
        }

        Ok(ids)
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
    /// 3. Project-level: .config/pitchfork.toml, .config/pitchfork.local.toml, pitchfork.toml and pitchfork.local.toml files
    ///    from filesystem root to the given directory
    ///
    /// Within each directory, .config/ comes before pitchfork.toml,
    /// which comes before pitchfork.local.toml, so local.toml values override base config.
    pub fn list_paths_from(cwd: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        paths.push(env::PITCHFORK_GLOBAL_CONFIG_SYSTEM.clone());
        paths.push(env::PITCHFORK_GLOBAL_CONFIG_USER.clone());

        // Find all project config files. Order is reversed so after .reverse():
        // - each directory has: .config/pitchfork.toml < .config/pitchfork.local.toml < pitchfork.toml < pitchfork.local.toml
        // - directories go from root to cwd (later configs override earlier)
        let mut project_paths = xx::file::find_up_all(
            cwd,
            &[
                "pitchfork.local.toml",
                "pitchfork.toml",
                ".config/pitchfork.local.toml",
                ".config/pitchfork.toml",
            ],
        );
        project_paths.reverse();
        paths.extend(project_paths);

        paths
    }

    /// Merge all configuration files from the current working directory.
    /// See `all_merged_from` for details.
    pub fn all_merged() -> Result<PitchforkToml> {
        Self::all_merged_from(&env::CWD)
    }
    /// Load all merged config including daemons from ALL registered namespaces.
    ///
    /// Unlike `all_merged_from` which only merges configs from the cwd chain,
    /// this also iterates all `[namespaces]` entries and loads their daemon configs.
    /// Use this when you need a complete view (e.g. `start` for a daemon from
    /// another namespace).
    pub fn all_merged_all_namespaces() -> Result<Self> {
        let mut pt = Self::all_merged_from(&env::CWD)?;

        let namespaces = Self::read_global_namespaces();
        for (ns_name, entry) in namespaces {
            match Self::all_merged_from(&entry.dir) {
                Ok(ns_config) => {
                    for (daemon_id, daemon_config) in ns_config.daemons {
                        if !pt.daemons.contains_key(&daemon_id) {
                            pt.daemons.insert(daemon_id, daemon_config);
                        }
                    }
                    // Merge namespace-level settings so daemon-local
                    // overrides (e.g. hooks, env defaults) are available.
                    pt.settings.merge_from(&ns_config.settings);
                }
                Err(e) => {
                    log::warn!(
                        "Failed to load namespace '{ns_name}' from {}: {e}",
                        entry.dir.display()
                    );
                }
            }
        }

        Ok(pt)
    }

    /// Merge all configuration files starting from a given directory.
    ///
    /// Reads and merges configuration files in precedence order.
    /// Each daemon ID is qualified with a namespace based on its config file location:
    /// - Global configs (`~/.config/pitchfork/config.toml`) use namespace "global"
    /// - Project configs use the parent directory name as namespace
    ///
    /// This prevents ID conflicts when multiple projects define daemons with the same name.
    ///
    /// # Errors
    /// Returns an error if any config file fails to parse. Aborts with an error
    /// if two *different* project config files produce the same namespace (e.g. two
    /// `pitchfork.toml` files in separate directories that share the same directory name).
    pub fn all_merged_from(cwd: &Path) -> Result<PitchforkToml> {
        use std::collections::HashMap;

        let paths = Self::list_paths_from(cwd);
        let mut ns_to_origin: HashMap<String, (PathBuf, PathBuf)> = HashMap::new();

        let mut pt = Self::default();
        for p in paths {
            match Self::read(&p) {
                Ok(pt2) => {
                    // Detect collisions for all existing project configs, including
                    // pitchfork.local.toml. Allow sibling base/local files in the same
                    // directory to share a namespace, including siblings via .config subfolder
                    if p.exists() && !is_global_config(&p) {
                        let ns = namespace_from_path(&p)?;
                        let origin_dir = if is_dot_config_pitchfork(&p) {
                            p.parent().and_then(|d| d.parent())
                        } else {
                            p.parent()
                        }
                        .map(|dir| dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()))
                        .unwrap_or_else(|| p.clone());

                        if let Some((other_path, other_dir)) = ns_to_origin.get(ns.as_str())
                            && *other_dir != origin_dir
                        {
                            return Err(crate::error::ConfigParseError::NamespaceCollision {
                                path_a: other_path.clone(),
                                path_b: p.clone(),
                                ns,
                            }
                            .into());
                        }
                        ns_to_origin.insert(ns, (p.clone(), origin_dir));
                    }

                    pt.merge(pt2)
                }
                Err(e) => return Err(e.wrap_err(format!("error reading {}", p.display()))),
            }
        }
        Ok(pt)
    }
}

impl PitchforkToml {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            namespace: None,
            settings: SettingsPartial::default(),
            slugs: IndexMap::new(),
            groups: IndexMap::new(),
            namespaces: IndexMap::new(),
            path: Some(path),
        }
    }

    /// Parse TOML content as a [`PitchforkToml`] without touching the filesystem.
    ///
    /// Applies the same namespace derivation and daemon validation as [`read()`] but
    /// uses the provided `content` directly instead of reading from disk.  `path` is
    /// used only for namespace derivation and error messages.
    ///
    /// This is useful for validating user-edited content before saving it.
    pub fn parse_str(content: &str, path: &Path) -> Result<Self> {
        let raw_config: PitchforkTomlRaw = toml::from_str(content)
            .map_err(|e| ConfigParseError::from_toml_error(path, content.to_string(), e))?;

        let namespace = {
            let base_explicit = sibling_base_config(path)
                .and_then(|p| if p.exists() { Some(p) } else { None })
                .map(|p| read_namespace_override_from_file(&p))
                .transpose()?
                .flatten();

            if is_local_config(path)
                && let (Some(local_ns), Some(base_ns)) =
                    (raw_config.namespace.as_deref(), base_explicit.as_deref())
                && local_ns != base_ns
            {
                return Err(ConfigParseError::InvalidNamespace {
                    path: path.to_path_buf(),
                    namespace: local_ns.to_string(),
                    reason: format!(
                        "namespace '{local_ns}' does not match sibling pitchfork.toml namespace '{base_ns}'"
                    ),
                }
                .into());
            }

            let explicit = raw_config.namespace.as_deref().or(base_explicit.as_deref());
            namespace_from_path_with_override(path, explicit)?
        };
        let mut pt = Self::new(path.to_path_buf());
        pt.namespace = raw_config.namespace.clone();

        for (short_name, raw_daemon) in raw_config.daemons {
            let id = match DaemonId::try_new(&namespace, &short_name) {
                Ok(id) => id,
                Err(e) => {
                    return Err(ConfigParseError::InvalidDaemonName {
                        name: short_name,
                        path: path.to_path_buf(),
                        reason: e.to_string(),
                    }
                    .into());
                }
            };

            let mut depends = Vec::new();
            for dep in raw_daemon.depends {
                let dep_id = if dep.contains('/') {
                    match DaemonId::parse(&dep) {
                        Ok(id) => id,
                        Err(e) => {
                            return Err(ConfigParseError::InvalidDependency {
                                daemon: short_name.clone(),
                                dependency: dep,
                                path: path.to_path_buf(),
                                reason: e.to_string(),
                            }
                            .into());
                        }
                    }
                } else {
                    match DaemonId::try_new(&namespace, &dep) {
                        Ok(id) => id,
                        Err(e) => {
                            return Err(ConfigParseError::InvalidDependency {
                                daemon: short_name.clone(),
                                dependency: dep,
                                path: path.to_path_buf(),
                                reason: e.to_string(),
                            }
                            .into());
                        }
                    }
                };
                depends.push(dep_id);
            }

            // Resolve port config: prefer new `port` field, fall back to deprecated fields
            let has_deprecated = !raw_daemon.expected_port.is_empty()
                || raw_daemon.auto_bump_port.is_some()
                || raw_daemon.port_bump_attempts.is_some();
            let port = if let Some(port) = raw_daemon.port {
                if has_deprecated {
                    warn!(
                        "daemon {short_name}: both `port` and deprecated expected_port/auto_bump_port/port_bump_attempts are set; ignoring deprecated fields"
                    );
                }
                Some(port)
            } else if has_deprecated {
                warn!(
                    "daemon {short_name}: expected_port/auto_bump_port/port_bump_attempts are deprecated, use [daemons.{short_name}.port] instead"
                );
                let bump = if raw_daemon.auto_bump_port.unwrap_or(false) {
                    PortBump(
                        raw_daemon
                            .port_bump_attempts
                            .unwrap_or_else(|| settings().default_port_bump_attempts()),
                    )
                } else {
                    PortBump(0)
                };
                Some(PortConfig {
                    expect: raw_daemon.expected_port,
                    bump,
                })
            } else {
                None
            };

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
                port,
                boot_start: raw_daemon.boot_start,
                depends,
                watch: raw_daemon.watch,
                watch_mode: raw_daemon.watch_mode.unwrap_or_default(),
                dir: raw_daemon.dir,
                env: raw_daemon.env,
                hooks: raw_daemon.hooks,
                mise: raw_daemon.mise,
                user: raw_daemon.user,
                memory_limit: raw_daemon.memory_limit,
                cpu_limit: raw_daemon.cpu_limit,
                stop_signal: raw_daemon.stop_signal,
                pty: raw_daemon.pty,
                time_retention: raw_daemon.time_retention,
                line_retention: raw_daemon.line_retention,
                archive_hook: raw_daemon.archive_hook,
                logs: raw_daemon.logs,
                path: Some(path.to_path_buf()),
            };
            pt.daemons.insert(id, daemon);
        }

        // Copy settings if present
        if let Some(settings) = raw_config.settings {
            pt.settings = settings;
        }

        // Copy slugs registry (only meaningful in global config files)
        for (slug, entry) in raw_config.slugs {
            pt.slugs.insert(
                slug,
                SlugEntry {
                    dir: entry.dir.map(PathBuf::from),
                    namespace: entry.namespace,
                    daemon: entry.daemon,
                },
            );
        }

        // Copy namespaces registry (only meaningful in global config files)
        for (name, entry) in raw_config.namespaces {
            pt.namespaces.insert(
                name,
                NamespaceEntry {
                    dir: PathBuf::from(entry.dir),
                },
            );
        }

        // Resolve group entries: convert short daemon names to qualified DaemonIds
        for (group_name, raw_group) in raw_config.groups {
            let mut daemons = Vec::new();
            for daemon_name in &raw_group.daemons {
                let id = if daemon_name.contains('/') {
                    DaemonId::parse(daemon_name).map_err(|e| {
                        ConfigParseError::InvalidDependency {
                            daemon: group_name.clone(),
                            dependency: daemon_name.clone(),
                            path: path.to_path_buf(),
                            reason: e.to_string(),
                        }
                    })?
                } else {
                    DaemonId::try_new(&namespace, daemon_name).map_err(|e| {
                        ConfigParseError::InvalidDaemonName {
                            name: daemon_name.clone(),
                            path: path.to_path_buf(),
                            reason: e.to_string(),
                        }
                    })?
                };
                daemons.push(id);
            }
            pt.groups.insert(group_name, GroupEntry { daemons });
        }

        Ok(pt)
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
        Self::parse_str(&raw, path)
    }

    pub fn write(&self) -> Result<()> {
        if let Some(path) = &self.path {
            let _lock = xx::fslock::get(path, false)
                .wrap_err_with(|| format!("failed to acquire lock on {}", path.display()))?;
            self.write_unlocked()
        } else {
            Err(FileError::NoPath.into())
        }
    }

    /// Write the config file without acquiring a file lock.
    ///
    /// The caller MUST hold the file lock (via `xx::fslock::get`) before
    /// calling this method. This is used by `register_slug` which needs to
    /// hold a single lock across a read-modify-write cycle.
    fn write_unlocked(&self) -> Result<()> {
        if let Some(path) = &self.path {
            // Determine the namespace for this config file
            let config_namespace = if path.exists() {
                namespace_from_path(path)?
            } else {
                namespace_from_path_with_override(path, self.namespace.as_deref())?
            };

            // Convert back to raw format for writing (use short names as keys)
            // Preserve settings so read-modify-write (e.g. `settings set`, `proxy add`)
            // doesn't drop `[settings.*]`. Gate on is_empty to avoid a bare `[settings]`.
            let mut raw = PitchforkTomlRaw {
                namespace: self.namespace.clone(),
                settings: (!self.settings.is_empty()).then(|| self.settings.clone()),
                ..PitchforkTomlRaw::default()
            };
            for (id, daemon) in &self.daemons {
                if id.namespace() != config_namespace {
                    return Err(miette::miette!(
                        "cannot write daemon '{}' to {}: daemon belongs to namespace '{}' but file namespace is '{}'",
                        id,
                        path.display(),
                        id.namespace(),
                        config_namespace
                    ));
                }
                let port = daemon.port.as_ref();
                let raw_daemon = PitchforkTomlDaemonRaw {
                    run: daemon.run.clone(),
                    auto: daemon.auto.clone(),
                    cron: daemon.cron.clone(),
                    retry: daemon.retry,
                    ready_delay: daemon.ready_delay,
                    ready_output: daemon.ready_output.clone(),
                    ready_http: daemon.ready_http.clone(),
                    ready_port: daemon.ready_port.clone(),
                    ready_cmd: daemon.ready_cmd.clone(),
                    port: port.cloned(),
                    // Deprecated fields: written for backward compatibility with older pitchfork versions
                    expected_port: port.map(|p| p.expect.clone()).unwrap_or_default(),
                    auto_bump_port: port.filter(|p| p.auto_bump()).map(|_| true),
                    port_bump_attempts: port
                        .filter(|p| p.auto_bump())
                        .map(|p| p.max_bump_attempts()),
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
                    watch_mode: match daemon.watch_mode {
                        WatchMode::Native => None,
                        mode => Some(mode),
                    },
                    dir: daemon.dir.clone(),
                    env: daemon.env.clone(),
                    hooks: daemon.hooks.clone(),
                    mise: daemon.mise,
                    user: daemon.user.clone(),
                    memory_limit: daemon.memory_limit,
                    cpu_limit: daemon.cpu_limit,
                    stop_signal: daemon.stop_signal,
                    pty: daemon.pty,
                    time_retention: daemon.time_retention.clone(),
                    line_retention: daemon.line_retention,
                    archive_hook: daemon.archive_hook.clone(),
                    logs: daemon.logs.clone(),
                };
                raw.daemons.insert(id.name().to_string(), raw_daemon);
            }

            // Copy slugs registry to raw format
            for (slug, entry) in &self.slugs {
                raw.slugs.insert(
                    slug.clone(),
                    SlugEntryRaw {
                        dir: entry.dir.as_ref().map(|d| d.to_string_lossy().to_string()),
                        namespace: entry.namespace.clone(),
                        daemon: entry.daemon.clone(),
                    },
                );
            }

            // Serialize groups back to raw format (preserve cross-namespace refs as qualified IDs)
            for (name, group) in &self.groups {
                let raw_daemons: Vec<String> = group
                    .daemons
                    .iter()
                    .map(|id| {
                        if id.namespace() == config_namespace {
                            id.name().to_string()
                        } else {
                            id.qualified()
                        }
                    })
                    .collect();
                raw.groups.insert(
                    name.clone(),
                    GroupEntryRaw {
                        daemons: raw_daemons,
                    },
                );
            }

            // Copy namespaces registry to raw format
            for (name, entry) in &self.namespaces {
                raw.namespaces.insert(
                    name.clone(),
                    NamespaceEntryRaw {
                        dir: entry.dir.to_string_lossy().to_string(),
                    },
                );
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
        // Merge slugs - pt's values override self's values
        for (slug, entry) in pt.slugs {
            self.slugs.insert(slug, entry);
        }
        // Merge groups - pt's values override self's values
        for (name, group) in pt.groups {
            self.groups.insert(name, group);
        }
        // Merge namespaces - pt's values override self's values
        for (name, entry) in pt.namespaces {
            self.namespaces.insert(name, entry);
        }
        // Merge settings - pt's values override self's values
        self.settings.merge_from(&pt.settings);
    }

    /// Read the global slug registry from the user-level global config.
    ///
    /// Returns a map of slug → SlugEntry from `[slugs]` in
    /// `~/.config/pitchfork/config.toml`.
    pub fn read_global_slugs() -> IndexMap<String, SlugEntry> {
        match Self::read(&*env::PITCHFORK_GLOBAL_CONFIG_USER) {
            Ok(pt) => pt.slugs,
            Err(_) => IndexMap::new(),
        }
    }

    /// Find the registered slug for a daemon using a pre-loaded slug registry.
    pub fn find_slug_for_daemon_in_registry(
        daemon_id: &DaemonId,
        global_slugs: &IndexMap<String, SlugEntry>,
    ) -> Option<String> {
        global_slugs
            .iter()
            .find(|(slug, entry)| {
                let daemon_name = entry.daemon.as_deref().unwrap_or(slug);
                if daemon_id.name() != daemon_name {
                    return false;
                }

                match entry.resolve_namespace() {
                    Some(namespace) => daemon_id.namespace() == namespace,
                    None => false,
                }
            })
            .map(|(slug, _)| slug.clone())
    }

    /// Check if a slug is registered in the global config's `[slugs]` section.
    #[allow(dead_code)]
    pub fn is_slug_registered(slug: &str) -> bool {
        Self::read_global_slugs().contains_key(slug)
    }

    /// Add a slug entry to the global config's `[slugs]` section using namespace instead of dir.
    ///
    /// Reads the global config, adds/updates the slug entry, and writes it back.
    /// If `namespace` is provided but not yet registered in `[namespaces]`,
    /// also registers it at `dir` (acquired via `resolve_dir()` on the slug entry).
    pub fn add_slug_with_namespace(
        slug: &str,
        namespace: Option<&str>,
        daemon: Option<&str>,
    ) -> Result<()> {
        let global_path = &*env::PITCHFORK_GLOBAL_CONFIG_USER;

        // Ensure the config directory exists
        if let Some(parent) = global_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                miette::miette!(
                    "Failed to create config directory {}: {e}",
                    parent.display()
                )
            })?;
        }

        let _lock = xx::fslock::get(global_path, false)
            .wrap_err_with(|| format!("failed to acquire lock on {}", global_path.display()))?;

        let mut pt = if global_path.exists() {
            let raw = std::fs::read_to_string(global_path).map_err(|e| FileError::ReadError {
                path: global_path.to_path_buf(),
                source: e,
            })?;
            Self::parse_str(&raw, global_path)?
        } else {
            Self::new(global_path.to_path_buf())
        };

        // If caller provided a namespace that isn't yet registered,
        // auto-register it at the directory we can resolve.
        // Falls back to CWD if the slug dir cannot be resolved.
        if let Some(ns) = namespace {
            if !pt.namespaces.contains_key(ns) {
                let dir = pt
                    .slugs
                    .get(slug)
                    .and_then(|e| e.resolve_dir())
                    .or_else(|| namespace.and_then(|_| env::CWD.as_path().canonicalize().ok()));
                if let Some(ref d) = dir {
                    pt.namespaces
                        .insert(ns.to_string(), NamespaceEntry { dir: d.clone() });
                }
            }
        }

        pt.slugs.insert(
            slug.to_string(),
            SlugEntry {
                dir: None,
                namespace: namespace.map(str::to_string),
                daemon: daemon.map(str::to_string),
            },
        );
        pt.write_unlocked()?;
        crate::proxy::hosts::sync_hosts_from_settings();
        Ok(())
    }

    /// Remove a slug from the global config's `[slugs]` section.
    pub fn remove_slug(slug: &str) -> Result<bool> {
        let global_path = &*env::PITCHFORK_GLOBAL_CONFIG_USER;
        if !global_path.exists() {
            return Ok(false);
        }

        let _lock = xx::fslock::get(global_path, false)
            .wrap_err_with(|| format!("failed to acquire lock on {}", global_path.display()))?;

        let raw = std::fs::read_to_string(global_path).map_err(|e| FileError::ReadError {
            path: global_path.to_path_buf(),
            source: e,
        })?;
        let mut pt = Self::parse_str(&raw, global_path)?;

        let removed = pt.slugs.shift_remove(slug).is_some();
        if removed {
            pt.write_unlocked()?;
            crate::proxy::hosts::sync_hosts_from_settings();
        }
        Ok(removed)
    }
    /// Returns a map of namespace → NamespaceEntry from `[namespaces]` in
    /// `~/.config/pitchfork/config.toml`.
    pub fn read_global_namespaces() -> IndexMap<String, NamespaceEntry> {
        match Self::read(&*env::PITCHFORK_GLOBAL_CONFIG_USER) {
            Ok(pt) => pt.namespaces,
            Err(_) => IndexMap::new(),
        }
    }

    /// Add a namespace entry to the global config's `[namespaces]` section.
    ///
    /// Reads the global config, adds/updates the namespace entry, and writes it back.
    pub fn register_namespace(name: &str, dir: &str) -> crate::Result<()> {
        let global_path = &*crate::env::PITCHFORK_GLOBAL_CONFIG_USER;

        // Ensure the config directory exists
        if let Some(parent) = global_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                miette::miette!(
                    "Failed to create config directory {}: {e}",
                    parent.display()
                )
            })?;
        }

        let _lock = xx::fslock::get(global_path, false)
            .wrap_err_with(|| format!("failed to acquire lock on {}", global_path.display()))?;

        let mut pt = if global_path.exists() {
            let raw = std::fs::read_to_string(global_path).map_err(|e| {
                crate::error::FileError::ReadError {
                    path: global_path.to_path_buf(),
                    source: e,
                }
            })?;
            Self::parse_str(&raw, global_path)?
        } else {
            Self::new(global_path.to_path_buf())
        };

        pt.namespaces.insert(
            name.to_string(),
            NamespaceEntry {
                dir: PathBuf::from(dir),
            },
        );
        pt.write_unlocked()?;
        Ok(())
    }

    /// Remove a namespace from the global config's `[namespaces]` section.
    pub fn remove_namespace(name: &str) -> crate::Result<bool> {
        let global_path = &*crate::env::PITCHFORK_GLOBAL_CONFIG_USER;
        if !global_path.exists() {
            return Ok(false);
        }

        let _lock = xx::fslock::get(global_path, false)
            .wrap_err_with(|| format!("failed to acquire lock on {}", global_path.display()))?;

        let raw = std::fs::read_to_string(global_path).map_err(|e| {
            crate::error::FileError::ReadError {
                path: global_path.to_path_buf(),
                source: e,
            }
        })?;
        let mut pt = Self::parse_str(&raw, global_path)?;

        let removed = pt.namespaces.shift_remove(name).is_some();
        if removed {
            pt.write_unlocked()?;
        }
        Ok(removed)
    }
}

/// Configuration for a single daemon (internal representation with DaemonId)
#[derive(Debug, Clone, JsonSchema, Default)]
pub struct PitchforkTomlDaemon {
    /// The command to run. Prepend with 'exec' to avoid shell process overhead.
    #[schemars(example = example_run_command())]
    pub run: String,
    /// Automatic start/stop behavior based on shell hooks
    #[schemars(default)]
    pub auto: Vec<PitchforkTomlAuto>,
    /// Cron scheduling configuration for periodic execution
    pub cron: Option<PitchforkTomlCron>,
    /// Number of times to retry if the daemon fails.
    /// Can be a number (e.g., `3`) or `true` for infinite retries.
    #[schemars(default)]
    pub retry: Retry,
    /// Delay in seconds before considering the daemon ready
    pub ready_delay: Option<u64>,
    /// Regex pattern to match in ANSI-stripped stdout/stderr to determine readiness
    pub ready_output: Option<String>,
    /// HTTP URL to poll for readiness. Accepts any 2xx response by default, or configured statuses.
    pub ready_http: Option<ReadyHttp>,
    /// TCP port to check for readiness (connection success = ready).
    /// Accepts a port number or a Tera template string that renders to one.
    pub ready_port: Option<ReadyPort>,
    /// Shell command to poll for readiness (exit code 0 = ready)
    pub ready_cmd: Option<String>,
    /// Port configuration: expected ports and auto-bump settings
    pub port: Option<PortConfig>,
    /// Whether to start this daemon automatically on system boot
    pub boot_start: Option<bool>,
    /// List of daemon IDs that must be started before this one
    #[schemars(default)]
    pub depends: Vec<DaemonId>,
    /// File patterns to watch for changes
    #[schemars(default)]
    pub watch: Vec<String>,
    /// File watching backend mode.
    ///
    /// - `native`: use platform-native notifications (default)
    /// - `poll`: use polling-based watcher
    /// - `auto`: prefer native, fall back to polling if native watch fails
    #[schemars(default)]
    pub watch_mode: WatchMode,
    /// Working directory for the daemon. Relative paths are resolved from the pitchfork.toml location.
    pub dir: Option<String>,
    /// Environment variables to set for the daemon process
    pub env: Option<IndexMap<String, String>>,
    /// Lifecycle hooks (on_ready, on_fail, on_retry)
    pub hooks: Option<PitchforkTomlHooks>,
    /// Wrap this daemon's command with `mise x --` for tool/env setup.
    /// Overrides the global `settings.general.mise` when set.
    pub mise: Option<bool>,
    /// Unix user to run this daemon as. Overrides `settings.supervisor.user` when set.
    pub user: Option<String>,
    /// Memory limit for the daemon process (e.g. "50MB", "1GiB").
    /// The supervisor periodically monitors RSS and kills the process if it exceeds the limit.
    pub memory_limit: Option<MemoryLimit>,
    /// CPU usage limit as a percentage (e.g. 80 for 80%, 200 for 2 cores).
    /// The supervisor periodically monitors CPU usage and kills the process if it exceeds the limit.
    pub cpu_limit: Option<CpuLimit>,
    /// Stop signal and optional per-daemon timeout. Accepts a signal name string
    /// or `{ signal = "...", timeout = "..." }` object.
    pub stop_signal: Option<StopConfig>,
    /// Allocate a pseudo-terminal for the daemon process.
    pub pty: Option<bool>,
    /// Maximum age of log entries to keep (e.g. "7d", "30d").
    /// Overrides the global `settings.logs.time_retention` when set.
    pub time_retention: Option<String>,
    /// Maximum number of log entries to keep per daemon.
    /// Overrides the global `settings.logs.line_retention` when set.
    pub line_retention: Option<i64>,
    /// Archive hook command invoked before retention prunes this daemon's logs.
    /// Overrides the global `settings.logs.archive_hook.command` when set.
    pub archive_hook: Option<String>,
    /// Per-daemon log configuration sub-table.
    pub logs: Option<PitchforkTomlDaemonLogs>,
    #[schemars(skip)]
    pub path: Option<PathBuf>,
}

impl PitchforkTomlDaemon {
    /// Build RunOptions from this daemon configuration.
    ///
    /// Carries over all config fields and resolves the working directory.
    /// Callers can override specific fields on the returned value.
    pub fn to_run_options(
        &self,
        id: &crate::daemon_id::DaemonId,
        cmd: Vec<String>,
    ) -> crate::daemon::RunOptions {
        use crate::daemon::RunOptions;

        let dir = crate::ipc::batch::resolve_daemon_dir(self.dir.as_deref(), self.path.as_deref());
        let slug = crate::pitchfork_toml::PitchforkToml::read_global_slugs()
            .into_iter()
            .find(|(slug, entry)| {
                let daemon_name = entry.daemon.as_deref().unwrap_or(slug);
                if daemon_name != id.name() {
                    return false;
                }

                match entry.resolve_namespace() {
                    Some(namespace) => namespace == id.namespace(),
                    None => false,
                }
            })
            .map(|(slug, _)| slug);

        RunOptions {
            id: id.clone(),
            cmd,
            run: Some(self.run.clone()),
            force: false,
            shell_pid: None,
            dir: Dir(dir),
            autostop: self.auto.contains(&PitchforkTomlAuto::Stop),
            cron_schedule: self.cron.as_ref().map(|c| c.schedule.clone()),
            cron_retrigger: self.cron.as_ref().map(|c| c.retrigger),
            cron_immediate: self.cron.as_ref().map(|c| c.immediate),
            retry: self.retry,
            retry_count: 0,
            ready_delay: self.ready_delay,
            ready_output: self.ready_output.clone(),
            ready_http: self.ready_http.clone(),
            // Templates are resolved to a literal port by render_daemon_templates
            // on the start path; an unrendered template has no usable port.
            ready_port: self.ready_port.as_ref().and_then(|rp| {
                let port = rp.as_port();
                if port.is_none() {
                    warn!(
                        "daemon {id}: ready_port template {rp:?} was not rendered on this start path; skipping port readiness check"
                    );
                }
                port
            }),
            ready_cmd: self.ready_cmd.clone(),
            port: self.port.clone(),
            wait_ready: false,
            depends: self.depends.clone(),
            env: self.env.clone(),
            watch: self.watch.clone(),
            watch_mode: self.watch_mode,
            watch_base_dir: Some(crate::ipc::batch::resolve_config_base_dir(
                self.path.as_deref(),
            )),
            mise: self.mise,
            slug,
            proxy: None,
            user: self.user.clone(),
            memory_limit: self.memory_limit,
            cpu_limit: self.cpu_limit,
            stop_signal: self.stop_signal,
            archive_hook: self
                .logs
                .as_ref()
                .and_then(|l| l.archive_hook.clone())
                .or_else(|| self.archive_hook.clone()),
            log_format: self.logs.as_ref().and_then(|l| l.log_format.clone()),
            on_output_hook: self.hooks.as_ref().and_then(|h| h.on_output.clone()),
            pty: self.pty,
        }
    }
}
fn example_run_command() -> &'static str {
    "exec node server.js"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_daemon_user_parses_and_flows_to_run_options() {
        let pt = PitchforkToml::parse_str(
            r#"
[daemons.api]
run = "node server.js"
user = "postgres"
"#,
            Path::new("/tmp/my-project/pitchfork.toml"),
        )
        .unwrap();

        let id = DaemonId::new("my-project", "api");
        let daemon = pt.daemons.get(&id).unwrap();
        assert_eq!(daemon.user.as_deref(), Some("postgres"));

        let opts = daemon.to_run_options(&id, vec!["node".to_string(), "server.js".to_string()]);
        assert_eq!(opts.user.as_deref(), Some("postgres"));
    }

    #[test]
    fn test_daemon_user_write_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pitchfork.toml");
        let mut pt = PitchforkToml::new(path.clone());
        pt.namespace = Some("test-project".to_string());
        pt.daemons.insert(
            DaemonId::new("test-project", "api"),
            PitchforkTomlDaemon {
                run: "node server.js".to_string(),
                user: Some("postgres".to_string()),
                ..PitchforkTomlDaemon::default()
            },
        );

        pt.write().unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("user = \"postgres\""));

        let parsed = PitchforkToml::read(&path).unwrap();
        let daemon = parsed
            .daemons
            .get(&DaemonId::new("test-project", "api"))
            .unwrap();
        assert_eq!(daemon.user.as_deref(), Some("postgres"));
    }

    #[test]
    fn test_settings_write_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pitchfork.toml");
        let mut pt = PitchforkToml::new(path.clone());
        pt.namespace = Some("test-project".to_string());
        pt.settings.web.auto_start = Some(true);
        pt.settings.general.log_level = Some("debug".to_string());

        pt.write().unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("[settings.web]"),
            "settings.web section should be written, got:\n{raw}"
        );
        assert!(raw.contains("auto_start = true"));
        assert!(raw.contains("log_level = \"debug\""));

        let parsed = PitchforkToml::read(&path).unwrap();
        assert_eq!(parsed.settings.web.auto_start, Some(true));
        assert_eq!(parsed.settings.general.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn test_settings_preserved_on_unrelated_write() {
        // Regression test for https://github.com/jdx/pitchfork/discussions/574
        // A read-modify-write of slugs/namespaces must not drop existing [settings].
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pitchfork.toml");
        std::fs::write(&path, "[settings.web]\nauto_start = true\n").unwrap();

        let mut pt = PitchforkToml::read(&path).unwrap();
        pt.slugs.insert(
            "api".to_string(),
            SlugEntry {
                dir: None,
                namespace: Some("myproject".to_string()),
                daemon: None,
            },
        );
        pt.namespaces.insert(
            "myproject".to_string(),
            NamespaceEntry {
                dir: PathBuf::from("/tmp/myproject"),
            },
        );
        pt.write().unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("[settings.web]"),
            "existing settings must be preserved, got:\n{raw}"
        );
        assert!(raw.contains("auto_start = true"));
        assert!(raw.contains("[slugs.api]"));

        let parsed = PitchforkToml::read(&path).unwrap();
        assert_eq!(parsed.settings.web.auto_start, Some(true));
        assert!(parsed.slugs.contains_key("api"));
    }
}
