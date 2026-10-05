use crate::Result;
use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::env;
use crate::error::{DependencyError, find_similar_daemon};
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlDaemon};
use crate::state_file::StateFile;
use indexmap::IndexMap;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

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
/// The dependencies are those each daemon was started with, as recorded in
/// the state file, so the order holds wherever `stop` runs: the config
/// visible from the current directory need not include the daemons' own
/// projects. Falls back to a single level containing all IDs if the state
/// file cannot be read.
pub fn compute_reverse_stop_order(active_ids: &[DaemonId]) -> Vec<Vec<DaemonId>> {
    if active_ids.is_empty() {
        return Vec::new();
    }
    match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
        Ok(state) => reverse_stop_order(active_ids, &state.daemons),
        Err(e) => {
            warn!(
                "failed to read state for dependency-ordered shutdown, stopping in arbitrary order: {e}"
            );
            vec![active_ids.to_vec()]
        }
    }
}

/// Group `active_ids` into levels to stop one after another: nothing in a
/// level is needed by anything in a later one, directly or through daemons
/// that are not being stopped. Each level can be stopped concurrently, and
/// keeps the order of `active_ids`.
///
/// Dependencies come from the daemons' records. A daemon reached without a
/// record, such as a disabled one that a start skipped, is looked up in the
/// config of the project it was reached from and of every registered
/// project, since a daemon can depend on one in another namespace. One found
/// in neither depends on nothing. Daemons left in a dependency cycle, and what they depend on,
/// are stopped together in the last level.
pub fn reverse_stop_order(
    active_ids: &[DaemonId],
    daemons: &BTreeMap<DaemonId, Daemon>,
) -> Vec<Vec<DaemonId>> {
    let mut configs: HashMap<PathBuf, Option<PitchforkToml>> = HashMap::new();
    reverse_stop_order_with(active_ids, daemons, &mut |project, id| {
        configs
            .entry(project.to_path_buf())
            .or_insert_with(|| PitchforkToml::all_merged_all_namespaces_from(project).ok())
            .as_ref()
            .and_then(|pt| pt.daemons.get(id))
            .map(|d| d.depends.clone())
            .unwrap_or_default()
    })
}

/// [`reverse_stop_order`], with `unrecorded` giving the dependencies of a
/// daemon that has no record, from the project directory it was reached from.
fn reverse_stop_order_with(
    active_ids: &[DaemonId],
    daemons: &BTreeMap<DaemonId, Daemon>,
    unrecorded: &mut dyn FnMut(&Path, &DaemonId) -> Vec<DaemonId>,
) -> Vec<Vec<DaemonId>> {
    let mut ids: Vec<&DaemonId> = Vec::new();
    let mut index: HashMap<&DaemonId, usize> = HashMap::new();
    for id in active_ids {
        if !index.contains_key(id) {
            index.insert(id, ids.len());
            ids.push(id);
        }
    }

    // Links between the daemons being stopped only: the ones each needs
    // directly or through daemons that are not. That is as many links as the
    // records have, rather than everything each daemon needs at any depth,
    // and stopping by them is the same order.
    let needs: Vec<Vec<usize>> = ids
        .iter()
        .map(|id| stopped_dependencies(id, daemons, &index, unrecorded))
        .collect();
    let mut needed_by = vec![0usize; ids.len()];
    for deps in &needs {
        for &dep in deps {
            needed_by[dep] += 1;
        }
    }

    let mut levels: Vec<Vec<DaemonId>> = Vec::new();
    let mut placed = vec![false; ids.len()];
    let mut level: Vec<usize> = (0..ids.len()).filter(|&i| needed_by[i] == 0).collect();
    while !level.is_empty() {
        let mut next = Vec::new();
        for &i in &level {
            placed[i] = true;
            for &dep in &needs[i] {
                needed_by[dep] -= 1;
                if needed_by[dep] == 0 {
                    next.push(dep);
                }
            }
        }
        levels.push(level.iter().map(|&i| ids[i].clone()).collect());
        next.sort_unstable();
        level = next;
    }
    let cycle: Vec<DaemonId> = (0..ids.len())
        .filter(|&i| !placed[i])
        .map(|i| ids[i].clone())
        .collect();
    if !cycle.is_empty() {
        levels.push(cycle);
    }

    debug!("shutdown order: {levels:?}");
    levels
}

