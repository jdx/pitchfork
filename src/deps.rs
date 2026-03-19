use crate::Result;
use crate::daemon_id::DaemonId;
use crate::error::{DependencyError, find_similar_daemon};
use crate::pitchfork_toml::PitchforkTomlDaemon;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::pitchfork_toml::PitchforkToml;

/// Result of dependency resolution
#[derive(Debug)]
pub struct DependencyOrder {
    /// Groups of daemons that can be started in parallel.
    /// Each level depends only on daemons in previous levels.
    pub levels: Vec<Vec<DaemonId>>,
}

/// Resolve dependency order using Kahn's algorithm (topological sort).
///
/// Returns daemons grouped into levels where:
/// - Level 0: daemons with no dependencies (or deps already satisfied)
/// - Level 1: daemons that only depend on level 0
/// - Level N: daemons that only depend on levels 0..(N-1)
///
/// Daemons within the same level can be started in parallel.
pub fn resolve_dependencies(
    requested: &[DaemonId],
    all_daemons: &IndexMap<DaemonId, PitchforkTomlDaemon>,
) -> Result<DependencyOrder> {
    // 1. Build the full set of daemons to start (requested + transitive deps)
    let mut to_start: HashSet<DaemonId> = HashSet::new();
    let mut queue: VecDeque<DaemonId> = requested.iter().cloned().collect();

    while let Some(id) = queue.pop_front() {
        if to_start.contains(&id) {
            continue;
        }

        let daemon = all_daemons.get(&id).ok_or_else(|| {
            let suggestion = find_similar_daemon(
                &id.qualified(),
                all_daemons
                    .keys()
                    .map(|k| k.qualified())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(|s| s.as_str()),
            );
            DependencyError::DaemonNotFound {
                name: id.qualified(),
                suggestion,
            }
        })?;

        to_start.insert(id.clone());

        // Add dependencies to queue
        for dep in &daemon.depends {
            if !all_daemons.contains_key(dep) {
                return Err(DependencyError::MissingDependency {
                    daemon: id.qualified(),
                    dependency: dep.qualified(),
                }
                .into());
            }
            if !to_start.contains(dep) {
                queue.push_back(dep.clone());
            }
        }
    }

    // 2. Build adjacency list and in-degree map
    let mut in_degree: HashMap<DaemonId, usize> = HashMap::new();
    let mut dependents: HashMap<DaemonId, Vec<DaemonId>> = HashMap::new();

    for id in &to_start {
        in_degree.entry(id.clone()).or_insert(0);
        dependents.entry(id.clone()).or_default();
    }

    for id in &to_start {
        let daemon = all_daemons.get(id).ok_or_else(|| {
            miette::miette!("Internal error: daemon '{}' missing from configuration", id)
        })?;
        for dep in &daemon.depends {
            if to_start.contains(dep) {
                *in_degree.get_mut(id).ok_or_else(|| {
                    miette::miette!("Internal error: in_degree missing for daemon '{}'", id)
                })? += 1;
                dependents
                    .get_mut(dep)
                    .ok_or_else(|| {
                        miette::miette!("Internal error: dependents missing for daemon '{}'", dep)
                    })?
                    .push(id.clone());
            }
        }
    }

    // 3. Kahn's algorithm with level tracking
    let mut processed: HashSet<DaemonId> = HashSet::new();
    let mut levels: Vec<Vec<DaemonId>> = Vec::new();
    let mut current_level: Vec<DaemonId> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    // Sort for deterministic order
    current_level.sort();

    while !current_level.is_empty() {
        let mut next_level = Vec::new();

        for id in &current_level {
            processed.insert(id.clone());

            let deps = dependents.get(id).ok_or_else(|| {
                miette::miette!("Internal error: dependents missing for daemon '{}'", id)
            })?;
            for dependent in deps {
                let deg = in_degree.get_mut(dependent).ok_or_else(|| {
                    miette::miette!(
                        "Internal error: in_degree missing for daemon '{}'",
                        dependent
                    )
                })?;
                *deg -= 1;
                if *deg == 0 {
                    next_level.push(dependent.clone());
                }
            }
        }

        levels.push(current_level);
        next_level.sort(); // Sort for deterministic order
        current_level = next_level;
    }

    // 4. Check for cycles
    if processed.len() != to_start.len() {
        let mut involved: Vec<_> = to_start
            .difference(&processed)
            .map(|id| id.qualified())
            .collect();
        involved.sort(); // Deterministic output
        return Err(DependencyError::CircularDependency { involved }.into());
    }

    Ok(DependencyOrder { levels })
}

