//! Project and stack endpoints.
//!
//! A *project* is a namespace registered in the global namespace registry. Its
//! *worktrees* are the git worktrees or jj workspaces discovered under the
//! project directory, including ones that have never run a daemon. A worktree's
//! *stack* is the set of daemon groups declared by the config loaded for that
//! worktree, together with each member daemon's live state.
//!
//! These endpoints are read-only. Fetching a project or a stack never starts a
//! daemon: auto-start happens only when a request reaches a daemon's own proxy
//! hostname.

use axum::{extract::Path, http::StatusCode, response::Json};
use indexmap::IndexMap;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path as StdPath, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::daemons::{ApiDaemonEntry, build_api_daemons, config_daemon_entry};
use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkToml;
use crate::proxy::worktree::{WorktreeEntry, discover_worktrees};

/// Name used for the single synthetic worktree of a project directory that is
/// not a git repository or jj workspace.
const DEFAULT_WORKTREE: &str = "default";

/// Group name that a stack presents first, as its primary action.
const DEFAULT_GROUP: &str = "default";

// ─── views (I/O free data the responses are built from) ──────────────────────

/// One worktree of a project, with the groups declared by its loaded config.
#[derive(Debug, Clone)]
pub struct WorktreeView {
    /// URL segment for this worktree (the sanitized branch/workspace name).
    pub name: String,
    pub branch: String,
    pub path: PathBuf,
    pub namespace: String,
    pub is_primary: bool,
    pub groups: IndexMap<String, Vec<DaemonId>>,
}

/// A project: a registered namespace plus its worktrees.
#[derive(Debug, Clone)]
pub struct ProjectView {
    pub name: String,
    pub dir: PathBuf,
    pub worktrees: Vec<WorktreeView>,
}

// ─── response types ──────────────────────────────────────────────────────────

#[derive(Serialize, Default, PartialEq, Debug)]
pub struct ApiDaemonCounts {
    total: usize,
    running: usize,
    stopped: usize,
    failed: usize,
    available: usize,
}

#[derive(Serialize)]
pub struct ApiProjectSummary {
    name: String,
    dir: String,
    worktree_count: usize,
    daemons: ApiDaemonCounts,
    /// Start time of the most recently started daemon in the project, RFC 3339.
    last_activity: Option<String>,
    url: String,
    api_url: String,
}

#[derive(Serialize)]
pub struct ApiWorktreeSummary {
    name: String,
    branch: String,
    path: String,
    namespace: String,
    is_primary: bool,
    group_count: usize,
    daemons: ApiDaemonCounts,
    last_activity: Option<String>,
    url: String,
    api_url: String,
    /// Disk used by the daemon data directories of this worktree. Omitted
    /// whenever pitchfork does not track a data directory for the daemons.
    #[serde(skip_serializing_if = "Option::is_none")]
    disk_usage_bytes: Option<u64>,
}

#[derive(Serialize)]
pub struct ApiProject {
    name: String,
    dir: String,
    daemons: ApiDaemonCounts,
    last_activity: Option<String>,
    worktrees: Vec<ApiWorktreeSummary>,
    /// Stack of the primary checkout, so the project page can show it inline.
    #[serde(skip_serializing_if = "Option::is_none")]
    stack: Option<ApiStack>,
}

#[derive(Serialize)]
pub struct ApiGroup {
    name: String,
    /// True for the `default` group, which a stack presents first.
    is_default: bool,
    daemons: Vec<ApiDaemonEntry>,
    /// Qualified ids the group declares that no known daemon matches.
    missing: Vec<String>,
    running: usize,
    total: usize,
}

#[derive(Serialize)]
pub struct ApiStack {
    project: String,
    worktree: String,
    branch: String,
    namespace: String,
    dir: String,
    is_primary: bool,
    groups: Vec<ApiGroup>,
    /// Daemons in the worktree's namespace that no group lists.
    ungrouped: Vec<ApiDaemonEntry>,
    daemons: ApiDaemonCounts,
    url: String,
}

// ─── builders (pure: no I/O, unit tested) ────────────────────────────────────

/// Daemon entries indexed by qualified id, in listing order.
pub type DaemonIndex = IndexMap<String, ApiDaemonEntry>;

