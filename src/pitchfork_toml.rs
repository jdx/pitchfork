use crate::daemon_id::DaemonId;
use crate::error::{ConfigParseError, DependencyError, FileError, find_similar_daemon};
use crate::state_file::StateFile;
use crate::{Result, env};
use indexmap::IndexMap;
use miette::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

/// Internal structure for reading config files (uses String keys for short daemon names)
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PitchforkTomlRaw {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub daemons: IndexMap<String, PitchforkTomlDaemonRaw>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hooks: Option<PitchforkTomlHooks>,
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
    #[schemars(skip)]
    pub path: Option<PathBuf>,
}

fn is_global_config(path: &Path) -> bool {
    path == *env::PITCHFORK_GLOBAL_CONFIG_USER || path == *env::PITCHFORK_GLOBAL_CONFIG_SYSTEM
}

fn is_local_config(path: &Path) -> bool {
    path.file_name()
        .map(|n| n == "pitchfork.local.toml")
        .unwrap_or(false)
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
    let raw_namespace = path
        .parent()
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
                "namespace '{}' does not match sibling pitchfork.toml namespace '{}'",
                local_ns, base_ns
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

        // Look for matching qualified IDs in the config
        let matches: Vec<DaemonId> = self
            .daemons
            .keys()
            .filter(|id| id.name() == user_id)
            .cloned()
            .collect();

        if matches.is_empty() {
            // No config matches. Validate short ID format and return no matches.
            let _ = DaemonId::try_new("global", user_id)?;
        }
        Ok(matches)
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

        // Also allow existing ad-hoc daemons (persisted in state file) to be
        // referenced by short ID. This keeps commands like status/restart/stop
        // working for daemons started via `pitchfork run`.
        if let Ok(state) = StateFile::read(&*env::PITCHFORK_STATE_FILE)
            && state.daemons.contains_key(&global_id)
        {
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
    pub fn all_merged() -> Result<PitchforkToml> {
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
    ///
    /// # Errors
    /// Prints (but does not abort) if a config file cannot be read. Aborts with an error
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
                    // directory to share a namespace.
                    if p.exists() && !is_global_config(&p) {
                        let ns = namespace_from_path(&p)?;
                        let origin_dir = p
                            .parent()
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
                Err(e) => eprintln!("error reading {}: {}", p.display(), e),
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
                        "namespace '{}' does not match sibling pitchfork.toml namespace '{}'",
                        local_ns, base_ns
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
                hooks: raw_daemon.hooks,
                path: Some(path.to_path_buf()),
            };
            pt.daemons.insert(id, daemon);
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

            // Determine the namespace for this config file
            let config_namespace = if path.exists() {
                namespace_from_path(path)?
            } else {
                namespace_from_path_with_override(path, self.namespace.as_deref())?
            };

            // Convert back to raw format for writing (use short names as keys)
            let mut raw = PitchforkTomlRaw {
                namespace: self.namespace.clone(),
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
                    hooks: daemon.hooks.clone(),
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
    pub fn merge(&mut self, pt: Self) {
        for (id, d) in pt.daemons {
            self.daemons.insert(id, d);
        }
    }
}

/// Lifecycle hooks for a daemon
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct PitchforkTomlHooks {
    /// Command to run when the daemon becomes ready
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_ready: Option<String>,
    /// Command to run when the daemon fails and all retries are exhausted
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_fail: Option<String>,
    /// Command to run before each retry attempt
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_retry: Option<String>,
}

/// Configuration for a single daemon (internal representation with DaemonId)
#[derive(Debug, Clone, JsonSchema)]
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
    /// Delay in milliseconds before considering the daemon ready
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
    #[schemars(default)]
    pub depends: Vec<DaemonId>,
    /// File patterns to watch for changes
    #[schemars(default)]
    pub watch: Vec<String>,
    /// Working directory for the daemon. Relative paths are resolved from the pitchfork.toml location.
    pub dir: Option<String>,
    /// Environment variables to set for the daemon process
    pub env: Option<IndexMap<String, String>>,
    /// Lifecycle hooks (on_ready, on_fail, on_retry)
    pub hooks: Option<PitchforkTomlHooks>,
    #[schemars(skip)]
    pub path: Option<PathBuf>,
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

/// Auto start/stop configuration
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PitchforkTomlAuto {
    Start,
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
