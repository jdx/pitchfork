use crate::Result;
use crate::daemon::Daemon;
use crate::daemon_id::{DaemonId, validate_namespace};
use crate::daemon_status::DaemonStatus;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{NamespaceEntry, PitchforkToml};
use indexmap::IndexMap;
use std::collections::HashSet;

/// A set of namespaces to scope daemon listings to.
///
/// An empty filter matches every daemon. Built from the `--namespace` and
/// `--project` CLI flags via [`NamespaceFilter::from_flags`].
#[derive(Debug, Clone, Default)]
pub struct NamespaceFilter {
    namespaces: Vec<String>,
}

impl NamespaceFilter {
    /// Creates a filter from a list of namespaces, deduplicating them.
    pub fn new(mut namespaces: Vec<String>) -> Self {
        namespaces.sort();
        namespaces.dedup();
        Self { namespaces }
    }

    /// Builds a filter from the `--namespace` and `--project` CLI flags.
    ///
    /// `--project` adds the current directory's namespace, resolved the same
    /// way short daemon IDs are: the nearest config file's namespace, falling
    /// back to `global` when no config file is found.
    pub fn from_flags(namespaces: &[String], project: bool) -> Result<Self> {
        let mut all = Vec::with_capacity(namespaces.len() + 1);
        for ns in namespaces {
            validate_namespace(ns)?;
            all.push(ns.clone());
        }
        if project {
            all.push(PitchforkToml::namespace_for_dir(&crate::env::CWD)?);
        }
        Ok(Self::new(all))
    }

    /// True when no namespaces are set, i.e. the filter matches everything.
    pub fn is_empty(&self) -> bool {
        self.namespaces.is_empty()
    }

    /// Whether the daemon ID's namespace is within this filter's scope.
    pub fn matches(&self, id: &DaemonId) -> bool {
        self.namespaces.is_empty() || self.namespaces.iter().any(|ns| ns == id.namespace())
    }

    /// The filter's namespace when scoped to exactly one, `None` otherwise.
    pub fn single(&self) -> Option<&str> {
        match self.namespaces.as_slice() {
            [ns] => Some(ns),
            _ => None,
        }
    }
}

/// Represents a daemon entry that can be either tracked (from state file) or available (from config only)
#[derive(Debug, Clone)]
pub struct DaemonListEntry {
    pub id: DaemonId,
    pub daemon: Daemon,
    pub is_disabled: bool,
    pub is_available: bool, // true if daemon is only in config, not in state
}

/// Get a unified list of all daemons from IPC client and config
///
/// This function merges daemons from the state file (including failed daemons) with daemons
/// defined in config files. Daemons that are only in config (not in state file) are marked
/// as "available".
///
/// This logic is shared across:
/// - `pitchfork list` command
/// - TUI daemon list
///
/// # Arguments
/// * `client` - IPC client to communicate with supervisor (used only for disabled list)
/// * `filter` - Namespace scope; entries outside it are never built or returned
///
/// # Returns
/// A vector of daemon entries with their current status
pub async fn get_all_daemons(
    client: &IpcClient,
    filter: &NamespaceFilter,
) -> Result<Vec<DaemonListEntry>> {
    let config = PitchforkToml::all_merged()?;

    // Read state file to get all daemons (including failed ones)
    let state_file = crate::state_file::StateFile::read(&*crate::env::PITCHFORK_STATE_FILE)?;
    let state_daemons: Vec<Daemon> = state_file.daemons.values().cloned().collect();

    let disabled_daemons = client.get_disabled_daemons().await?;
    let disabled_set: HashSet<DaemonId> = disabled_daemons.into_iter().collect();

    build_daemon_list(
        state_daemons,
        disabled_set,
        config,
        PitchforkToml::read_global_namespaces(),
        filter,
    )
}