fn counts_for<'a>(entries: impl Iterator<Item = &'a ApiDaemonEntry>) -> ApiDaemonCounts {
    let mut c = ApiDaemonCounts::default();
    for e in entries {
        c.total += 1;
        match e.status_kind() {
            "running" => c.running += 1,
            "available" => c.available += 1,
            "failed" | "errored" => c.failed += 1,
            _ => c.stopped += 1,
        }
    }
    c
}

/// Start time of the most recently started running daemon, RFC 3339.
///
/// Uptime is the only start information the supervisor keeps, so a worktree
/// with nothing running reports `null` rather than a guess.
fn last_activity_for<'a>(entries: impl Iterator<Item = &'a ApiDaemonEntry>) -> Option<String> {
    let youngest = entries.filter_map(|e| e.uptime_secs()).min()?;
    let started = chrono::Local::now() - chrono::Duration::seconds(youngest as i64);
    Some(started.to_rfc3339())
}

fn namespace_entries<'a>(
    daemons: &'a DaemonIndex,
    namespace: &'a str,
) -> impl Iterator<Item = &'a ApiDaemonEntry> {
    daemons
        .values()
        .filter(move |e| e.namespace().eq_ignore_ascii_case(namespace))
}

/// Every namespace a project covers: one per worktree, deduplicated.
fn project_namespaces(project: &ProjectView) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for wt in &project.worktrees {
        if seen.insert(wt.namespace.to_ascii_lowercase()) {
            out.push(wt.namespace.clone());
        }
    }
    out
}

fn project_entries<'a>(daemons: &'a DaemonIndex, project: &ProjectView) -> Vec<&'a ApiDaemonEntry> {
    let namespaces = project_namespaces(project);
    daemons
        .values()
        .filter(|e| {
            namespaces
                .iter()
                .any(|ns| ns.eq_ignore_ascii_case(e.namespace()))
        })
        .collect()
}

pub fn build_project_summaries(
    projects: &[ProjectView],
    daemons: &DaemonIndex,
) -> Vec<ApiProjectSummary> {
    projects
        .iter()
        .map(|p| {
            let entries = project_entries(daemons, p);
            ApiProjectSummary {
                name: p.name.clone(),
                dir: p.dir.to_string_lossy().to_string(),
                worktree_count: p.worktrees.len(),
                daemons: counts_for(entries.iter().copied()),
                last_activity: last_activity_for(entries.iter().copied()),
                url: format!("/projects/{}", p.name),
                api_url: format!("/api/projects/{}", p.name),
            }
        })
        .collect()
}

fn build_worktree_summary(
    project: &ProjectView,
    wt: &WorktreeView,
    daemons: &DaemonIndex,
) -> ApiWorktreeSummary {
    ApiWorktreeSummary {
        name: wt.name.clone(),
        branch: wt.branch.clone(),
        path: wt.path.to_string_lossy().to_string(),
        namespace: wt.namespace.clone(),
        is_primary: wt.is_primary,
        group_count: wt.groups.len(),
        daemons: counts_for(namespace_entries(daemons, &wt.namespace)),
        last_activity: last_activity_for(namespace_entries(daemons, &wt.namespace)),
        url: format!("/projects/{}/{}", project.name, wt.name),
        api_url: format!("/api/projects/{}/{}", project.name, wt.name),
        // Pitchfork does not track a data directory for daemons, so there is
        // nothing to measure. The field stays absent instead of reporting 0.
        disk_usage_bytes: None,
    }
}

/// Groups of a worktree, `default` first, then the rest in config order.
fn ordered_groups(wt: &WorktreeView) -> Vec<(&String, &Vec<DaemonId>)> {
    let mut groups: Vec<(&String, &Vec<DaemonId>)> = wt.groups.iter().collect();
    groups.sort_by_key(|(name, _)| !name.eq_ignore_ascii_case(DEFAULT_GROUP));
    groups
}

