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
    /// Namespace daemons declared here would carry, or `None` when none can be
    /// derived. Borrowing the project's namespace would let this worktree list
    /// and control the primary checkout's daemons.
    pub namespace: Option<String>,
    /// Why the namespace could not be derived, when it could not be.
    pub namespace_error: Option<String>,
    pub is_primary: bool,
    /// False when the directory is gone, e.g. a checkout deleted while its
    /// `[namespaces]` entry stayed behind.
    pub dir_exists: bool,
    pub groups: IndexMap<String, Vec<DaemonId>>,
    /// Error from loading this worktree's config, if it could not be read.
    /// A syntax error otherwise shows up as a stack with no groups at all.
    pub config_error: Option<String>,
}

/// A project: a registered namespace plus its worktrees.
#[derive(Debug, Clone)]
pub struct ProjectView {
    pub name: String,
    pub dir: PathBuf,
    /// False when the registered directory is gone, e.g. a deleted checkout
    /// whose `[namespaces]` entry was never removed.
    pub dir_exists: bool,
    pub worktrees: Vec<WorktreeView>,
}

// ─── response types ──────────────────────────────────────────────────────────

#[derive(Serialize, Default, PartialEq, Debug)]
pub struct ApiDaemonCounts {
    total: usize,
    running: usize,
    stopped: usize,
    /// Oneshot daemons that ran and exited successfully.
    completed: usize,
    /// Daemons on their way up or down (`waiting`, `stopping`), which are
    /// neither running nor stopped yet.
    transitioning: usize,
    failed: usize,
    available: usize,
}

#[derive(Serialize)]
pub struct ApiProjectSummary {
    name: String,
    dir: String,
    worktree_count: usize,
    /// False when the registered directory no longer exists.
    dir_exists: bool,
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
    namespace: Option<String>,
    /// Why no namespace could be derived for this worktree, when none could.
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace_error: Option<String>,
    is_primary: bool,
    /// False when the supervisor cannot resolve config for some of this
    /// worktree's daemons, so starting them would fail.
    can_start: bool,
    /// False when the worktree directory no longer exists.
    dir_exists: bool,
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
    dir_exists: bool,
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
    namespace: Option<String>,
    /// Why no namespace could be derived for this worktree, when none could.
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace_error: Option<String>,
    dir: String,
    is_primary: bool,
    /// False when `unresolvable_daemons` is non-empty.
    can_start: bool,
    /// Daemons this stack shows, in its own namespace or named by one of its
    /// groups, that the supervisor cannot resolve a config for. It loads
    /// configs from its own project and from the namespace registry, so a
    /// worktree in neither is visible but not startable until registered.
    unresolvable_daemons: Vec<String>,
    /// False when the worktree directory no longer exists.
    dir_exists: bool,
    /// Why this worktree's config could not be read, when it could not be.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_error: Option<String>,
    groups: Vec<ApiGroup>,
    /// Daemons in the worktree's namespace that no group lists.
    ungrouped: Vec<ApiDaemonEntry>,
    daemons: ApiDaemonCounts,
    url: String,
}

// ─── builders (pure: no I/O, unit tested) ────────────────────────────────────

/// Daemon entries indexed by qualified id, in listing order.
pub type DaemonIndex = IndexMap<String, ApiDaemonEntry>;

/// Qualified ids the supervisor can resolve a config for.
///
/// This is what the web control endpoints require: they call
/// `IpcClient::start_daemon`, which looks the daemon up in the merged config
/// and fails with "Daemon config not found" otherwise. A daemon already
/// running does not help, because restart stops it before that lookup.
pub type Resolvable = HashSet<String>;