/// Get a unified list of all daemons from supervisor directly (for Web UI)
///
/// This function is used by the Web UI which runs inside the supervisor process
/// and can access the supervisor directly without IPC.
///
/// # Arguments
/// * `supervisor` - Reference to the supervisor instance
///
/// # Returns
/// A vector of daemon entries with their current status
pub async fn get_all_daemons_direct(
    supervisor: &crate::supervisor::Supervisor,
) -> Result<Vec<DaemonListEntry>> {
    let config = PitchforkToml::all_merged()?;

    // Read all daemons from state file (including failed/stopped ones)
    let state_file = supervisor.state_file.lock().await;
    let state_daemons: Vec<Daemon> = state_file.daemons.values().cloned().collect();
    let disabled_set: HashSet<DaemonId> = state_file.disabled.clone().into_iter().collect();
    drop(state_file); // Release lock early

    build_daemon_list(
        state_daemons,
        disabled_set,
        config,
        PitchforkToml::read_global_namespaces(),
        &NamespaceFilter::default(),
    )
}

/// Look up a single daemon by ID from state + config (for Web UI show handler).
///
/// Checks the state file first, then falls back to config files (including namespaces).
/// Returns `None` if the daemon is not found anywhere.
pub async fn get_daemon_direct(
    supervisor: &crate::supervisor::Supervisor,
    id: &DaemonId,
) -> Result<Option<DaemonListEntry>> {
    let pitchfork_id = DaemonId::pitchfork();
    if *id == pitchfork_id {
        return Ok(None);
    }

    // Check state file first
    let state_file = supervisor.state_file.lock().await;
    if let Some(daemon) = state_file.daemons.get(id).cloned() {
        let is_disabled = state_file.disabled.contains(id);
        drop(state_file);
        return Ok(Some(DaemonListEntry {
            id: id.clone(),
            is_available: daemon.config_registered,
            daemon,
            is_disabled,
        }));
    }
    let is_disabled = state_file.disabled.contains(id);
    drop(state_file);

    // Not in state — look in local config
    let config = PitchforkToml::all_merged()?;
    if let Some(daemon_config) = config.daemons.get(id) {
        return Ok(Some(DaemonListEntry {
            id: id.clone(),
            daemon: build_placeholder_daemon(id, daemon_config),
            is_disabled,
            is_available: true,
        }));
    }

    // Check registered namespaces
    let namespaces = PitchforkToml::read_global_namespaces();
    for (_, entry) in namespaces {
        match PitchforkToml::all_merged_from(&entry.dir) {
            Ok(ns_config) => {
                if let Some(daemon_config) = ns_config.daemons.get(id) {
                    return Ok(Some(DaemonListEntry {
                        id: id.clone(),
                        daemon: build_placeholder_daemon(id, daemon_config),
                        is_disabled,
                        is_available: true,
                    }));
                }
            }
            Err(e) => {
                log::warn!("Failed to load namespace from {}: {e}", entry.dir.display());
            }
        }
    }

    Ok(None)
}

/// Build a placeholder Daemon from config for daemons that exist in config but not state.
pub fn build_placeholder_daemon(
    id: &DaemonId,
    daemon_config: &crate::pitchfork_toml::PitchforkTomlDaemon,
) -> Daemon {
    Daemon {
        id: id.clone(),
        status: DaemonStatus::Stopped,
        port: daemon_config.port.clone(),
        depends: vec![],
        env: None,
        watch: vec![],
        watch_mode: daemon_config.watch_mode,
        watch_base_dir: None,
        mise: daemon_config.mise,
        user: daemon_config.user.clone(),
        active_port: None,
        slug: None,
        proxy: None,
        memory_limit: daemon_config.memory_limit,
        cpu_limit: daemon_config.cpu_limit,
        ..Daemon::default()
    }
}