pub fn build_stack(project: &ProjectView, wt: &WorktreeView, daemons: &DaemonIndex) -> ApiStack {
    let mut grouped: HashSet<String> = HashSet::new();
    let mut groups = Vec::new();

    for (name, members) in ordered_groups(wt) {
        let mut entries = Vec::new();
        let mut missing = Vec::new();
        for id in members {
            let qualified = id.qualified();
            match daemons.get(&qualified) {
                Some(entry) => {
                    grouped.insert(qualified.clone());
                    entries.push(entry.clone());
                }
                None => missing.push(qualified),
            }
        }
        let counts = counts_for(entries.iter());
        groups.push(ApiGroup {
            name: name.clone(),
            is_default: name.eq_ignore_ascii_case(DEFAULT_GROUP),
            missing,
            running: counts.running,
            total: members.len(),
            daemons: entries,
        });
    }

    let ungrouped: Vec<ApiDaemonEntry> = namespace_entries(daemons, &wt.namespace)
        .filter(|e| !grouped.contains(e.qualified()))
        .cloned()
        .collect();

    ApiStack {
        project: project.name.clone(),
        worktree: wt.name.clone(),
        branch: wt.branch.clone(),
        namespace: wt.namespace.clone(),
        dir: wt.path.to_string_lossy().to_string(),
        is_primary: wt.is_primary,
        groups,
        ungrouped,
        daemons: counts_for(namespace_entries(daemons, &wt.namespace)),
        url: format!("/projects/{}/{}", project.name, wt.name),
    }
}

pub fn build_project(project: &ProjectView, daemons: &DaemonIndex) -> ApiProject {
    let entries = project_entries(daemons, project);
    let primary = project.worktrees.iter().find(|w| w.is_primary);
    ApiProject {
        name: project.name.clone(),
        dir: project.dir.to_string_lossy().to_string(),
        daemons: counts_for(entries.iter().copied()),
        last_activity: last_activity_for(entries.iter().copied()),
        worktrees: project
            .worktrees
            .iter()
            .map(|w| build_worktree_summary(project, w, daemons))
            .collect(),
        stack: primary.map(|w| build_stack(project, w, daemons)),
    }
}

pub fn find_project<'a>(projects: &'a [ProjectView], name: &str) -> Option<&'a ProjectView> {
    projects.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

pub fn find_worktree<'a>(project: &'a ProjectView, name: &str) -> Option<&'a WorktreeView> {
    project
        .worktrees
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case(name) || w.branch.eq_ignore_ascii_case(name))
}

// ─── discovery (I/O) ─────────────────────────────────────────────────────────

/// Worktree discovery spawns `git`/`jj`, so results are cached briefly: the web
/// UI polls these endpoints and a project's worktree set changes rarely.
const DISCOVERY_TTL: Duration = Duration::from_secs(5);

type DiscoveryCache = HashMap<PathBuf, (Instant, Vec<WorktreeEntry>)>;

fn discovery_cache() -> &'static Mutex<DiscoveryCache> {
    static CACHE: std::sync::OnceLock<Mutex<DiscoveryCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn discover_cached(dir: &StdPath) -> Vec<WorktreeEntry> {
    if let Ok(cache) = discovery_cache().lock()
        && let Some((at, entries)) = cache.get(dir)
        && at.elapsed() < DISCOVERY_TTL
    {
        return entries.clone();
    }
    let entries = discover_worktrees(dir);
    if let Ok(mut cache) = discovery_cache().lock() {
        cache.insert(dir.to_path_buf(), (Instant::now(), entries.clone()));
    }
    entries
}

fn canonical(path: &StdPath) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn groups_for_dir(dir: &StdPath) -> IndexMap<String, Vec<DaemonId>> {
    match PitchforkToml::all_merged_from(dir) {
        Ok(config) => config
            .groups
            .into_iter()
            .map(|(name, group)| (name, group.daemons))
            .collect(),
        Err(e) => {
            log::warn!("Failed to load config for {}: {e}", dir.display());
            IndexMap::new()
        }
    }
}