/// Compute the order in which daemons should be stopped, respecting
/// reverse dependency order (dependents first, then their dependencies).
///
/// This is a shared helper used by both the supervisor's `close()` and
/// the IPC `stop_daemons()` batch operation.
///
/// Returns a list of levels in reverse dependency order. Each level is a
/// `Vec<DaemonId>` of daemons that can be stopped concurrently.
/// Ad-hoc daemons (not in config) are placed in the first level.
///
/// Falls back to a single level containing all IDs if config loading
/// or dependency resolution fails.
pub fn compute_reverse_stop_order(active_ids: &[DaemonId]) -> Vec<Vec<DaemonId>> {
    compute_reverse_stop_order_with_config(active_ids, None)
}

/// Like [`compute_reverse_stop_order`] but accepts a pre-loaded config to
/// avoid redundant disk I/O when the caller already has one.
pub fn compute_reverse_stop_order_with_config(
    active_ids: &[DaemonId],
    config: Option<&PitchforkToml>,
) -> Vec<Vec<DaemonId>> {
    if active_ids.is_empty() {
        return Vec::new();
    }

    let owned_pt;
    let pt = match config {
        Some(pt) => pt,
        None => match PitchforkToml::all_merged() {
            Ok(loaded) => {
                owned_pt = loaded;
                &owned_pt
            }
            Err(e) => {
                warn!(
                    "failed to load config for dependency-ordered shutdown, stopping in arbitrary order: {e}"
                );
                return vec![active_ids.to_vec()];
            }
        },
    };

    let active_set: HashSet<&DaemonId> = active_ids.iter().collect();
    let config_ids: Vec<DaemonId> = active_ids
        .iter()
        .filter(|id| pt.daemons.contains_key(*id))
        .cloned()
        .collect();
    let adhoc_ids: Vec<DaemonId> = active_ids
        .iter()
        .filter(|id| !pt.daemons.contains_key(*id))
        .cloned()
        .collect();

    if config_ids.is_empty() {
        // All ad-hoc daemons, no dependency ordering needed
        return vec![active_ids.to_vec()];
    }

    match resolve_dependencies(&config_ids, &pt.daemons) {
        Ok(dep_order) => {
            let mut levels: Vec<Vec<DaemonId>> = Vec::new();

            // Stop ad-hoc daemons first (they have no dependency info)
            if !adhoc_ids.is_empty() {
                levels.push(adhoc_ids);
            }

            // Then stop config daemons in reverse dependency order
            for level in dep_order.levels.into_iter().rev() {
                let filtered: Vec<DaemonId> = level
                    .into_iter()
                    .filter(|id| active_set.contains(id))
                    .collect();
                if !filtered.is_empty() {
                    levels.push(filtered);
                }
            }

            debug!("shutdown order: {levels:?}");
            levels
        }
        Err(e) => {
            warn!("dependency resolution failed during shutdown, stopping in arbitrary order: {e}");
            vec![active_ids.to_vec()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_id::DaemonId;
    use crate::pitchfork_toml::{PitchforkTomlDaemon, Retry};
    use indexmap::IndexMap;

    // Helper to build a test daemon with only `depends` set, all other fields default/None.
    // Keeps tests concise while satisfying all required struct fields.

    fn make_daemon(depends: Vec<&str>) -> PitchforkTomlDaemon {
        PitchforkTomlDaemon {
            run: "echo test".to_string(),
            auto: vec![],
            cron: None,
            retry: Retry::default(),
            ready_delay: None,
            ready_output: None,
            ready_http: None,
            ready_port: None,
            ready_cmd: None,
            expected_port: Vec::new(),
            auto_bump_port: false,
            port_bump_attempts: 10,
            boot_start: None,
            depends: depends
                .into_iter()
                .map(|s| DaemonId::new("global", s))
                .collect(),
            watch: vec![],
            dir: None,
            env: None,
            hooks: None,
            path: None,
            mise: None,
            slug: None,
            proxy: None,
            memory_limit: None,
            cpu_limit: None,
        }
    }

    fn id(name: &str) -> DaemonId {
        DaemonId::new("global", name)
    }

    #[test]
    fn test_no_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("api"), make_daemon(vec![]));

        let result = resolve_dependencies(&[id("api")], &daemons).unwrap();

        assert_eq!(result.levels.len(), 1);
        assert_eq!(result.levels[0], vec![id("api")]);
    }

    #[test]
    fn test_simple_dependency() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("postgres"), make_daemon(vec![]));
        daemons.insert(id("api"), make_daemon(vec!["postgres"]));

        let result = resolve_dependencies(&[id("api")], &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        assert_eq!(result.levels[0], vec![id("postgres")]);
        assert_eq!(result.levels[1], vec![id("api")]);
    }

    #[test]
    fn test_multiple_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("postgres"), make_daemon(vec![]));
        daemons.insert(id("redis"), make_daemon(vec![]));
        daemons.insert(id("api"), make_daemon(vec!["postgres", "redis"]));

        let result = resolve_dependencies(&[id("api")], &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        // postgres and redis can start in parallel
        assert!(result.levels[0].contains(&id("postgres")));
        assert!(result.levels[0].contains(&id("redis")));
        assert_eq!(result.levels[1], vec![id("api")]);
    }

    #[test]
    fn test_transitive_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("database"), make_daemon(vec![]));
        daemons.insert(id("backend"), make_daemon(vec!["database"]));
        daemons.insert(id("api"), make_daemon(vec!["backend"]));

        let result = resolve_dependencies(&[id("api")], &daemons).unwrap();

        assert_eq!(result.levels.len(), 3);
        assert_eq!(result.levels[0], vec![id("database")]);
        assert_eq!(result.levels[1], vec![id("backend")]);
        assert_eq!(result.levels[2], vec![id("api")]);
    }

    #[test]
    fn test_diamond_dependency() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("db"), make_daemon(vec![]));
        daemons.insert(id("auth"), make_daemon(vec!["db"]));
        daemons.insert(id("data"), make_daemon(vec!["db"]));
        daemons.insert(id("api"), make_daemon(vec!["auth", "data"]));

        let result = resolve_dependencies(&[id("api")], &daemons).unwrap();

        assert_eq!(result.levels.len(), 3);
        assert_eq!(result.levels[0], vec![id("db")]);
        // auth and data can start in parallel
        assert!(result.levels[1].contains(&id("auth")));
        assert!(result.levels[1].contains(&id("data")));
        assert_eq!(result.levels[2], vec![id("api")]);
    }

    #[test]
    fn test_circular_dependency_detected() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("a"), make_daemon(vec!["c"]));
        daemons.insert(id("b"), make_daemon(vec!["a"]));
        daemons.insert(id("c"), make_daemon(vec!["b"]));

        let result = resolve_dependencies(&[id("a")], &daemons);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("circular dependency"));
    }

    #[test]
    fn test_missing_dependency_error() {
        let mut daemons = IndexMap::new();
        let mut daemon = make_daemon(vec![]);
        daemon.depends = vec![DaemonId::new("global", "nonexistent")];
        daemons.insert(id("api"), daemon);

        let result = resolve_dependencies(&[id("api")], &daemons);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("not defined"));
    }

    #[test]
    fn test_missing_requested_daemon_error() {
        let daemons = IndexMap::new();

        let result = resolve_dependencies(&[id("nonexistent")], &daemons);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_multiple_requested_daemons() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("db"), make_daemon(vec![]));
        daemons.insert(id("api"), make_daemon(vec!["db"]));
        daemons.insert(id("worker"), make_daemon(vec!["db"]));

        let result = resolve_dependencies(&[id("api"), id("worker")], &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        assert_eq!(result.levels[0], vec![id("db")]);
        // api and worker can start in parallel
        assert!(result.levels[1].contains(&id("api")));
        assert!(result.levels[1].contains(&id("worker")));
    }

    #[test]
    fn test_start_all_with_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert(id("db"), make_daemon(vec![]));
        daemons.insert(id("cache"), make_daemon(vec![]));
        daemons.insert(id("api"), make_daemon(vec!["db", "cache"]));
        daemons.insert(id("worker"), make_daemon(vec!["db"]));

        let all_ids: Vec<DaemonId> = daemons.keys().cloned().collect();
        let result = resolve_dependencies(&all_ids, &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        // db and cache have no deps
        assert!(result.levels[0].contains(&id("db")));
        assert!(result.levels[0].contains(&id("cache")));
        // api and worker depend on level 0
        assert!(result.levels[1].contains(&id("api")));
        assert!(result.levels[1].contains(&id("worker")));
    }
}