/// Daemons of this worktree the web endpoints could not start or restart.
/// Daemons this worktree's stack shows that the web endpoints could not start
/// or restart: the worktree's own namespace, plus every group member the stack
/// renders. A group can name daemons of other namespaces, and those need the
/// same check, or the page would offer a Restart that stops a daemon and then
/// fails to bring it back.
fn unresolvable_for(
    daemons: &DaemonIndex,
    wt: &WorktreeView,
    resolvable: &Resolvable,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let mut consider = |qualified: &str| {
        if !resolvable.contains(qualified) && seen.insert(qualified.to_string()) {
            out.push(qualified.to_string());
        }
    };

    for entry in namespace_entries(daemons, wt.namespace.as_deref()) {
        consider(entry.qualified());
    }
    for id in wt.groups.values().flatten() {
        let qualified = id.qualified();
        // Members no daemon matches are reported as `missing` instead.
        if daemons.contains_key(&qualified) {
            consider(&qualified);
        }
    }

    out
}

fn counts_for<'a>(entries: impl Iterator<Item = &'a ApiDaemonEntry>) -> ApiDaemonCounts {
    let mut c = ApiDaemonCounts::default();
    for e in entries {
        c.total += 1;
        match e.status_kind() {
            "running" => c.running += 1,
            "available" => c.available += 1,
            "failed" | "errored" => c.failed += 1,
            "completed" => c.completed += 1,
            "waiting" | "stopping" => c.transitioning += 1,
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

/// Entries of one namespace; none at all when the worktree has no namespace.
fn namespace_entries<'a>(
    daemons: &'a DaemonIndex,
    namespace: Option<&'a str>,
) -> impl Iterator<Item = &'a ApiDaemonEntry> {
    daemons
        .values()
        .filter(move |e| namespace.is_some_and(|ns| e.namespace().eq_ignore_ascii_case(ns)))
}

/// Every namespace a project covers: one per worktree, deduplicated.
fn project_namespaces(project: &ProjectView) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for wt in project.worktrees.iter() {
        let Some(namespace) = wt.namespace.as_deref() else {
            continue;
        };
        if seen.insert(namespace.to_ascii_lowercase()) {
            out.push(namespace.to_string());
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
                dir_exists: p.dir_exists,
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
    resolvable: &Resolvable,
) -> ApiWorktreeSummary {
    ApiWorktreeSummary {
        name: wt.name.clone(),
        branch: wt.branch.clone(),
        path: wt.path.to_string_lossy().to_string(),
        namespace: wt.namespace.clone(),
        namespace_error: wt.namespace_error.clone(),
        is_primary: wt.is_primary,
        can_start: wt.dir_exists && unresolvable_for(daemons, wt, resolvable).is_empty(),
        dir_exists: wt.dir_exists,
        group_count: wt.groups.len(),
        daemons: counts_for(namespace_entries(daemons, wt.namespace.as_deref())),
        last_activity: last_activity_for(namespace_entries(daemons, wt.namespace.as_deref())),
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

pub fn build_stack(
    project: &ProjectView,
    wt: &WorktreeView,
    daemons: &DaemonIndex,
    resolvable: &Resolvable,
) -> ApiStack {
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

    let unresolvable = unresolvable_for(daemons, wt, resolvable);
    // A registration left behind by a deleted checkout must not look healthy.
    let dir_exists = wt.dir_exists;

    let ungrouped: Vec<ApiDaemonEntry> = namespace_entries(daemons, wt.namespace.as_deref())
        .filter(|e| !grouped.contains(e.qualified()))
        .cloned()
        .collect();

    ApiStack {
        project: project.name.clone(),
        worktree: wt.name.clone(),
        branch: wt.branch.clone(),
        namespace: wt.namespace.clone(),
        namespace_error: wt.namespace_error.clone(),
        dir: wt.path.to_string_lossy().to_string(),
        is_primary: wt.is_primary,
        can_start: dir_exists && unresolvable.is_empty(),
        unresolvable_daemons: unresolvable,
        dir_exists,
        config_error: wt.config_error.clone(),
        groups,
        ungrouped,
        daemons: counts_for(namespace_entries(daemons, wt.namespace.as_deref())),
        url: format!("/projects/{}/{}", project.name, wt.name),
    }
}

pub fn build_project(
    project: &ProjectView,
    daemons: &DaemonIndex,
    resolvable: &Resolvable,
) -> ApiProject {
    let entries = project_entries(daemons, project);
    let primary = project.worktrees.iter().find(|w| w.is_primary);
    ApiProject {
        name: project.name.clone(),
        dir: project.dir.to_string_lossy().to_string(),
        dir_exists: project.dir_exists,
        daemons: counts_for(entries.iter().copied()),
        last_activity: last_activity_for(entries.iter().copied()),
        worktrees: project
            .worktrees
            .iter()
            .map(|w| build_worktree_summary(project, w, daemons, resolvable))
            .collect(),
        stack: primary.map(|w| build_stack(project, w, daemons, resolvable)),
    }
}

pub fn find_project<'a>(projects: &'a [ProjectView], name: &str) -> Option<&'a ProjectView> {
    projects.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// Look a worktree up by its URL name, falling back to its branch name.
///
/// URL names win, so a branch whose sanitized name collided and was suffixed
/// (`feature-api-2`) is still reachable at its own URL rather than resolving to
/// whichever worktree claimed the base name.
pub fn find_worktree<'a>(project: &'a ProjectView, name: &str) -> Option<&'a WorktreeView> {
    project
        .worktrees
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case(name))
        .or_else(|| {
            project
                .worktrees
                .iter()
                .find(|w| w.branch.eq_ignore_ascii_case(name))
        })
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

/// Groups declared by the config loaded for `dir`, plus the error when that
/// config could not be read: a syntax error would otherwise be indistinguishable
/// from a worktree that declares no groups.
fn groups_for_dir(dir: &StdPath) -> (IndexMap<String, Vec<DaemonId>>, Option<String>) {
    match PitchforkToml::all_merged_from(dir) {
        Ok(config) => (
            config
                .groups
                .into_iter()
                .map(|(name, group)| (name, group.daemons))
                .collect(),
            None,
        ),
        Err(e) => {
            log::warn!("Failed to load config for {}: {e}", dir.display());
            (IndexMap::new(), Some(e.to_string()))
        }
    }
}

/// A URL name for a worktree that no other worktree of this project uses.
///
/// Distinct branches can sanitize to the same name (`feature/api` and
/// `feature-api` both give `feature-api`), so later collisions get a numeric
/// suffix instead of being dropped. Discovery order is stable, so the names
/// are too.
fn unique_name(base: &str, taken: &mut HashSet<String>) -> String {
    let base = if base.is_empty() {
        DEFAULT_WORKTREE
    } else {
        base
    };
    let mut candidate = base.to_string();
    let mut n = 2;
    while !taken.insert(candidate.to_ascii_lowercase()) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

/// The namespace daemons declared in this worktree would carry.
///
/// `namespace_for_dir` answers `global` when the worktree has no config file of
/// its own, because the user config is then the nearest one. Attributing the
/// worktree to `global` would show every global daemon under it, so fall back
/// to the name the directory itself would produce.
///
/// That fallback can fail, for instance when the directory name is not a valid
/// namespace. The worktree then has no namespace at all rather than borrowing
/// the project's, which would let it list and control the primary checkout's
/// daemons.
fn namespace_for_worktree(path: &StdPath) -> Result<String, String> {
    match PitchforkToml::namespace_for_dir(path) {
        Ok(ns) if ns != "global" => Ok(ns),
        _ => PitchforkToml::namespace_for_project_dir(path).map_err(|e| e.to_string()),
    }
}

fn worktree_view_for(
    path: PathBuf,
    branch: String,
    name: String,
    project_canonical: &StdPath,
) -> WorktreeView {
    let (namespace, namespace_error) = match namespace_for_worktree(&path) {
        Ok(ns) => (Some(ns), None),
        Err(e) => {
            log::warn!("No namespace for worktree {}: {e}", path.display());
            (None, Some(e))
        }
    };
    let (groups, config_error) = groups_for_dir(&path);
    WorktreeView {
        is_primary: canonical(&path) == project_canonical,
        dir_exists: path.exists(),
        groups,
        config_error,
        namespace,
        namespace_error,
        name,
        branch,
        path,
    }
}

fn worktree_views(project_dir: &StdPath) -> Vec<WorktreeView> {
    let project_canonical = canonical(project_dir);
    // Discover from the main checkout: `jj workspace list` resolved from a
    // secondary workspace reports `default` as the directory it ran in, which
    // would hide the real main workspace.
    let discovery_dir =
        main_checkout_root(project_dir).unwrap_or_else(|| project_dir.to_path_buf());
    let discovered = discover_cached(&discovery_dir);

    let mut taken: HashSet<String> = HashSet::new();
    let mut views = Vec::with_capacity(discovered.len() + 1);
    for wt in discovered {
        let name = unique_name(&wt.sanitized_branch, &mut taken);
        views.push(worktree_view_for(
            wt.path,
            wt.branch,
            name,
            &project_canonical,
        ));
    }

    // Discovery only reports checkouts that are on a branch, so a detached
    // HEAD checkout is missing from it. The project's own directory must still
    // appear, otherwise the project has no primary worktree and no stack.
    if !views.iter().any(|w| w.is_primary) {
        let name = unique_name(DEFAULT_WORKTREE, &mut taken);
        views.insert(
            0,
            worktree_view_for(
                project_dir.to_path_buf(),
                DEFAULT_WORKTREE.to_string(),
                name,
                &project_canonical,
            ),
        );
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

/// The main checkout of the repository `dir` belongs to, when `dir` is a
/// linked git worktree or a secondary jj workspace.
///
/// Both record where the real repository lives: `.git` is a file reading
/// `gitdir: <main>/.git/worktrees/<name>`, and `.jj/repo` is a file holding the
/// path of `<main>/.jj/repo`.
fn main_checkout_root(dir: &StdPath) -> Option<PathBuf> {
    let jj_repo = dir.join(".jj").join("repo");
    if jj_repo.is_file() {
        let target = std::fs::read_to_string(&jj_repo).ok()?;
        let target = PathBuf::from(target.trim());
        let target = if target.is_absolute() {
            target
        } else {
            dir.join(".jj").join(target)
        };
        // <main>/.jj/repo → <main>
        return target.parent()?.parent().map(StdPath::to_path_buf);
    }

    let git = dir.join(".git");
    if git.is_file() {
        let content = std::fs::read_to_string(&git).ok()?;
        let gitdir = content.trim().strip_prefix("gitdir:")?.trim();
        let gitdir = PathBuf::from(gitdir);
        let gitdir = if gitdir.is_absolute() {
            gitdir
        } else {
            dir.join(gitdir)
        };
        // Only a linked worktree points at <main>/.git/worktrees/<name>. A
        // submodule's .git file points at <super>/.git/modules/<name>, which
        // must not make the superproject its main checkout.
        if !gitdir.components().any(|c| c.as_os_str() == "worktrees") {
            return None;
        }
        let mut current = gitdir.as_path();
        loop {
            let parent = current.parent()?;
            if current.file_name().is_some_and(|name| name == ".git") {
                return Some(parent.to_path_buf());
            }
            current = parent;
        }
    }

    None
}

/// Build the project views from the global namespace registry.
///
/// A namespace registered on a linked worktree whose main checkout is also
/// registered is listed only as that checkout's worktree, never as a project of
/// its own. Registering only the worktree keeps it a project in its own right.
fn collect_project_views_blocking() -> Vec<ProjectView> {
    let registry = PitchforkToml::read_global_namespaces();

    let mut projects: Vec<(ProjectView, Option<PathBuf>)> = registry
        .iter()
        .map(|(name, entry)| {
            let worktrees = worktree_views(&entry.dir);
            // Where this project's repository really lives, so a registration
            // that points at a linked worktree is folded into its checkout.
            let main = main_checkout_root(&entry.dir)
                .map(|r| canonical(&r))
                .or_else(|| {
                    worktrees
                        .iter()
                        .find(|w| is_main_checkout(&w.path))
                        .map(|w| canonical(&w.path))
                });
            (
                ProjectView {
                    name: name.clone(),
                    dir_exists: entry.dir.exists(),
                    dir: entry.dir.clone(),
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

/// Live daemon state indexed by qualified id, plus the ids the supervisor can
/// resolve a config for.
///
/// Daemons declared by a worktree's config but unknown to the supervisor are
/// added as available entries, so a worktree that has never run still shows its
/// stack. Every config read here happens on a blocking worker: parsing takes
/// the global config lock, which must not run on the async executor.
async fn daemon_index(extra_dirs: &[PathBuf]) -> Result<(DaemonIndex, Resolvable), StatusCode> {
    let mut index: DaemonIndex = build_api_daemons()
        .await
        .map_err(|e| {
            log::error!("Failed to list daemons: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .into_iter()
        .map(|e| (e.qualified().to_string(), e))
        .collect();

    let dirs = extra_dirs.to_vec();
    let (extra, resolvable) = tokio::task::spawn_blocking(move || config_entries_blocking(&dirs))
        .await
        .map_err(|e| {
            log::error!("Failed to load worktree configs: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    for (qualified, entry) in extra {
        index.entry(qualified).or_insert(entry);
    }

    Ok((index, resolvable))
}

/// Read every worktree config and the supervisor's own resolvable set.
///
/// Runs entirely on a blocking worker: it parses config files and reads the
/// global slug registry, which lock and hit the filesystem.
fn config_entries_blocking(dirs: &[PathBuf]) -> (Vec<(String, ApiDaemonEntry)>, Resolvable) {
    // The same view the supervisor's start path builds, so "resolvable" here
    // means exactly "a start request would find a config".
    let resolvable: Resolvable = match PitchforkToml::all_merged_all_namespaces() {
        Ok(config) => config.daemons.keys().map(|id| id.qualified()).collect(),
        Err(e) => {
            log::warn!("Failed to load merged config: {e}");
            Resolvable::new()
        }
    };

    let slugs = PitchforkToml::read_global_slugs();
    let settings = crate::settings::settings();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for dir in dirs {
        match PitchforkToml::all_merged_from(dir) {
            Ok(config) => {
                for (id, daemon_config) in &config.daemons {
                    let qualified = id.qualified();
                    if seen.insert(qualified.clone()) {
                        entries.push((
                            qualified,
                            config_daemon_entry(id, daemon_config, &slugs, &settings),
                        ));
                    }
                }
            }
            Err(e) => log::warn!("Failed to load config for {}: {e}", dir.display()),
        }
    }

    (entries, resolvable)
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
    let (daemons, _) = daemon_index(&dirs).await?;
    Ok(Json(build_project_summaries(&projects, &daemons)))
}

pub async fn show(Path(name): Path<String>) -> Result<Json<ApiProject>, StatusCode> {
    let projects = collect_project_views().await;
    let project = find_project(&projects, &name)
        .ok_or(StatusCode::NOT_FOUND)?
        .clone();
    let dirs: Vec<PathBuf> = project.worktrees.iter().map(|w| w.path.clone()).collect();
    let (daemons, resolvable) = daemon_index(&dirs).await?;
    Ok(Json(build_project(&project, &daemons, &resolvable)))
}

pub async fn stack(
    Path((name, worktree)): Path<(String, String)>,
) -> Result<Json<ApiStack>, StatusCode> {
    let projects = collect_project_views().await;
    let project = find_project(&projects, &name).ok_or(StatusCode::NOT_FOUND)?;
    let wt = find_worktree(project, &worktree).ok_or(StatusCode::NOT_FOUND)?;
    let (daemons, resolvable) = daemon_index(std::slice::from_ref(&wt.path)).await?;
    Ok(Json(build_stack(project, wt, &daemons, &resolvable)))
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

    /// Every daemon in the fixture is resolvable unless a test says otherwise.
    fn all_resolvable(daemons: &DaemonIndex) -> Resolvable {
        daemons.keys().cloned().collect()
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
            namespace: Some("shop".into()),
            namespace_error: None,
            is_primary: true,
            dir_exists: true,
            groups: IndexMap::from([("web".to_string(), group(&["shop/api", "shop/frontend"]))]),
            config_error: None,
        };
        let shop_feature = WorktreeView {
            name: "feature-a".into(),
            branch: "feature/a".into(),
            path: PathBuf::from("/src/shop-feature-a"),
            namespace: Some("shop-feature-a".into()),
            namespace_error: None,
            is_primary: false,
            dir_exists: true,
            groups: IndexMap::from([
                ("workers".to_string(), group(&["shop-feature-a/worker"])),
                (
                    "default".to_string(),
                    group(&["shop-feature-a/api", "shop-feature-a/gone"]),
                ),
            ]),
            config_error: None,
        };
        let blog = WorktreeView {
            name: "default".into(),
            branch: "default".into(),
            path: PathBuf::from("/src/blog"),
            namespace: Some("blog".into()),
            namespace_error: None,
            is_primary: true,
            dir_exists: true,
            groups: IndexMap::new(),
            config_error: None,
        };
        vec![
            ProjectView {
                name: "blog".into(),
                dir: PathBuf::from("/src/blog"),
                dir_exists: true,
                worktrees: vec![blog],
            },
            ProjectView {
                name: "shop".into(),
                dir: PathBuf::from("/src/shop"),
                dir_exists: true,
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
        let daemons = fixture_daemons();
        let json =
            serde_json::to_value(build_project(project, &daemons, &all_resolvable(&daemons)))
                .unwrap();

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
        let daemons = fixture_daemons();
        let json = serde_json::to_value(build_stack(
            project,
            wt,
            &daemons,
            &all_resolvable(&daemons),
        ))
        .unwrap();

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

    /// Daemons the supervisor has neither a config nor a saved command for are
    /// listed, but the stack reports that it cannot start them, so the UI does
    /// not offer an action that would fail with "Daemon config not found".
    #[test]
    fn stack_reports_daemons_the_supervisor_cannot_resolve() {
        let projects = fixture_projects();
        let project = find_project(&projects, "shop").unwrap();
        let daemons = fixture_daemons();

        // Only the primary worktree's namespace is resolvable here.
        let resolvable: Resolvable = daemons
            .keys()
            .filter(|id| id.starts_with("shop/"))
            .cloned()
            .collect();

        let json = serde_json::to_value(build_project(project, &daemons, &resolvable)).unwrap();
        assert_eq!(json["worktrees"][0]["can_start"], true);
        assert_eq!(json["worktrees"][1]["can_start"], false);

        let wt = find_worktree(project, "feature-a").unwrap();
        let stack = serde_json::to_value(build_stack(project, wt, &daemons, &resolvable)).unwrap();
        assert_eq!(stack["can_start"], false);
        let unresolvable = stack["unresolvable_daemons"].as_array().unwrap();
        // Every daemon of the worktree is reported, running or not: the web
        // endpoints need a config for all of them.
        assert!(unresolvable.contains(&serde_json::json!("shop-feature-a/worker")));
        assert!(unresolvable.contains(&serde_json::json!("shop-feature-a/api")));
    }

    #[test]
    fn colliding_branch_names_get_distinct_urls() {
        let mut taken = HashSet::new();
        // `feature/api` and `feature-api` both sanitize to `feature-api`.
        assert_eq!(unique_name("feature-api", &mut taken), "feature-api");
        assert_eq!(unique_name("feature-api", &mut taken), "feature-api-2");
        assert_eq!(unique_name("feature-api", &mut taken), "feature-api-3");
        // Matching is case-insensitive, like host names, so this collides too
        // and continues the sequence.
        assert_eq!(unique_name("Feature-Api", &mut taken), "Feature-Api-4");
    }

    /// A detached HEAD checkout is missing from worktree discovery, which
    /// reports only branches. The project directory must still be listed, or
    /// the project would have no primary worktree and no stack.
    #[test]
    fn detached_primary_checkout_is_still_listed() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join(".git").join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();

        let views = worktree_views(&repo);
        assert_eq!(views.len(), 1);
        assert!(views[0].is_primary);
        assert_eq!(views[0].name, DEFAULT_WORKTREE);
    }

    #[test]
    fn main_checkout_root_reads_linked_git_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("repo");
        let linked = temp.path().join("repo-feature");
        std::fs::create_dir_all(main.join(".git").join("worktrees").join("feature")).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!(
                "gitdir: {}\n",
                main.join(".git/worktrees/feature").display()
            ),
        )
        .unwrap();

        assert_eq!(main_checkout_root(&linked), Some(main.clone()));
        // The main checkout itself is not a linked worktree.
        assert_eq!(main_checkout_root(&main), None);
        assert!(is_main_checkout(&main));
        assert!(!is_main_checkout(&linked));
    }

    #[test]
    fn main_checkout_root_reads_secondary_jj_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("repo");
        let secondary = temp.path().join("repo-ws");
        std::fs::create_dir_all(main.join(".jj").join("repo")).unwrap();
        std::fs::create_dir_all(secondary.join(".jj")).unwrap();
        std::fs::write(
            secondary.join(".jj").join("repo"),
            main.join(".jj/repo").display().to_string(),
        )
        .unwrap();

        assert_eq!(main_checkout_root(&secondary), Some(main.clone()));
        assert_eq!(main_checkout_root(&main), None);
        assert!(!is_main_checkout(&secondary));
    }

    /// A running daemon whose config the supervisor cannot resolve is still
    /// unresolvable: restart stops it first and then fails to start it again.
    #[test]
    fn running_daemons_without_a_config_are_not_startable() {
        let projects = fixture_projects();
        let project = find_project(&projects, "blog").unwrap();
        let wt = find_worktree(project, "default").unwrap();
        let daemons = index(vec![daemon("blog/site", ApiDaemonStatus::Running, Some(5))]);

        let stack = build_stack(project, wt, &daemons, &Resolvable::new());
        assert_eq!(stack.unresolvable_daemons, vec!["blog/site".to_string()]);
        assert!(!stack.can_start);
    }

    /// A worktree with no config file of its own must not inherit the `global`
    /// namespace: that would list every global daemon under it, with working
    /// controls, as if the worktree declared them.
    #[test]
    fn worktree_without_a_config_does_not_borrow_the_global_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("feat");
        std::fs::create_dir_all(&worktree).unwrap();

        let namespace = namespace_for_worktree(&worktree).unwrap();
        assert_eq!(namespace, "feat");
        assert_ne!(namespace, "global");
    }

    /// A group can name daemons of other namespaces. Those are rendered by the
    /// stack, so they need the same resolvability check: otherwise Restart
    /// stops such a daemon and then fails to start it again.
    #[test]
    fn cross_namespace_group_members_are_checked_too() {
        let mut projects = fixture_projects();
        let shop = projects.iter_mut().find(|p| p.name == "shop").unwrap();
        shop.worktrees[0]
            .groups
            .insert("shared".to_string(), group(&["shop/api", "other/api"]));

        let daemons = index(vec![
            daemon("shop/api", ApiDaemonStatus::Running, Some(10)),
            daemon("other/api", ApiDaemonStatus::Running, Some(10)),
        ]);
        // The supervisor can resolve the worktree's own daemon, but not the
        // one the group borrows from another namespace.
        let resolvable: Resolvable = ["shop/api".to_string()].into_iter().collect();

        let project = find_project(&projects, "shop").unwrap();
        let wt = find_worktree(project, "main").unwrap();
        let stack = build_stack(project, wt, &daemons, &resolvable);

        assert_eq!(stack.unresolvable_daemons, vec!["other/api".to_string()]);
        assert!(!stack.can_start);
    }

    /// A directory name that is not a valid namespace leaves the worktree
    /// without one, rather than borrowing the project's: that would list and
    /// control the primary checkout's daemons under this worktree.
    #[test]
    fn worktree_with_an_underivable_namespace_borrows_none() {
        let temp = tempfile::tempdir().unwrap();
        // Non-ASCII directory names have no valid namespace.
        let worktree = temp.path().join("功能");
        std::fs::create_dir_all(&worktree).unwrap();
        assert!(namespace_for_worktree(&worktree).is_err());

        let views = worktree_views(&worktree);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].namespace, None);
        assert!(views[0].namespace_error.is_some());

        // With no namespace, no daemon is attributed to it.
        let daemons = index(vec![daemon("shop/api", ApiDaemonStatus::Running, Some(5))]);
        let project = ProjectView {
            name: "shop".into(),
            dir: worktree.clone(),
            dir_exists: true,
            worktrees: views,
        };
        let json =
            serde_json::to_value(build_project(&project, &daemons, &all_resolvable(&daemons)))
                .unwrap();
        assert_eq!(json["worktrees"][0]["namespace"], serde_json::Value::Null);
        assert_eq!(json["worktrees"][0]["daemons"]["total"], 0);
        assert_eq!(json["stack"]["ungrouped"].as_array().unwrap().len(), 0);
    }

    /// A `[namespaces]` entry whose directory was deleted must not render as a
    /// healthy project with startable daemons.
    #[test]
    fn missing_project_directory_is_reported_and_not_startable() {
        let missing = PathBuf::from("/nonexistent/pitchfork-project");
        let worktrees = worktree_views(&missing);
        assert_eq!(worktrees.len(), 1);
        assert!(!worktrees[0].dir_exists);

        let project = ProjectView {
            name: "ghost".into(),
            dir: missing,
            dir_exists: false,
            worktrees,
        };
        let daemons = DaemonIndex::new();
        let json =
            serde_json::to_value(build_project(&project, &daemons, &Resolvable::new())).unwrap();
        assert_eq!(json["dir_exists"], false);
        assert_eq!(json["worktrees"][0]["dir_exists"], false);
        assert_eq!(json["worktrees"][0]["can_start"], false);
        assert_eq!(json["stack"]["can_start"], false);
    }

    /// Discovery against a real repository, so the git plumbing this relies on
    /// is exercised rather than only its parsers.
    #[test]
    fn worktree_views_match_real_git_worktrees() {
        fn git(dir: &StdPath, args: &[&str]) -> bool {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("shop");
        std::fs::create_dir_all(&repo).unwrap();
        if !git(&repo, &["init", "-q", "-b", "main", "."]) {
            return; // no usable git here
        }
        std::fs::write(
            repo.join("pitchfork.toml"),
            "[daemons.api]\nrun = \"true\"\n",
        )
        .unwrap();
        assert!(git(&repo, &["add", "-A"]));
        assert!(git(&repo, &["commit", "-qm", "init"]));
        assert!(git(
            &repo,
            &["worktree", "add", "-q", "../shop-feat", "-b", "feature/a"]
        ));

        let views = worktree_views(&repo);
        let names: Vec<&str> = views.iter().map(|w| w.name.as_str()).collect();
        assert!(names.contains(&"main"), "got {names:?}");
        assert!(names.contains(&"feature-a"), "got {names:?}");

        let primary = views.iter().find(|w| w.is_primary).unwrap();
        assert_eq!(primary.branch, "main");
        assert!(primary.dir_exists);

        let linked = views.iter().find(|w| w.name == "feature-a").unwrap();
        assert!(!linked.is_primary);
        // The linked worktree resolves back to the same main checkout.
        assert_eq!(
            main_checkout_root(&linked.path).map(|p| canonical(&p)),
            Some(canonical(&repo))
        );
    }

    #[test]
    fn submodules_are_not_treated_as_linked_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let superproject = temp.path().join("repo");
        let submodule = superproject.join("vendor").join("lib");
        std::fs::create_dir_all(superproject.join(".git").join("modules").join("lib")).unwrap();
        std::fs::create_dir_all(&submodule).unwrap();
        std::fs::write(
            submodule.join(".git"),
            format!(
                "gitdir: {}\n",
                superproject.join(".git/modules/lib").display()
            ),
        )
        .unwrap();

        // The superproject is not the submodule's main checkout, so the
        // submodule keeps its own project and worktrees.
        assert_eq!(main_checkout_root(&submodule), None);
    }

    #[test]
    fn stack_endpoint_serves_a_worktree_without_groups() {
        let projects = fixture_projects();
        let project = find_project(&projects, "blog").unwrap();
        let wt = find_worktree(project, "default").unwrap();
        let daemons = fixture_daemons();
        let stack = build_stack(project, wt, &daemons, &all_resolvable(&daemons));

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