/// Internal helper to build the daemon list from state daemons and config.
///
/// The namespace `filter` is applied here — the shared list path — so every
/// consumer (`pitchfork list`, TUI, MCP) sees the same scoped view and the
/// TUI's fuzzy search operates on an already-scoped set.
fn build_daemon_list(
    state_daemons: Vec<Daemon>,
    disabled_set: HashSet<DaemonId>,
    config: PitchforkToml,
    ns_registry: IndexMap<String, NamespaceEntry>,
    filter: &NamespaceFilter,
) -> Result<Vec<DaemonListEntry>> {
    let mut entries = Vec::new();
    let mut seen_ids = HashSet::new();

    // Skip the supervisor itself
    let pitchfork_id = DaemonId::pitchfork();

    // First, add all daemons from state file
    for daemon in state_daemons {
        if daemon.id == pitchfork_id || !filter.matches(&daemon.id) {
            continue; // Skip supervisor itself and out-of-scope namespaces
        }

        // proxy and mise are stored as Option<bool> in the Daemon struct.
        // None means "inherit from global settings", which is resolved at display/routing time.
        // No override needed here — daemon_list consumers call .unwrap_or(settings()...) themselves.

        seen_ids.insert(daemon.id.clone());
        entries.push(DaemonListEntry {
            id: daemon.id.clone(),
            is_disabled: disabled_set.contains(&daemon.id),
            is_available: daemon.config_registered,
            daemon,
        });
    }

    // Then, add daemons from config that aren't in state file (available daemons)
    for (daemon_id, daemon_config) in &config.daemons {
        if *daemon_id == pitchfork_id || seen_ids.contains(daemon_id) || !filter.matches(daemon_id)
        {
            continue;
        }

        let placeholder = build_placeholder_daemon(daemon_id, daemon_config);

        entries.push(DaemonListEntry {
            id: daemon_id.clone(),
            daemon: placeholder,
            is_disabled: disabled_set.contains(daemon_id),
            is_available: true,
        });
        seen_ids.insert(daemon_id.clone());
    }

    // Add daemons from registered namespaces. Namespace dirs are still loaded
    // even when their registry key is outside the filter: their merged config
    // may include parent-directory configs whose namespaces do match.
    for (ns_name, entry) in ns_registry {
        match PitchforkToml::all_merged_from(&entry.dir) {
            Ok(ns_config) => {
                for (daemon_id, daemon_config) in &ns_config.daemons {
                    if *daemon_id == pitchfork_id
                        || seen_ids.contains(daemon_id)
                        || !filter.matches(daemon_id)
                    {
                        continue;
                    }
                    let placeholder = build_placeholder_daemon(daemon_id, daemon_config);
                    entries.push(DaemonListEntry {
                        id: daemon_id.clone(),
                        daemon: placeholder,
                        is_disabled: disabled_set.contains(daemon_id),
                        is_available: true,
                    });
                    seen_ids.insert(daemon_id.clone());
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to load namespace '{ns_name}' from {}: {e}",
                    entry.dir.display()
                );
            }
        }
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitchfork_toml::PitchforkTomlDaemon;
    use std::path::PathBuf;

    fn state_daemon(ns: &str, name: &str) -> Daemon {
        Daemon {
            id: DaemonId::new(ns, name),
            ..Daemon::default()
        }
    }

    fn config_with(daemons: &[(&str, &str)]) -> PitchforkToml {
        let mut pt = PitchforkToml::new(PathBuf::from("/tmp/pitchfork.toml"));
        for (ns, name) in daemons {
            pt.daemons
                .insert(DaemonId::new(*ns, *name), PitchforkTomlDaemon::default());
        }
        pt
    }

    fn qualified_ids(entries: &[DaemonListEntry]) -> Vec<String> {
        entries.iter().map(|e| e.id.qualified()).collect()
    }

    #[test]
    fn test_empty_filter_matches_everything() {
        let filter = NamespaceFilter::default();
        assert!(filter.is_empty());
        assert!(filter.matches(&DaemonId::new("frontend", "api")));
        assert!(filter.matches(&DaemonId::new("global", "postgres")));
        assert_eq!(filter.single(), None);
    }

    #[test]
    fn test_filter_matches_only_listed_namespaces() {
        let filter = NamespaceFilter::new(vec!["frontend".to_string()]);
        assert!(!filter.is_empty());
        assert!(filter.matches(&DaemonId::new("frontend", "api")));
        assert!(!filter.matches(&DaemonId::new("backend", "api")));
        assert!(!filter.matches(&DaemonId::new("global", "postgres")));
    }

    #[test]
    fn test_filter_multiple_namespaces_union() {
        let filter = NamespaceFilter::new(vec!["frontend".to_string(), "backend".to_string()]);
        assert!(filter.matches(&DaemonId::new("frontend", "api")));
        assert!(filter.matches(&DaemonId::new("backend", "api")));
        assert!(!filter.matches(&DaemonId::new("global", "postgres")));
        assert_eq!(filter.single(), None);
    }

    #[test]
    fn test_filter_single() {
        let filter = NamespaceFilter::new(vec!["frontend".to_string()]);
        assert_eq!(filter.single(), Some("frontend"));

        // Duplicates collapse to a single namespace
        let filter = NamespaceFilter::new(vec!["frontend".to_string(), "frontend".to_string()]);
        assert_eq!(filter.single(), Some("frontend"));
    }

    #[test]
    fn test_from_flags_validates_namespaces() {
        // Valid namespaces pass through
        let filter = NamespaceFilter::from_flags(&["frontend".to_string()], false).unwrap();
        assert_eq!(filter.single(), Some("frontend"));

        // Invalid namespaces are rejected with the daemon ID component rules
        assert!(NamespaceFilter::from_flags(&["my--ns".to_string()], false).is_err());
        assert!(NamespaceFilter::from_flags(&["has space".to_string()], false).is_err());
        assert!(NamespaceFilter::from_flags(&["a/b".to_string()], false).is_err());
        assert!(NamespaceFilter::from_flags(&[String::new()], false).is_err());
    }

    #[test]
    fn test_from_flags_dedups() {
        let filter =
            NamespaceFilter::from_flags(&["frontend".to_string(), "frontend".to_string()], false)
                .unwrap();
        assert_eq!(filter.single(), Some("frontend"));
    }

    #[test]
    fn test_build_daemon_list_unfiltered_keeps_all_namespaces() {
        let state = vec![
            state_daemon("frontend", "api"),
            state_daemon("backend", "api"),
        ];
        let config = config_with(&[("frontend", "worker")]);
        let entries = build_daemon_list(
            state,
            HashSet::new(),
            config,
            IndexMap::new(),
            &NamespaceFilter::default(),
        )
        .unwrap();
        let ids = qualified_ids(&entries);
        assert!(ids.contains(&"frontend/api".to_string()));
        assert!(ids.contains(&"backend/api".to_string()));
        assert!(ids.contains(&"frontend/worker".to_string()));
    }

    #[test]
    fn test_build_daemon_list_filters_state_and_config_daemons() {
        let state = vec![
            state_daemon("frontend", "api"),
            state_daemon("backend", "api"),
        ];
        let config = config_with(&[("frontend", "worker"), ("backend", "worker")]);
        let entries = build_daemon_list(
            state,
            HashSet::new(),
            config,
            IndexMap::new(),
            &NamespaceFilter::new(vec!["frontend".to_string()]),
        )
        .unwrap();
        let ids = qualified_ids(&entries);
        assert_eq!(ids, vec!["frontend/api", "frontend/worker"]);
    }

    #[test]
    fn test_build_daemon_list_filter_union_of_namespaces() {
        let state = vec![
            state_daemon("frontend", "api"),
            state_daemon("backend", "api"),
            state_daemon("global", "postgres"),
        ];
        let entries = build_daemon_list(
            state,
            HashSet::new(),
            config_with(&[]),
            IndexMap::new(),
            &NamespaceFilter::new(vec!["frontend".to_string(), "global".to_string()]),
        )
        .unwrap();
        let ids = qualified_ids(&entries);
        assert!(ids.contains(&"frontend/api".to_string()));
        assert!(ids.contains(&"global/postgres".to_string()));
        assert!(!ids.contains(&"backend/api".to_string()));
    }

    #[test]
    fn test_build_daemon_list_filter_preserves_disabled_flag() {
        let state = vec![state_daemon("frontend", "api")];
        let disabled: HashSet<DaemonId> = [DaemonId::new("frontend", "api")].into_iter().collect();
        let entries = build_daemon_list(
            state,
            disabled,
            config_with(&[]),
            IndexMap::new(),
            &NamespaceFilter::new(vec!["frontend".to_string()]),
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_disabled);
    }
}
