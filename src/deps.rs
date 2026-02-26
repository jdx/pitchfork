use crate::Result;
use crate::error::{DependencyError, find_similar_daemon};
use crate::pitchfork_toml::PitchforkTomlDaemon;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet, VecDeque};

/// Result of dependency resolution
#[derive(Debug)]
pub struct DependencyOrder {
    /// Groups of daemons that can be started in parallel.
    /// Each level depends only on daemons in previous levels.
    pub levels: Vec<Vec<String>>,
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
    requested: &[String],
    all_daemons: &IndexMap<String, PitchforkTomlDaemon>,
) -> Result<DependencyOrder> {
    // 1. Build the full set of daemons to start (requested + transitive deps)
    let mut to_start: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = requested.iter().cloned().collect();

    while let Some(id) = queue.pop_front() {
        if to_start.contains(&id) {
            continue;
        }

        let daemon = all_daemons.get(&id).ok_or_else(|| {
            let suggestion = find_similar_daemon(&id, all_daemons.keys().map(|s| s.as_str()));
            DependencyError::DaemonNotFound {
                name: id.clone(),
                suggestion,
            }
        })?;

        to_start.insert(id.clone());

        // Add dependencies to queue
        for dep in &daemon.depends {
            if !all_daemons.contains_key(dep) {
                return Err(DependencyError::MissingDependency {
                    daemon: id.clone(),
                    dependency: dep.clone(),
                }
                .into());
            }
            if !to_start.contains(dep) {
                queue.push_back(dep.clone());
            }
        }
    }

    // 2. Build adjacency list and in-degree map
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

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
    let mut processed: HashSet<String> = HashSet::new();
    let mut levels: Vec<Vec<String>> = Vec::new();
    let mut current_level: Vec<String> = in_degree
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
        let mut involved: Vec<_> = to_start.difference(&processed).cloned().collect();
        involved.sort(); // Deterministic output
        return Err(DependencyError::CircularDependency { involved }.into());
    }

    Ok(DependencyOrder { levels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitchfork_toml::{PitchforkTomlDaemon, Retry};
    use indexmap::IndexMap;

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
            boot_start: None,
            depends: depends.into_iter().map(String::from).collect(),
            watch: vec![],
            dir: None,
            env: None,
            hooks: None,
            path: None,
        }
    }

    #[test]
    fn test_no_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert("api".to_string(), make_daemon(vec![]));

        let result = resolve_dependencies(&["api".to_string()], &daemons).unwrap();

        assert_eq!(result.levels.len(), 1);
        assert_eq!(result.levels[0], vec!["api"]);
    }

    #[test]
    fn test_simple_dependency() {
        let mut daemons = IndexMap::new();
        daemons.insert("postgres".to_string(), make_daemon(vec![]));
        daemons.insert("api".to_string(), make_daemon(vec!["postgres"]));

        let result = resolve_dependencies(&["api".to_string()], &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        assert_eq!(result.levels[0], vec!["postgres"]);
        assert_eq!(result.levels[1], vec!["api"]);
    }

    #[test]
    fn test_multiple_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert("postgres".to_string(), make_daemon(vec![]));
        daemons.insert("redis".to_string(), make_daemon(vec![]));
        daemons.insert("api".to_string(), make_daemon(vec!["postgres", "redis"]));

        let result = resolve_dependencies(&["api".to_string()], &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        // postgres and redis can start in parallel
        assert!(result.levels[0].contains(&"postgres".to_string()));
        assert!(result.levels[0].contains(&"redis".to_string()));
        assert_eq!(result.levels[1], vec!["api"]);
    }

    #[test]
    fn test_transitive_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert("database".to_string(), make_daemon(vec![]));
        daemons.insert("backend".to_string(), make_daemon(vec!["database"]));
        daemons.insert("api".to_string(), make_daemon(vec!["backend"]));

        let result = resolve_dependencies(&["api".to_string()], &daemons).unwrap();

        assert_eq!(result.levels.len(), 3);
        assert_eq!(result.levels[0], vec!["database"]);
        assert_eq!(result.levels[1], vec!["backend"]);
        assert_eq!(result.levels[2], vec!["api"]);
    }

    #[test]
    fn test_diamond_dependency() {
        let mut daemons = IndexMap::new();
        daemons.insert("db".to_string(), make_daemon(vec![]));
        daemons.insert("auth".to_string(), make_daemon(vec!["db"]));
        daemons.insert("data".to_string(), make_daemon(vec!["db"]));
        daemons.insert("api".to_string(), make_daemon(vec!["auth", "data"]));

        let result = resolve_dependencies(&["api".to_string()], &daemons).unwrap();

        assert_eq!(result.levels.len(), 3);
        assert_eq!(result.levels[0], vec!["db"]);
        // auth and data can start in parallel
        assert!(result.levels[1].contains(&"auth".to_string()));
        assert!(result.levels[1].contains(&"data".to_string()));
        assert_eq!(result.levels[2], vec!["api"]);
    }

    #[test]
    fn test_circular_dependency_detected() {
        let mut daemons = IndexMap::new();
        daemons.insert("a".to_string(), make_daemon(vec!["c"]));
        daemons.insert("b".to_string(), make_daemon(vec!["a"]));
        daemons.insert("c".to_string(), make_daemon(vec!["b"]));

        let result = resolve_dependencies(&["a".to_string()], &daemons);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("circular dependency"));
    }

    #[test]
    fn test_missing_dependency_error() {
        let mut daemons = IndexMap::new();
        daemons.insert("api".to_string(), make_daemon(vec!["nonexistent"]));

        let result = resolve_dependencies(&["api".to_string()], &daemons);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("not defined"));
    }

    #[test]
    fn test_missing_requested_daemon_error() {
        let daemons = IndexMap::new();

        let result = resolve_dependencies(&["nonexistent".to_string()], &daemons);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_multiple_requested_daemons() {
        let mut daemons = IndexMap::new();
        daemons.insert("db".to_string(), make_daemon(vec![]));
        daemons.insert("api".to_string(), make_daemon(vec!["db"]));
        daemons.insert("worker".to_string(), make_daemon(vec!["db"]));

        let result =
            resolve_dependencies(&["api".to_string(), "worker".to_string()], &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        assert_eq!(result.levels[0], vec!["db"]);
        // api and worker can start in parallel
        assert!(result.levels[1].contains(&"api".to_string()));
        assert!(result.levels[1].contains(&"worker".to_string()));
    }

    #[test]
    fn test_start_all_with_dependencies() {
        let mut daemons = IndexMap::new();
        daemons.insert("db".to_string(), make_daemon(vec![]));
        daemons.insert("cache".to_string(), make_daemon(vec![]));
        daemons.insert("api".to_string(), make_daemon(vec!["db", "cache"]));
        daemons.insert("worker".to_string(), make_daemon(vec!["db"]));

        let all_ids: Vec<String> = daemons.keys().cloned().collect();
        let result = resolve_dependencies(&all_ids, &daemons).unwrap();

        assert_eq!(result.levels.len(), 2);
        // db and cache have no deps
        assert!(result.levels[0].contains(&"db".to_string()));
        assert!(result.levels[0].contains(&"cache".to_string()));
        // api and worker depend on level 0
        assert!(result.levels[1].contains(&"api".to_string()));
        assert!(result.levels[1].contains(&"worker".to_string()));
    }
}