/// The daemons being stopped (indexed by `index`) that `id` needs according
/// to the records, directly or through daemons that are not being stopped.
/// The search does not go past a daemon being stopped: what that one needs
/// are its own links. A daemon without a record is resolved by `unrecorded`
/// in the project of the recorded daemon it was reached through.
fn stopped_dependencies(
    id: &DaemonId,
    daemons: &BTreeMap<DaemonId, Daemon>,
    index: &HashMap<&DaemonId, usize>,
    unrecorded: &mut dyn FnMut(&Path, &DaemonId) -> Vec<DaemonId>,
) -> Vec<usize> {
    let mut found = Vec::new();
    let mut seen: HashSet<DaemonId> = HashSet::from([id.clone()]);
    let mut queue: VecDeque<(DaemonId, Option<PathBuf>)> = VecDeque::from([(id.clone(), None)]);
    while let Some((current, reached_from)) = queue.pop_front() {
        let (depends, project) = match daemons.get(&current) {
            Some(daemon) => (
                daemon.depends.clone(),
                daemon.watch_base_dir.clone().or(reached_from),
            ),
            None => match reached_from {
                Some(project) => (unrecorded(&project, &current), Some(project)),
                None => continue,
            },
        };
        for dep in depends {
            if seen.contains(&dep) {
                continue;
            }
            seen.insert(dep.clone());
            match index.get(&dep) {
                Some(&i) => found.push(i),
                None => queue.push_back((dep, project.clone())),
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_id::DaemonId;
    use crate::pitchfork_toml::PitchforkTomlDaemon;
    use indexmap::IndexMap;

    // Helper to build a test daemon with only `depends` set, all other fields default/None.
    // Keeps tests concise while satisfying all required struct fields.

    fn make_daemon(depends: Vec<&str>) -> PitchforkTomlDaemon {
        PitchforkTomlDaemon {
            run: "echo test".into(),
            depends: depends
                .into_iter()
                .map(|s| DaemonId::new("global", s))
                .collect(),
            ..Default::default()
        }
    }

    fn id(name: &str) -> DaemonId {
        DaemonId::new("global", name)
    }

    fn records(entries: &[(DaemonId, &[DaemonId])]) -> BTreeMap<DaemonId, Daemon> {
        entries
            .iter()
            .map(|(id, depends)| {
                (
                    id.clone(),
                    Daemon {
                        id: id.clone(),
                        depends: depends.to_vec(),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    #[test]
    fn test_stop_order_follows_recorded_dependencies_of_any_project() {
        let db = DaemonId::new("projb", "db");
        let app = DaemonId::new("projb", "app");
        let daemons = records(&[(db.clone(), &[]), (app.clone(), std::slice::from_ref(&db))]);
        assert_eq!(
            reverse_stop_order(&[db.clone(), app.clone()], &daemons),
            vec![vec![app], vec![db]]
        );
    }

    #[test]
    fn test_stop_order_goes_through_daemons_not_being_stopped() {
        // web needs cache only through api, which is not running.
        let daemons = records(&[
            (id("cache"), &[]),
            (id("api"), &[id("cache")]),
            (id("web"), &[id("api")]),
        ]);
        assert_eq!(
            reverse_stop_order(&[id("cache"), id("web")], &daemons),
            vec![vec![id("web")], vec![id("cache")]]
        );
    }

    #[test]
    fn test_stop_order_puts_unrelated_and_unrecorded_daemons_first() {
        let daemons = records(&[(id("db"), &[]), (id("app"), &[id("db")])]);
        assert_eq!(
            reverse_stop_order(&[id("db"), id("adhoc"), id("app")], &daemons),
            vec![vec![id("adhoc"), id("app")], vec![id("db")]]
        );
    }

    #[test]
    fn test_stop_order_goes_through_a_daemon_without_a_record_by_its_config() {
        // api was skipped as disabled when web started, so it has no record;
        // its project's config says it needs cache.
        let project = PathBuf::from("/project");
        let mut daemons = records(&[(id("cache"), &[]), (id("web"), &[id("api")])]);
        daemons.get_mut(&id("web")).unwrap().watch_base_dir = Some(project.clone());
        let mut looked_up = Vec::new();
        let levels =
            reverse_stop_order_with(&[id("cache"), id("web")], &daemons, &mut |dir, daemon| {
                looked_up.push((dir.to_path_buf(), daemon.clone()));
                if *daemon == id("api") {
                    vec![id("cache")]
                } else {
                    vec![]
                }
            });
        assert_eq!(levels, vec![vec![id("web")], vec![id("cache")]]);
        assert_eq!(looked_up, vec![(project, id("api"))]);
    }

    #[test]
    fn test_stop_order_stops_a_cycle_together_last() {
        let daemons = records(&[
            (id("a"), &[id("b")]),
            (id("b"), &[id("a")]),
            (id("c"), &[id("a")]),
        ]);
        assert_eq!(
            reverse_stop_order(&[id("a"), id("b"), id("c")], &daemons),
            vec![vec![id("c")], vec![id("a"), id("b")]]
        );
    }

    /// The order as first written: every daemon's full set of transitive
    /// dependencies, peeled one level at a time. Kept to check that the
    /// linear version orders exactly the same.
    fn reference_stop_order(
        active_ids: &[DaemonId],
        daemons: &BTreeMap<DaemonId, Daemon>,
    ) -> Vec<Vec<DaemonId>> {
        let all_needs = |id: &DaemonId| {
            let mut found = HashSet::new();
            let mut queue = VecDeque::from([id.clone()]);
            while let Some(current) = queue.pop_front() {
                for dep in daemons
                    .get(&current)
                    .map(|d| &d.depends)
                    .into_iter()
                    .flatten()
                {
                    if found.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
            found
        };
        let mut remaining: Vec<DaemonId> = Vec::new();
        for id in active_ids {
            if !remaining.contains(id) {
                remaining.push(id.clone());
            }
        }
        let needs: HashMap<DaemonId, HashSet<DaemonId>> = remaining
            .iter()
            .map(|id| (id.clone(), all_needs(id)))
            .collect();
        let mut levels = Vec::new();
        while !remaining.is_empty() {
            let level: Vec<DaemonId> = remaining
                .iter()
                .filter(|id| {
                    !remaining
                        .iter()
                        .any(|other| other != *id && needs[other].contains(*id))
                })
                .cloned()
                .collect();
            if level.is_empty() {
                levels.push(std::mem::take(&mut remaining));
                break;
            }
            remaining.retain(|id| !level.contains(id));
            levels.push(level);
        }
        levels
    }

    #[test]
    fn test_stop_order_matches_the_reference_on_random_graphs() {
        // A small LCG keeps the graphs reproducible without a dependency.
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        for _ in 0..500 {
            let count = 1 + next(12);
            let names: Vec<DaemonId> = (0..count).map(|i| id(&format!("d{i}"))).collect();
            let mut deps: Vec<(DaemonId, Vec<DaemonId>)> = Vec::new();
            for name in &names {
                // Mostly forward links, with the odd backward one for cycles.
                let links = (0..next(3))
                    .map(|_| names[next(count) as usize].clone())
                    .collect();
                deps.push((name.clone(), links));
            }
            let daemons: BTreeMap<DaemonId, Daemon> = deps
                .iter()
                .filter(|_| next(5) != 0) // Some daemons have no record.
                .map(|(id, depends)| {
                    (
                        id.clone(),
                        Daemon {
                            id: id.clone(),
                            depends: depends.clone(),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            let active: Vec<DaemonId> = names
                .iter()
                .filter(|_| next(3) != 0) // Some are not being stopped.
                .cloned()
                .collect();
            assert_eq!(
                reverse_stop_order(&active, &daemons),
                reference_stop_order(&active, &daemons),
                "active {active:?}, records {deps:?}"
            );
        }
    }

    #[test]
    fn test_stop_order_of_a_long_chain() {
        let names: Vec<DaemonId> = (0..5000).map(|i| id(&format!("d{i}"))).collect();
        let daemons: BTreeMap<DaemonId, Daemon> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                (
                    name.clone(),
                    Daemon {
                        id: name.clone(),
                        depends: names.get(i + 1).cloned().into_iter().collect(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        // Every other daemon is running; the rest are links through stopped ones.
        let active: Vec<DaemonId> = names.iter().step_by(2).cloned().collect();
        let levels = reverse_stop_order(&active, &daemons);
        assert_eq!(
            levels,
            active.iter().map(|id| vec![id.clone()]).collect::<Vec<_>>()
        );
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
