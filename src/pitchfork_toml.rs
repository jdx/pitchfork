use crate::daemon_id::DaemonId;
use crate::error::{ConfigParseError, DependencyError, FileError, find_similar_daemon};
use crate::settings::SettingsPartial;
use crate::settings::settings;
use crate::state_file::StateFile;
use crate::{Result, env};
use indexmap::IndexMap;
use miette::Context;
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::SystemTime;

// Re-export config value types so existing `use crate::pitchfork_toml::X` paths keep working.
pub use crate::config_types::{
    CpuLimit, CronRetrigger, Dir, MemoryLimit, OnOutputHook, PitchforkTomlAuto, PitchforkTomlCron,
    PitchforkTomlHooks, PortBump, PortConfig, ReadyCmd, ReadyHttp, ReadyOutput, ReadyPort, Retry,
    StopConfig, StopSignal, WatchMode,
};

/// Raw slug entry as read from TOML (uses String for dir path).
/// Format in global config:
/// ```toml
/// [slugs]
/// api = { dir = "/home/user/my-api", daemon = "server" }
/// docs = { dir = "/home/user/docs-site" }  # daemon defaults to slug name
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct GroupEntryRaw {
    #[schemars(with = "Vec<DaemonId>")]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
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
    /// Top-level environment variables applied to all daemons as defaults.
    /// Per-daemon `env` overrides these. Values support Tera templates.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
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
    pub ready_output: Option<ReadyOutput>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_http: Option<ReadyHttp>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_port: Option<ReadyPort>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_cmd: Option<ReadyCmd>,
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
#[derive(Debug, Clone, Default, JsonSchema)]
#[schemars(title = "Pitchfork Configuration")]
pub struct PitchforkToml {
    /// Map of daemon IDs to their configurations
    #[serde(default)]
    pub daemons: IndexMap<DaemonId, PitchforkTomlDaemon>,
    /// Top-level environment variables applied to all daemons as defaults.
    /// Per-daemon `env` overrides these on key conflicts. Values support Tera
    /// templates (e.g. `{{ daemons.api.port }}`, `{{ settings.proxy.tld }}`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
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
    #[schemars(default, with = "IndexMap<String, SlugEntryRaw>")]
    pub slugs: IndexMap<String, SlugEntry>,
    /// Named groups of daemons for batch operations.
    #[schemars(default, with = "IndexMap<String, GroupEntryRaw>")]
    pub groups: IndexMap<String, GroupEntry>,
    /// Namespace registry (merged from global config files).
    /// Maps namespace names to their project directory.
    #[schemars(default, with = "IndexMap<String, NamespaceEntryRaw>")]
    pub namespaces: IndexMap<String, NamespaceEntry>,
    #[schemars(skip)]
    pub path: Option<PathBuf>,
}

pub(crate) fn is_global_config(path: &Path) -> bool {
    path == *env::PITCHFORK_GLOBAL_CONFIG_USER || path == *env::PITCHFORK_GLOBAL_CONFIG_SYSTEM
}

pub(crate) fn is_dot_config_pitchfork(path: &Path) -> bool {
    path.ends_with(".config/pitchfork.toml") || path.ends_with(".config/pitchfork.local.toml")
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

fn project_config_dir(path: &Path) -> Option<&Path> {
    if is_dot_config_pitchfork(path) {
        path.parent().and_then(Path::parent)
    } else {
        path.parent()
    }
}

fn project_config_family(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = project_config_dir(path) else {
        return vec![path.to_path_buf()];
    };
    vec![
        dir.join(".config/pitchfork.toml"),
        dir.join(".config/pitchfork.local.toml"),
        dir.join("pitchfork.toml"),
        dir.join("pitchfork.local.toml"),
    ]
}

/// Resolve one namespace override shared by all project config files in a
/// directory. `content_override` represents unsaved content for `path`.
fn directory_namespace_override(
    path: &Path,
    content_override: Option<&str>,
) -> Result<Option<String>> {
    if is_global_config(path) {
        return match content_override {
            Some(content) => parse_namespace_override_from_content(path, content),
            None => read_namespace_override_from_file(path),
        };
    }

    let mut selected: Option<(String, PathBuf)> = None;
    for candidate in project_config_family(path) {
        let explicit = if candidate == path {
            match content_override {
                Some(content) => parse_namespace_override_from_content(&candidate, content)?,
                None => read_namespace_override_from_file(&candidate)?,
            }
        } else {
            read_namespace_override_from_file(&candidate)?
        };
        let Some(namespace) = explicit else { continue };
        if let Some((selected_namespace, selected_path)) = &selected
            && selected_namespace != &namespace
        {
            return Err(ConfigParseError::InvalidNamespace {
                path: candidate,
                namespace,
                reason: format!(
                    "namespace does not match directory-level namespace '{}' declared in {}",
                    selected_namespace,
                    selected_path.display()
                ),
            }
            .into());
        }
        selected = Some((namespace, candidate));
    }
    Ok(selected.map(|(namespace, _)| namespace))
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
    let explicit = directory_namespace_override(path, None)?;
    namespace_from_path_with_override(path, explicit.as_deref())
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

/// Find the nearest ancestor directory of `dir` that contains a `.git` or
/// `.jj` marker (the project root of a git worktree / jj workspace).
///
/// Returns `None` when `dir` is not inside any git/jj project, so callers can
/// fall back to non-worktree behavior. In a linked git worktree, `.git` is a
/// *file* (not a directory) pointing at the common gitdir, so we check
/// existence rather than `is_dir()`.
fn find_project_root(dir: &Path) -> Option<PathBuf> {
    // Canonicalize the start dir so a symlinked path resolves into the
    // repository hierarchy before traversing parents; otherwise `parent()`
    // walks outside the repo and misses `.git`/`.jj`.
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut current = canonical_dir.as_path();
    loop {
        if current.join(".git").exists() || current.join(".jj").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// Cached result of `all_merged_from`, keyed by the cwd used to discover config paths.
///
/// The cache key is the canonical cwd `PathBuf`. The entry stores the merged
/// [`PitchforkToml`] plus a snapshot of every source file's (mtime, size) at
/// cache time. A cache hit requires the same set of paths with identical
/// mtimes **and** sizes.
///
/// Tracking size in addition to mtime catches timestamp-preserving content
/// changes (e.g. `cp --preserve=timestamps`, same-second edits on filesystems
/// with coarse mtime granularity like NFS) that mtime alone would miss.
///
/// **Known limitation**: an equal-size content replacement that also preserves
/// mtime will not be detected. This is an accepted trade-off — fully closing
/// this gap would require hashing file contents on every cache hit, negating
/// the I/O savings the cache exists to provide. In practice, editors and `git
/// pull` always change mtime, and `cp --preserve=timestamps` almost always
/// changes size. The `ReloadConfig` IPC handler (`settings reload`) serves as
/// an explicit escape hatch to force a full re-read when needed.
struct ConfigCacheEntry {
    config: PitchforkToml,
    /// (path, (mtime, size)) snapshot — order matches `list_paths_from` at cache time.
    source_meta: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
}

/// Global config parse cache, keyed by canonical cwd.
///
/// Uses a `std::sync::Mutex` (not tokio) because config parsing is CPU-bound
/// and callers like `spawn_blocking(PitchforkToml::all_merged)` run outside
/// the async runtime. Contention is minimal: the mutex is held only for the
/// metadata comparison and the occasional re-read, not across I/O.
static CONFIG_CACHE: Lazy<StdMutex<HashMap<PathBuf, ConfigCacheEntry>>> =
    Lazy::new(|| StdMutex::new(HashMap::new()));

/// Compare a list of paths' current (mtime, size) against a cached snapshot.
///
/// Returns `true` if every path exists (or not) and has the same mtime and
/// size as when the snapshot was taken. Any difference — a new file, a deleted
/// file, a changed mtime, or a changed size — invalidates the cache.
fn meta_matches(paths: &[PathBuf], snapshot: &[(PathBuf, Option<(SystemTime, u64)>)]) -> bool {
    if paths.len() != snapshot.len() {
        return false;
    }
    paths
        .iter()
        .zip(snapshot.iter())
        .all(|(p, (snap_p, snap_meta))| p == snap_p && current_meta(p) == *snap_meta)
}

/// Best-effort (mtime, size) — `None` if the path doesn't exist or metadata fails.
fn current_meta(path: &Path) -> Option<(SystemTime, u64)> {
    let md = std::fs::metadata(path).ok()?;
    Some((md.modified().ok()?, md.len()))
}

/// Snapshot all (path, (mtime, size)) pairs for a list of paths.
fn snapshot_meta(paths: &[PathBuf]) -> Vec<(PathBuf, Option<(SystemTime, u64)>)> {
    paths.iter().map(|p| (p.clone(), current_meta(p))).collect()
}

/// Invalidate the entire config parse cache.
///
/// Called after any config file write (`write()`, `write_unlocked()`,
/// `add_slug_with_namespace()`, `remove_slug()`, `register_namespace()`,
/// `remove_namespace()`) so that subsequent `all_merged_from` calls re-read
/// from disk.
///
/// Also called by `settings reload` (via IPC `ReloadConfig`) so that external
/// edits to config files are picked up.
///
/// **Cross-process note**: CLI and supervisor are separate processes with
/// independent caches. When the CLI calls this after `write_unlocked()`, it
/// only clears the CLI's own cache (which is about to exit anyway). The
/// supervisor relies on mtime change detection to pick up CLI-written changes.
/// The `ReloadConfig` IPC handler is the one that matters — it clears the
/// supervisor's cache.
pub fn invalidate_config_cache() {
    if let Ok(mut cache) = CONFIG_CACHE.lock() {
        cache.clear();
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
            if let Some(dir) = entry.resolve_dir()
                && let Ok(project_config) = Self::all_merged_from(&dir)
            {
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
            if let Some(dir) = entry.resolve_dir()
                && let Ok(project_config) = Self::all_merged_from(&dir)
            {
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

    /// Like `resolve_id`, but allows ad-hoc short IDs in the current directory's
    /// derived namespace.
    ///
    /// This is intended for commands such as `pitchfork run` that create
    /// managed daemons without requiring prior config entries.
    pub fn resolve_id_allow_adhoc(user_id: &str) -> Result<DaemonId> {
        Self::resolve_id_allow_adhoc_from(user_id, &env::CWD)
    }

    fn resolve_id_allow_adhoc_from(user_id: &str, dir: &Path) -> Result<DaemonId> {
        if user_id.contains('/') {
            return DaemonId::parse(user_id);
        }

        let ns = Self::namespace_for_dir(dir)?;
        DaemonId::try_new(ns, user_id)
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
        Self::all_merged_all_namespaces_from(&env::CWD)
    }

    /// Core of [`Self::all_merged_all_namespaces`], parameterized by the
    /// starting directory so it is testable without touching the global `CWD`.
    pub(crate) fn all_merged_all_namespaces_from(start_dir: &Path) -> Result<Self> {
        let mut pt = Self::all_merged_from(start_dir)?;

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

        // Auto-discover git worktrees / jj workspaces under the current
        // project, so daemons defined in a worktree are visible to supervisor
        // background tasks (cron registration, boot_start, file watch) even
        // when that worktree's namespace was never registered in
        // `[namespaces]`. Discovery spawns a `git`/`jj` subprocess (<1ms) and
        // the per-worktree config reads are cached by `CONFIG_CACHE`, so no
        // extra caching layer is needed here.
        if let Some(project_root) = find_project_root(start_dir) {
            let worktrees = crate::proxy::worktree::discover_worktrees(&project_root);
            for wt in &worktrees {
                match Self::all_merged_from(&wt.path) {
                    Ok(wt_config) => {
                        for (daemon_id, daemon_config) in wt_config.daemons {
                            if !pt.daemons.contains_key(&daemon_id) {
                                pt.daemons.insert(daemon_id, daemon_config);
                            }
                        }
                        pt.settings.merge_from(&wt_config.settings);
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to load worktree '{}' config from {}: {e}",
                            wt.branch,
                            wt.path.display()
                        );
                    }
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
    /// Results are cached by cwd and invalidated when any source file's mtime
    /// changes or when [`invalidate_config_cache`] is called (e.g. after a
    /// config write via `write()` / `write_unlocked()`).
    ///
    /// # Errors
    /// Returns an error if any config file fails to parse. Aborts with an error
    /// if two *different* project config files produce the same namespace (e.g. two
    /// `pitchfork.toml` files in separate directories that share the same directory name).
    pub fn all_merged_from(cwd: &Path) -> Result<PitchforkToml> {
        let paths = Self::list_paths_from(cwd);

        // Fast path: check the cache under a short-lived lock.
        // We canonicalize cwd for a stable key. If canonicalization fails
        // (e.g. the directory was just deleted), fall back to the raw path.
        let cache_key = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());

        {
            let cache = CONFIG_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = cache.get(&cache_key)
                && meta_matches(&paths, &entry.source_meta)
            {
                return Ok(entry.config.clone());
            }
        }

        // Cache miss: snapshot (mtime, size) BEFORE reading.
        // If a file changes during the read, the snapshot (old mtime/size) won't
        // match the current values on the next call, forcing a re-read.
        // Taking the snapshot after the read would store old content with new
        // metadata, serving stale data indefinitely.
        let snapshot = snapshot_meta(&paths);
        let pt = Self::all_merged_from_uncached(&paths)?;

        // Store in cache.
        let mut cache = CONFIG_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(
            cache_key,
            ConfigCacheEntry {
                config: pt.clone(),
                source_meta: snapshot,
            },
        );

        Ok(pt)
    }

    /// Uncached merge of all configuration files from a list of paths.
    ///
    /// This is the original merge logic extracted from `all_merged_from` so that
    /// the cache layer can wrap it without duplicating the algorithm.
    fn all_merged_from_uncached(paths: &[PathBuf]) -> Result<PitchforkToml> {
        use std::collections::HashMap as StdHashMap;

        let mut ns_to_origin: StdHashMap<String, (PathBuf, PathBuf)> = StdHashMap::new();

        let mut pt = Self::default();
        for p in paths {
            match Self::read(p) {
                Ok(pt2) => {
                    // Detect collisions for all existing project configs, including
                    // pitchfork.local.toml. Allow sibling base/local files in the same
                    // directory to share a namespace, including siblings via .config subfolder
                    if p.exists() && !is_global_config(p) {
                        let ns = namespace_from_path(p)?;
                        let origin_dir = if is_dot_config_pitchfork(p) {
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
            env: None,
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

        let explicit = directory_namespace_override(path, Some(content))?;
        let namespace = namespace_from_path_with_override(path, explicit.as_deref())?;
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

        // Copy top-level env
        pt.env = raw_config.env;

        // Copy slugs registry (only meaningful in global config files)
        for (slug, entry) in raw_config.slugs {
            pt.slugs.insert(
                slug,
                SlugEntry {
                    dir: entry.dir.map(env::expand_tilde),
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
                    dir: env::expand_tilde(entry.dir),
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
                env: self.env.clone(),
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
            invalidate_config_cache();
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
        // Merge top-level env - pt's values override self's values
        if let Some(env) = pt.env {
            let merged = self.env.get_or_insert_with(IndexMap::new);
            for (k, v) in env {
                merged.insert(k, v);
            }
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
        if let Some(ns) = namespace
            && !pt.namespaces.contains_key(ns)
        {
            // Resolve against the already-parsed `pt` instead of
            // SlugEntry::resolve_dir(): that re-reads the global config via
            // read(), which would re-acquire the lock held above (flock is per
            // open file description, so the same process deadlocks on itself).
            let dir = pt
                .slugs
                .get(slug)
                .and_then(|e| {
                    e.dir.clone().or_else(|| {
                        e.namespace
                            .as_ref()
                            .and_then(|ns| pt.namespaces.get(ns).map(|entry| entry.dir.clone()))
                    })
                })
                .or_else(|| env::CWD.as_path().canonicalize().ok());
            if let Some(ref d) = dir {
                pt.namespaces
                    .insert(ns.to_string(), NamespaceEntry { dir: d.clone() });
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
        // Sync hosts from the in-memory slug set, not sync_hosts_from_settings():
        // that re-reads the global config via read(), which acquires the lock held
        // above — flock is per open file description, so re-acquiring in the same
        // process deadlocks against our own lock. Staying under the lock also keeps
        // hosts writes ordered with config mutations across concurrent commands.
        let slug_names: Vec<String> = pt.slugs.keys().cloned().collect();
        crate::proxy::hosts::sync_hosts_from_settings_with_slugs(&slug_names);
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
            // Sync hosts from the in-memory slug set, not sync_hosts_from_settings():
            // that re-reads the global config via read(), which acquires the lock held
            // above — flock is per open file description, so re-acquiring in the same
            // process deadlocks against our own lock. Staying under the lock also keeps
            // hosts writes ordered with config mutations across concurrent commands.
            let slug_names: Vec<String> = pt.slugs.keys().cloned().collect();
            crate::proxy::hosts::sync_hosts_from_settings_with_slugs(&slug_names);
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
                dir: env::expand_tilde(dir),
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
    pub ready_output: Option<ReadyOutput>,
    /// HTTP URL to poll for readiness. Accepts any 2xx response by default, or configured statuses.
    pub ready_http: Option<ReadyHttp>,
    /// TCP port to check for readiness (connection success = ready).
    /// Accepts a port number, a Tera template string that renders to one, or an
    /// object with an optional overall polling timeout.
    pub ready_port: Option<ReadyPort>,
    /// Shell command to poll for readiness (exit code 0 = ready)
    pub ready_cmd: Option<ReadyCmd>,
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
    /// Effective user for this daemon: per-daemon `user` overrides `settings.supervisor.user`.
    ///
    /// Returns `None` when neither is set (inherit the supervisor's user).
    pub fn effective_user(&self) -> Option<String> {
        let daemon_user = self
            .user
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty());
        daemon_user.map(str::to_owned).or_else(|| {
            let s = crate::settings::settings();
            let su = s.supervisor.user.trim();
            (!su.is_empty()).then(|| su.to_owned())
        })
    }

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

        let effective_user = self.effective_user();
        let dir = crate::ipc::batch::resolve_daemon_dir(
            self.dir.as_deref(),
            self.path.as_deref(),
            effective_user.as_deref(),
        );
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
            ready_port: self.ready_port.clone(),
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
    fn test_registry_dirs_expand_tilde() {
        let pt = PitchforkToml::parse_str(
            r#"
[slugs.api]
dir = "~/projects/api"

[namespaces.web]
dir = "~/projects/web"
"#,
            Path::new("/tmp/config.toml"),
        )
        .unwrap();

        assert_eq!(
            pt.slugs["api"].dir,
            Some(crate::env::HOME_DIR.join("projects/api"))
        );
        assert_eq!(
            pt.namespaces["web"].dir,
            crate::env::HOME_DIR.join("projects/web")
        );
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

    #[test]
    fn test_config_cache_hit_and_invalidation() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let config_path = dir.join("pitchfork.toml");
        std::fs::write(&config_path, "[daemons.api]\nrun = \"echo v1\"\n").unwrap();

        // Clear any pre-existing cache entries for this directory.
        super::invalidate_config_cache();

        // First call: cache miss, reads from disk.
        let pt1 = PitchforkToml::all_merged_from(dir).unwrap();
        let daemon_id = DaemonId::new(namespace_from_path(&config_path).unwrap(), "api");
        assert_eq!(pt1.daemons[&daemon_id].run, "echo v1");

        // Second call: should be a cache hit (same mtime).
        let pt2 = PitchforkToml::all_merged_from(dir).unwrap();
        assert_eq!(pt2.daemons[&daemon_id].run, "echo v1");

        // Modify the config file — mtime changes, cache should miss.
        // Sleep briefly to ensure mtime resolution differs.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&config_path, "[daemons.api]\nrun = \"echo v2\"\n").unwrap();

        let pt3 = PitchforkToml::all_merged_from(dir).unwrap();
        assert_eq!(pt3.daemons[&daemon_id].run, "echo v2");

        // Explicit invalidation should also force a re-read.
        super::invalidate_config_cache();
        let pt4 = PitchforkToml::all_merged_from(dir).unwrap();
        assert_eq!(pt4.daemons[&daemon_id].run, "echo v2");

        // Clean up.
        super::invalidate_config_cache();
    }

    #[test]
    fn test_config_cache_invalidation_on_write() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let config_path = dir.join("pitchfork.toml");
        std::fs::write(&config_path, "[daemons.api]\nrun = \"echo v1\"\n").unwrap();

        super::invalidate_config_cache();

        // Populate cache.
        let pt1 = PitchforkToml::all_merged_from(dir).unwrap();
        let daemon_id = DaemonId::new(namespace_from_path(&config_path).unwrap(), "api");
        assert_eq!(pt1.daemons[&daemon_id].run, "echo v1");

        // Write via PitchforkToml::write() — should invalidate cache.
        let mut pt = PitchforkToml::read(&config_path).unwrap();
        pt.daemons.get_mut(&daemon_id).unwrap().run = "echo v3".to_string();
        // write() needs the path set and namespace match
        let _ = pt.write();

        // Next read should see the updated value, not the cached one.
        let pt2 = PitchforkToml::all_merged_from(dir).unwrap();
        assert_eq!(pt2.daemons[&daemon_id].run, "echo v3");

        super::invalidate_config_cache();
    }

    #[test]
    fn test_config_cache_size_invalidation() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let config_path = dir.join("pitchfork.toml");
        std::fs::write(&config_path, "[daemons.api]\nrun = \"echo v1\"\n").unwrap();

        super::invalidate_config_cache();

        // Populate cache.
        let pt1 = PitchforkToml::all_merged_from(dir).unwrap();
        let daemon_id = DaemonId::new(namespace_from_path(&config_path).unwrap(), "api");
        assert_eq!(pt1.daemons[&daemon_id].run, "echo v1");

        // Capture the original mtime, then write different-size content and
        // restore the *same* mtime — simulating `cp --preserve=timestamps`
        // or a same-second edit on a coarse-grained filesystem.
        let original_mtime = std::fs::metadata(&config_path).unwrap().modified().unwrap();
        std::fs::write(&config_path, "[daemons.api]\nrun = \"echo different\"\n").unwrap();
        // On Windows, set_times requires the file handle to be opened with
        // write access; File::open is read-only.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&config_path)
            .unwrap();
        let times = std::fs::FileTimes::new().set_modified(original_mtime);
        file.set_times(times).unwrap();

        // Size changed (shorter run string), so cache should miss even though
        // mtime is identical.
        let pt2 = PitchforkToml::all_merged_from(dir).unwrap();
        assert_eq!(
            pt2.daemons[&daemon_id].run, "echo different",
            "cache should invalidate on size change even with identical mtime"
        );

        super::invalidate_config_cache();
    }

    #[test]
    fn test_find_project_root_in_plain_dir_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(find_project_root(temp.path()), None);
    }

    #[test]
    fn test_find_project_root_finds_git_marker() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        let sub = repo.join("sub/dir");
        std::fs::create_dir_all(&sub).unwrap();

        // `find_project_root` canonicalizes, so compare against the canonical
        // path (on Windows this differs by the `\\?\` verbatim prefix).
        assert_eq!(find_project_root(&sub), Some(repo.canonicalize().unwrap()));
    }

    #[test]
    fn test_find_project_root_accepts_git_file_marker() {
        // Linked git worktrees store `.git` as a *file* pointing at the
        // common gitdir, so `exists()` (not `is_dir()`) is the right check.
        let temp = tempfile::tempdir().unwrap();
        let wt = temp.path().join("my-worktree");
        std::fs::create_dir(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /tmp/some-common-gitdir\n").unwrap();

        assert_eq!(find_project_root(&wt), Some(wt.canonicalize().unwrap()));
    }

    /// A symlinked start dir must resolve into the repository hierarchy so
    /// `parent()` traversal does not walk out of the repo.
    #[cfg(unix)]
    #[test]
    fn test_find_project_root_resolves_symlinked_start_dir() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("real-repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        let sub = repo.join("sub/dir");
        std::fs::create_dir_all(&sub).unwrap();
        let link = temp.path().join("link-to-sub");
        symlink(&sub, &link).unwrap();

        assert_eq!(find_project_root(&link), Some(repo));
    }

    /// Build a real git repository with a linked worktree and assert that
    /// `all_merged_all_namespaces_from` picks up daemons from both.
    #[test]
    fn test_all_merged_all_namespaces_discovers_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir(&repo).unwrap();

        // git worktree add requires at least one commit.
        let git_init = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&repo)
            .output()
            .expect("git init");
        assert!(git_init.status.success(), "git init failed: {:?}", git_init);

        std::fs::write(repo.join("main.toml"), "hello\n").unwrap();

        let git_commit = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=pitchfork-test",
                "-c",
                "user.email=pitchfork-test@example.com",
                "add",
                "-A",
            ])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(git_commit.status.success());

        let git_commit = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=pitchfork-test",
                "-c",
                "user.email=pitchfork-test@example.com",
                "commit",
                "-m",
                "init",
            ])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(
            git_commit.status.success(),
            "git commit failed: {:?}",
            git_commit
        );

        let wt = temp.path().join("my-repo-feature");
        let git_wt = std::process::Command::new("git")
            .args(["worktree", "add", "-b", "feature-x", wt.to_str().unwrap()])
            .current_dir(&repo)
            .output()
            .expect("git worktree add");
        assert!(
            git_wt.status.success(),
            "git worktree add failed: {:?}",
            git_wt
        );

        // Config in the main checkout.
        std::fs::write(
            repo.join("pitchfork.toml"),
            "[daemons.api]\nrun = \"echo main\"\n",
        )
        .unwrap();
        // Config in the linked worktree (different namespace: dir name).
        std::fs::write(
            wt.join("pitchfork.toml"),
            "[daemons.worker]\nrun = \"echo wt\"\n",
        )
        .unwrap();

        super::invalidate_config_cache();

        // Resolve from inside the worktree: both namespaces must be visible.
        let pt = PitchforkToml::all_merged_all_namespaces_from(&wt).unwrap();

        let main_id = DaemonId::new("my-repo", "api");
        let wt_id = DaemonId::new("my-repo-feature", "worker");
        assert!(
            pt.daemons.contains_key(&main_id),
            "main checkout daemon missing"
        );
        assert!(pt.daemons.contains_key(&wt_id), "worktree daemon missing");

        // Resolving from the main checkout must also see the worktree daemon.
        let pt_from_main = PitchforkToml::all_merged_all_namespaces_from(&repo).unwrap();
        assert!(pt_from_main.daemons.contains_key(&wt_id));

        // Clean up.
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", wt.to_str().unwrap()])
            .current_dir(&repo)
            .output();
        super::invalidate_config_cache();
    }

    #[test]
    fn test_adhoc_id_uses_invocation_directory_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("feature-tree");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(
            project.join("pitchfork.toml"),
            "[daemons.other]\nrun = \"true\"\n",
        )
        .unwrap();

        let id = PitchforkToml::resolve_id_allow_adhoc_from("api", &project).unwrap();
        assert_eq!(id, DaemonId::new("feature-tree", "api"));
        let qualified =
            PitchforkToml::resolve_id_allow_adhoc_from("explicit/api", &project).unwrap();
        assert_eq!(qualified, DaemonId::new("explicit", "api"));
    }

    #[test]
    fn test_adhoc_id_falls_back_to_global_without_project_config() {
        let temp = tempfile::tempdir().unwrap();
        let id = PitchforkToml::resolve_id_allow_adhoc_from("api", temp.path()).unwrap();
        assert_eq!(id, DaemonId::new("global", "api"));
    }
}