fn worktree_views(project_name: &str, project_dir: &StdPath) -> Vec<WorktreeView> {
    let project_canonical = canonical(project_dir);
    let discovered = discover_cached(project_dir);

    if discovered.is_empty() {
        return vec![WorktreeView {
            name: DEFAULT_WORKTREE.to_string(),
            branch: DEFAULT_WORKTREE.to_string(),
            namespace: PitchforkToml::namespace_for_dir(project_dir)
                .unwrap_or_else(|_| project_name.to_string()),
            groups: groups_for_dir(project_dir),
            path: project_dir.to_path_buf(),
            is_primary: true,
        }];
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut views = Vec::with_capacity(discovered.len());
    for wt in discovered {
        let key = wt.sanitized_branch.to_ascii_lowercase();
        if !seen.insert(key) {
            log::warn!(
                "Skipping worktree '{}' of project '{project_name}': its URL name collides with another worktree",
                wt.branch
            );
            continue;
        }
        let namespace =
            PitchforkToml::namespace_for_dir(&wt.path).unwrap_or_else(|_| project_name.to_string());
        views.push(WorktreeView {
            name: wt.sanitized_branch.clone(),
            branch: wt.branch.clone(),
            is_primary: canonical(&wt.path) == project_canonical,
            groups: groups_for_dir(&wt.path),
            namespace,
            path: wt.path,
        });
    }
    views
}

/// Whether `dir` is a repository's main checkout rather than a linked
/// worktree. A linked git worktree has a `.git` *file* pointing at the common
/// git directory; a secondary jj workspace has a `.jj/repo` file pointing at
/// the main repository. A directory that is neither is its own root.
fn is_main_checkout(dir: &StdPath) -> bool {
    let jj = dir.join(".jj");
    if jj.exists() {
        return jj.join("repo").is_dir();
    }
    let git = dir.join(".git");
    if git.exists() {
        return git.is_dir();
    }
    true
}

/// Build the project views from the global namespace registry.
///
/// A namespace registered on a linked worktree whose main checkout is also
/// registered is listed only as that checkout's worktree, never as a project of
/// its own. Registering only the worktree keeps it a project in its own right.
fn collect_project_views_blocking() -> Vec<ProjectView> {
    let registry = PitchforkToml::read_global_namespaces();

    let mut projects: Vec<(ProjectView, Option<PathBuf>)> = registry
        .into_iter()
        .map(|(name, entry)| {
            let worktrees = worktree_views(&name, &entry.dir);
            let main = worktrees
                .iter()
                .find(|w| is_main_checkout(&w.path))
                .map(|w| canonical(&w.path));
            (
                ProjectView {
                    name,
                    dir: entry.dir,
                    worktrees,
                },
                main,
            )
        })
        .collect();

    let registered: HashSet<PathBuf> = projects.iter().map(|(p, _)| canonical(&p.dir)).collect();

    projects.retain(|(p, main)| match main {
        Some(main) => canonical(&p.dir) == *main || !registered.contains(main),
        None => true,
    });

    let mut projects: Vec<ProjectView> = projects.into_iter().map(|(p, _)| p).collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects
}

async fn collect_project_views() -> Vec<ProjectView> {
    tokio::task::spawn_blocking(collect_project_views_blocking)
        .await
        .unwrap_or_else(|e| {
            log::error!("Failed to collect projects: {e}");
            Vec::new()
        })
}

/// Live daemon state, indexed by qualified id.
///
/// Daemons declared by a worktree's config but unknown to the supervisor are
/// added as available entries, so a worktree that has never run still shows its
/// stack.
async fn daemon_index(extra_dirs: &[PathBuf]) -> Result<DaemonIndex, StatusCode> {
    let mut index: DaemonIndex = build_api_daemons()
        .await
        .map_err(|e| {
            log::error!("Failed to list daemons: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .into_iter()
        .map(|e| (e.qualified().to_string(), e))
        .collect();

    for dir in extra_dirs {
        let dir = dir.clone();
        let configured =
            tokio::task::spawn_blocking(move || match PitchforkToml::all_merged_from(&dir) {
                Ok(config) => config.daemons.into_iter().collect::<Vec<_>>(),
                Err(e) => {
                    log::warn!("Failed to load config for {}: {e}", dir.display());
                    Vec::new()
                }
            })
            .await
            .unwrap_or_default();

        for (id, daemon_config) in configured {
            let qualified = id.qualified();
            if index.contains_key(&qualified) {
                continue;
            }
            index.insert(qualified, config_daemon_entry(&id, &daemon_config));
        }
    }

    Ok(index)
}

// ─── handlers ────────────────────────────────────────────────────────────────

pub async fn list() -> Result<Json<Vec<ApiProjectSummary>>, StatusCode> {
    let projects = collect_project_views().await;
    // Include every worktree's own config so the counts here match the ones
    // the project page shows, even for worktrees with no registered namespace.
    let dirs: Vec<PathBuf> = projects
        .iter()
        .flat_map(|p| p.worktrees.iter().map(|w| w.path.clone()))
        .collect();
    let daemons = daemon_index(&dirs).await?;
    Ok(Json(build_project_summaries(&projects, &daemons)))
}

pub async fn show(Path(name): Path<String>) -> Result<Json<ApiProject>, StatusCode> {
    let projects = collect_project_views().await;
    let project = find_project(&projects, &name)
        .ok_or(StatusCode::NOT_FOUND)?
        .clone();
    let dirs: Vec<PathBuf> = project.worktrees.iter().map(|w| w.path.clone()).collect();
    let daemons = daemon_index(&dirs).await?;
    Ok(Json(build_project(&project, &daemons)))
}

pub async fn stack(
    Path((name, worktree)): Path<(String, String)>,
) -> Result<Json<ApiStack>, StatusCode> {
    let projects = collect_project_views().await;
    let project = find_project(&projects, &name).ok_or(StatusCode::NOT_FOUND)?;
    let wt = find_worktree(project, &worktree).ok_or(StatusCode::NOT_FOUND)?;
    let daemons = daemon_index(std::slice::from_ref(&wt.path)).await?;
    Ok(Json(build_stack(project, wt, &daemons)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::routes::api::daemons::ApiDaemonStatus;

    fn daemon(qualified: &str, status: ApiDaemonStatus, uptime: Option<u64>) -> ApiDaemonEntry {
        ApiDaemonEntry::stub(qualified, status, uptime)
    }

    fn index(entries: Vec<ApiDaemonEntry>) -> DaemonIndex {
        entries
            .into_iter()
            .map(|e| (e.qualified().to_string(), e))
            .collect()
    }

    fn group(members: &[&str]) -> Vec<DaemonId> {
        members
            .iter()
            .map(|m| DaemonId::parse(m).unwrap())
            .collect()
    }

    /// Two projects: `shop` with a primary checkout and a `feature-a`
    /// worktree that declares a `default` group, and `blog` with a single
    /// checkout and no groups.
    fn fixture_projects() -> Vec<ProjectView> {
        let shop_primary = WorktreeView {
            name: "main".into(),
            branch: "main".into(),
            path: PathBuf::from("/src/shop"),
            namespace: "shop".into(),
            is_primary: true,
            groups: IndexMap::from([("web".to_string(), group(&["shop/api", "shop/frontend"]))]),
        };
        let shop_feature = WorktreeView {
            name: "feature-a".into(),
            branch: "feature/a".into(),
            path: PathBuf::from("/src/shop-feature-a"),
            namespace: "shop-feature-a".into(),
            is_primary: false,
            groups: IndexMap::from([
                ("workers".to_string(), group(&["shop-feature-a/worker"])),
                (
                    "default".to_string(),
                    group(&["shop-feature-a/api", "shop-feature-a/gone"]),
                ),
            ]),
        };
        let blog = WorktreeView {
            name: "default".into(),
            branch: "default".into(),
            path: PathBuf::from("/src/blog"),
            namespace: "blog".into(),
            is_primary: true,
            groups: IndexMap::new(),
        };
        vec![
            ProjectView {
                name: "blog".into(),
                dir: PathBuf::from("/src/blog"),
                worktrees: vec![blog],
            },
            ProjectView {
                name: "shop".into(),
                dir: PathBuf::from("/src/shop"),
                worktrees: vec![shop_primary, shop_feature],
            },
        ]
    }

    fn fixture_daemons() -> DaemonIndex {
        index(vec![
            daemon("shop/api", ApiDaemonStatus::Running, Some(120)),
            daemon("shop/frontend", ApiDaemonStatus::Stopped, None),
            daemon("shop-feature-a/api", ApiDaemonStatus::Running, Some(30)),
            daemon("shop-feature-a/worker", ApiDaemonStatus::Available, None),
            daemon("shop-feature-a/extra", ApiDaemonStatus::Stopped, None),
            daemon("blog/site", ApiDaemonStatus::Available, None),
        ])
    }

    #[test]
    fn projects_endpoint_lists_every_project_with_counts() {
        let summaries = build_project_summaries(&fixture_projects(), &fixture_daemons());
        let json = serde_json::to_value(&summaries).unwrap();

        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["name"], "blog");
        assert_eq!(json[0]["worktree_count"], 1);
        assert_eq!(json[0]["daemons"]["available"], 1);
        assert_eq!(json[0]["url"], "/projects/blog");

        // `shop` spans both worktree namespaces.
        assert_eq!(json[1]["name"], "shop");
        assert_eq!(json[1]["worktree_count"], 2);
        assert_eq!(json[1]["daemons"]["total"], 5);
        assert_eq!(json[1]["daemons"]["running"], 2);
        assert_eq!(json[1]["daemons"]["stopped"], 2);
        assert_eq!(json[1]["daemons"]["available"], 1);
        assert!(json[1]["last_activity"].is_string());
    }

    #[test]
    fn projects_endpoint_reports_no_activity_when_nothing_runs() {
        let projects = fixture_projects();
        let daemons = index(vec![daemon("blog/site", ApiDaemonStatus::Stopped, None)]);
        let summaries = build_project_summaries(&projects, &daemons);
        assert!(summaries[0].last_activity.is_none());
    }

    #[test]
    fn project_endpoint_lists_worktrees_and_primary_stack() {
        let projects = fixture_projects();
        let project = find_project(&projects, "shop").unwrap();
        let json = serde_json::to_value(build_project(project, &fixture_daemons())).unwrap();

        let worktrees = json["worktrees"].as_array().unwrap();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0]["name"], "main");
        assert_eq!(worktrees[0]["is_primary"], true);
        assert_eq!(worktrees[0]["daemons"]["running"], 1);
        assert_eq!(worktrees[1]["name"], "feature-a");
        assert_eq!(worktrees[1]["branch"], "feature/a");
        assert_eq!(worktrees[1]["namespace"], "shop-feature-a");
        assert_eq!(worktrees[1]["url"], "/projects/shop/feature-a");
        // Never-started worktrees are listed with their daemons.
        assert_eq!(worktrees[1]["daemons"]["total"], 3);
        // Disk usage is unknown, so the field is absent rather than zero.
        assert!(worktrees[1].get("disk_usage_bytes").is_none());

        // The primary checkout's stack is served with the project.
        assert_eq!(json["stack"]["worktree"], "main");
        assert_eq!(json["stack"]["groups"][0]["name"], "web");
    }

    #[test]
    fn project_lookup_is_case_insensitive_and_reports_unknown_names() {
        let projects = fixture_projects();
        assert!(find_project(&projects, "SHOP").is_some());
        assert!(find_project(&projects, "nope").is_none());

        let project = find_project(&projects, "shop").unwrap();
        assert!(find_worktree(project, "FEATURE-A").is_some());
        // The unsanitized branch name resolves too.
        assert!(find_worktree(project, "feature/a").is_some());
        assert!(find_worktree(project, "missing").is_none());
    }

    #[test]
    fn stack_endpoint_puts_default_group_first_and_flags_missing_members() {
        let projects = fixture_projects();
        let project = find_project(&projects, "shop").unwrap();
        let wt = find_worktree(project, "feature-a").unwrap();
        let json = serde_json::to_value(build_stack(project, wt, &fixture_daemons())).unwrap();

        assert_eq!(json["project"], "shop");
        assert_eq!(json["worktree"], "feature-a");
        assert_eq!(json["namespace"], "shop-feature-a");
        assert_eq!(json["is_primary"], false);

        let groups = json["groups"].as_array().unwrap();
        assert_eq!(groups[0]["name"], "default");
        assert_eq!(groups[0]["is_default"], true);
        assert_eq!(groups[0]["total"], 2);
        assert_eq!(groups[0]["running"], 1);
        // A group member with no matching daemon is reported, not dropped.
        assert_eq!(groups[0]["missing"][0], "shop-feature-a/gone");
        assert_eq!(
            groups[0]["daemons"][0]["id"]["qualified"],
            "shop-feature-a/api"
        );
        assert_eq!(groups[1]["name"], "workers");
        assert_eq!(groups[1]["is_default"], false);

        // Namespace daemons no group lists stay visible.
        let ungrouped = json["ungrouped"].as_array().unwrap();
        assert_eq!(ungrouped.len(), 1);
        assert_eq!(ungrouped[0]["id"]["qualified"], "shop-feature-a/extra");
    }

    #[test]
    fn stack_endpoint_serves_a_worktree_without_groups() {
        let projects = fixture_projects();
        let project = find_project(&projects, "blog").unwrap();
        let wt = find_worktree(project, "default").unwrap();
        let stack = build_stack(project, wt, &fixture_daemons());

        assert!(stack.groups.is_empty());
        assert_eq!(stack.ungrouped.len(), 1);
        assert_eq!(
            stack.daemons,
            ApiDaemonCounts {
                total: 1,
                available: 1,
                ..Default::default()
            }
        );
    }
}
