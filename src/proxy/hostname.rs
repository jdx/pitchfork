//! Automatic hostnames for daemons.
//!
//! Every daemon that configures a `port` gets a hostname derived from where its
//! configuration lives, resolved right to left against a registry of projects:
//!
//! ```text
//! <daemon>.<worktree>.<project>.<tld>   daemon in a linked git worktree
//! <daemon>.<project>.<tld>              daemon in the primary checkout
//! <worktree>.<project>.<tld>            stack page (not a daemon)
//! <project>.<tld>                       project page (not a daemon)
//! ```
//!
//! Labels are DNS-safe lowercase. The project label is the namespace's project
//! name when the project declares one explicitly, otherwise the directory name
//! of the primary checkout. The worktree label is the linked worktree's
//! directory name unless the config sets `worktree_label`.

use crate::config_types::ProxyConfig;
use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlDaemon};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Maximum length of a single DNS label (RFC 1035).
const MAX_LABEL_LEN: usize = 63;

/// Maximum length of a full host name (RFC 1035), which the labels share with
/// the configured TLD.
pub(crate) const MAX_HOSTNAME_LEN: usize = 253;

/// Whether a hostname's labels still leave room for the configured TLD.
///
/// Three maximum-length labels plus a long `proxy.tld` can exceed what DNS
/// accepts, and a name nothing can resolve is worse than no name at all. Every
/// surface asks this — generation, listing and routing — so they agree on
/// whether such a hostname exists.
pub fn hostname_fits(host: &str) -> bool {
    let s = crate::settings::settings();
    host.len() + 1 + crate::proxy::effective_tld(&s).len() <= MAX_HOSTNAME_LEN
}

/// Convert an arbitrary name into a DNS-safe lowercase label.
///
/// ASCII letters are lowercased, digits are kept, and every other character
/// becomes `-`. Runs of `-` collapse, leading and trailing `-` are trimmed and
/// the result is truncated to 63 characters. Returns `None` when nothing
/// usable is left, in which case the caller has no hostname to offer.
pub fn sanitize_label(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    let trimmed = if trimmed.len() > MAX_LABEL_LEN {
        trimmed[..MAX_LABEL_LEN].trim_end_matches('-')
    } else {
        trimmed
    };
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

// ─── checkout detection ──────────────────────────────────────────────────────

/// Where a directory sits in a git repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// The primary checkout: the parent of the git common directory.
    pub primary: PathBuf,
    /// The linked worktree's own root, when the directory is inside one.
    pub worktree: Option<PathBuf>,
}

impl Checkout {
    /// The root of the checkout the directory belongs to.
    pub fn root(&self) -> &Path {
        self.worktree.as_deref().unwrap_or(&self.primary)
    }
}

/// Parse the `gitdir:` pointer of a linked worktree's `.git` file.
///
/// Returns the linked worktree's administrative directory
/// (`<common>/worktrees/<name>`), resolved against `dir` when relative.
fn parse_gitdir_pointer(dir: &Path, content: &str) -> Option<PathBuf> {
    let target = content
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))?
        .trim();
    if target.is_empty() {
        return None;
    }
    let path = PathBuf::from(target);
    let path = if path.is_absolute() {
        path
    } else {
        dir.join(path)
    };
    Some(normalize(&path))
}

/// Resolve `..` and `.` components without touching the filesystem, so the
/// result is stable for directories git has already removed.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            part => out.push(part.as_os_str()),
        }
    }
    out
}

/// Canonicalize as much of a path as exists, keeping the rest as written.
///
/// Every checkout path pitchfork compares goes through here, so the same
/// directory has one spelling no matter how it was reached: through a symlink,
/// as `/var` instead of `/private/var` on macOS, or with the `\\?\` prefix
/// Windows adds. A leaf that no longer exists — a deleted working directory, a
/// dangling symlink — still resolves against its nearest surviving ancestor, so
/// a daemon whose directory disappeared keeps the checkout it started in.
fn canonicalize_best_effort(path: &Path) -> PathBuf {
    let path = normalize(path);
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.as_path();
    loop {
        if let Ok(resolved) = current.canonicalize() {
            let mut out = dunce::simplified(&resolved).to_path_buf();
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let Some(parent) = current.parent() else {
            return path;
        };
        match current.file_name() {
            Some(name) => suffix.push(name.to_os_string()),
            // A path with no file name (a bare root) cannot be walked further.
            None => return path,
        }
        current = parent;
    }
}

/// Split `<common>/worktrees/<name>` into the primary checkout directory.
///
/// The common directory is the parent of `worktrees/`, and the primary
/// checkout is its parent. Anything else (a submodule's `.git/modules/...`
/// pointer, for instance) is not a linked worktree.
fn primary_from_worktree_gitdir(gitdir: &Path) -> Option<PathBuf> {
    let worktrees_dir = gitdir.parent()?;
    if worktrees_dir.file_name()? != "worktrees" {
        return None;
    }
    let common = worktrees_dir.parent()?;
    // A bare repository has no primary checkout: its common directory is the
    // repository itself (`repo.git`), not a `.git` inside a working tree.
    // Treating its parent as the project would group every bare repository in
    // that directory under one label, so such a worktree stands on its own.
    if common.file_name()? != ".git" {
        return None;
    }
    // The pointer is whatever git wrote, so it needs the same resolution as a
    // path the caller supplied before it can be compared with one.
    common.parent().map(canonicalize_best_effort)
}

/// The root of the checkout a directory belongs to, canonicalized.
///
/// Used to attribute a running daemon to one checkout. Comparing paths
/// lexically would put a worktree nested inside its primary checkout (say
/// `.worktrees/feature`) in the primary, and would miss a daemon whose
/// directory reaches the same place through a symlink.
pub fn checkout_root_of(dir: &Path) -> PathBuf {
    let checkout = detect_checkout(dir);
    checkout.root().to_path_buf()
}

/// Locate the checkout containing `dir` by walking up to the nearest `.git`.
///
/// A `.git` directory marks the primary checkout. A `.git` file whose
/// `gitdir:` points into `<common>/worktrees/<name>` marks a linked worktree,
/// whose primary checkout is the parent of the common directory. When no
/// `.git` is found the directory is treated as its own primary checkout, so
/// projects that are not git repositories still get a hostname.
pub fn detect_checkout(dir: &Path) -> Checkout {
    let start = canonicalize_best_effort(dir);
    for current in start.ancestors() {
        let git = current.join(".git");
        if git.is_dir() {
            return Checkout {
                primary: current.to_path_buf(),
                worktree: None,
            };
        }
        if git.is_file() {
            let primary = std::fs::read_to_string(&git)
                .ok()
                .and_then(|content| parse_gitdir_pointer(current, &content))
                .and_then(|gitdir| primary_from_worktree_gitdir(&gitdir));
            return match primary {
                Some(primary) if primary != current => Checkout {
                    primary,
                    worktree: Some(current.to_path_buf()),
                },
                // A `.git` file that is not a linked worktree pointer (a
                // submodule, say) still marks the root of its own checkout.
                _ => Checkout {
                    primary: current.to_path_buf(),
                    worktree: None,
                },
            };
        }
    }
    Checkout {
        primary: start,
        worktree: None,
    }
}

// ─── labels ──────────────────────────────────────────────────────────────────

/// The project label for a primary checkout.
///
/// A project registered or configured with an explicit namespace uses that
/// name; otherwise the directory name of the primary checkout is used.
pub fn project_label(primary: &Path) -> Option<String> {
    let explicit = PitchforkToml::project_namespace_override(primary)
        .ok()
        .flatten()
        .or_else(|| crate::extra_configs::namespace_for_dir(primary));
    match explicit {
        Some(ns) => sanitize_label(&ns),
        None => sanitize_label(&primary.file_name()?.to_string_lossy()),
    }
}

/// The worktree label for a linked worktree directory.
///
/// Defaults to the directory name, overridden by the `worktree_label` key in
/// that worktree's own configuration files.
pub fn worktree_label(worktree: &Path) -> Option<String> {
    if let Some(label) = PitchforkToml::project_worktree_label(worktree) {
        return sanitize_label(&label);
    }
    sanitize_label(&worktree.file_name()?.to_string_lossy())
}

/// The daemon label for a daemon config: its name, or the `proxy` override.
pub fn daemon_label(name: &str, proxy: Option<&ProxyConfig>) -> Option<String> {
    if proxy.is_some_and(ProxyConfig::is_disabled) {
        return None;
    }
    match proxy.and_then(ProxyConfig::label) {
        Some(label) => sanitize_label(label),
        None => sanitize_label(name),
    }
}

/// Join hostname labels left to right, omitting the worktree when absent.
fn join_labels(daemon: &str, worktree: Option<&str>, project: &str) -> String {
    match worktree {
        Some(wt) => format!("{daemon}.{wt}.{project}"),
        None => format!("{daemon}.{project}"),
    }
}

/// The automatic hostname (without TLD) for a daemon, e.g. `api.fix-1.myproj`.
///
/// Every label is checked against the same project the proxy routes with, so a
/// label the proxy refuses as ambiguous is never advertised as a URL. Returns
/// `None` when the daemon configures no port, opted out with `proxy = false`,
/// no label could be derived, or any of its labels collides with another
/// daemon, worktree or project.
pub fn auto_host_for_daemon(id: &DaemonId, config: &PitchforkTomlDaemon) -> Option<String> {
    config.port.as_ref()?;
    let daemon = daemon_label(id.name(), config.proxy.as_ref())?;
    let path = config.path.as_deref()?;
    // A daemon declared in a global config belongs to no project, and the
    // config's own directory is not one: it would put daemons under a label
    // like `pitchfork`. Those daemons are reachable through a legacy slug.
    if crate::pitchfork_toml::is_global_config(path) {
        return None;
    }
    let base = crate::pitchfork_toml::project_dir_for_config(path)?;
    let checkout = detect_checkout(&base);
    let project_label = project_label(&checkout.primary)?;
    if project_label_is_ambiguous(&project_label, &checkout.primary) {
        return None;
    }

    let project = project_hosts_for(&checkout.primary, &project_label)?;
    let (hosts, worktree) = match &checkout.worktree {
        Some(dir) => {
            // A worktree missing from the project either collided with another
            // worktree's label or failed to load; either way it is not routed.
            let label = worktree_label(dir)?;
            (project.worktrees.get(&label)?, Some(label))
        }
        None => (&project.primary, None),
    };
    // The checkout routes this label to this daemon, or to nothing at all.
    if hosts.daemons.get(&daemon).map(String::as_str) != Some(id.name()) {
        return None;
    }

    let host = join_labels(&daemon, worktree.as_deref(), &project_label);
    if !hostname_fits(&host) {
        log::warn!(
            "'{host}' plus the configured proxy.tld is over the {MAX_HOSTNAME_LEN}-byte DNS \
             limit, so no hostname is assigned. Shorten the project, worktree or daemon name, \
             or use a shorter proxy.tld."
        );
        return None;
    }
    Some(host)
}

/// The hostname to advertise for a daemon: a legacy `[slugs]` entry when one
/// exists, otherwise the automatic hostname.
///
/// Legacy slugs win because the proxy resolves them first, so this never
/// advertises an address that routes somewhere else.
pub fn host_for_daemon(
    id: &DaemonId,
    config: Option<&PitchforkTomlDaemon>,
    global_slugs: &indexmap::IndexMap<String, crate::pitchfork_toml::SlugEntry>,
) -> Option<String> {
    // Slugs are registered without a length check, and one too long for the
    // configured TLD is a name the proxy will not route, so it is not offered
    // as a URL either.
    if let Some(slug) = PitchforkToml::find_slug_for_daemon_in_registry(id, global_slugs) {
        if hostname_fits(&slug) {
            return Some(slug);
        }
        log::warn!(
            "Slug '{slug}' plus the configured proxy.tld is over the \
             {MAX_HOSTNAME_LEN}-byte DNS limit, so it is not advertised for {id}."
        );
    }
    auto_host_for_daemon(id, config?)
}

// ─── registry ────────────────────────────────────────────────────────────────

/// One checkout of a project and the daemons reachable within it.
#[derive(Debug, Clone)]
pub struct CheckoutHosts {
    /// Root directory of this checkout.
    pub dir: PathBuf,
    /// Namespace the checkout's daemons belong to.
    pub namespace: String,
    /// Daemon label → daemon name.
    pub daemons: HashMap<String, String>,
}

impl CheckoutHosts {
    /// Load the routable daemons of one checkout, along with any collisions
    /// found while doing so.
    ///
    /// Two daemons whose labels reduce to the same string are both dropped:
    /// keeping either one would route half the requests to a daemon the user
    /// did not name, and which of the two won would depend on config order.
    fn load(dir: &Path) -> Option<(Self, Vec<String>)> {
        let namespace = PitchforkToml::namespace_for_dir(dir).ok()?;
        let pt = PitchforkToml::all_merged_from(dir).ok()?;
        let mut daemons: HashMap<String, String> = HashMap::new();
        let mut errors = Vec::new();
        let mut colliding: Vec<String> = Vec::new();
        for (id, config) in &pt.daemons {
            if id.namespace() != namespace || config.port.is_none() {
                continue;
            }
            let Some(label) = daemon_label(id.name(), config.proxy.as_ref()) else {
                continue;
            };
            match daemons.get(&label) {
                Some(other) => {
                    errors.push(format!(
                        "daemon hostname label '{label}' in {} is claimed by both '{other}' and \
                         '{name}'. Rename one of them, or set `proxy = \"<label>\"` on one.",
                        dir.display(),
                        name = id.name(),
                    ));
                    colliding.push(label);
                }
                None => {
                    daemons.insert(label, id.name().to_string());
                }
            }
        }
        for label in colliding {
            daemons.remove(&label);
        }
        Some((
            Self {
                dir: dir.to_path_buf(),
                namespace,
                daemons,
            },
            errors,
        ))
    }

    /// Sorted daemon labels, for listings and error pages.
    pub fn labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.daemons.keys().cloned().collect();
        labels.sort();
        labels
    }
}

/// A project and its checkouts, keyed by hostname label.
#[derive(Debug, Clone)]
pub struct ProjectHosts {
    pub label: String,
    pub primary: CheckoutHosts,
    /// Worktree label → checkout.
    pub worktrees: HashMap<String, CheckoutHosts>,
}

impl ProjectHosts {
    /// Every checkout of this project: its primary and each linked worktree.
    pub fn checkouts(&self) -> impl Iterator<Item = &CheckoutHosts> {
        std::iter::once(&self.primary).chain(self.worktrees.values())
    }

    /// Sorted worktree labels.
    pub fn worktree_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.worktrees.keys().cloned().collect();
        labels.sort();
        labels
    }
}

/// All projects the proxy can route to, keyed by project label.
#[derive(Debug, Clone, Default)]
pub struct HostRegistry {
    pub projects: HashMap<String, ProjectHosts>,
    /// Label collisions found while loading, reported to the user as-is.
    pub errors: Vec<String>,
}

/// What a hostname resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTarget {
    /// A daemon in one of the project's checkouts.
    Daemon {
        project: String,
        worktree: Option<String>,
        dir: PathBuf,
        namespace: String,
        daemon: String,
    },
    /// `<project>.<tld>` — reserved for the project page.
    ProjectPage { project: String },
    /// `<worktree>.<project>.<tld>` — reserved for the stack page.
    WorktreePage { project: String, worktree: String },
    /// The rightmost label is not a known project.
    UnknownProject { known: Vec<String> },
    /// The project is known but the daemon label is not.
    UnknownDaemon {
        project: String,
        worktree: Option<String>,
        known: Vec<String>,
    },
}

/// Group a project's worktree checkouts by label.
///
/// Two worktrees that reduce to the same label are both dropped and reported:
/// routing one of them would answer half the requests with the other's content,
/// which is worse than not routing the label at all.
fn group_worktrees(
    project: &str,
    found: Vec<(String, CheckoutHosts)>,
) -> (HashMap<String, CheckoutHosts>, Vec<String>) {
    let mut kept: HashMap<String, CheckoutHosts> = HashMap::new();
    let mut errors = Vec::new();
    let mut colliding: Vec<String> = Vec::new();
    for (label, hosts) in found {
        match kept.get(&label) {
            Some(existing) if existing.dir == hosts.dir => continue,
            Some(existing) => {
                errors.push(format!(
                    "worktree label '{label}' in project '{project}' is claimed by two \
                     directories: {} and {}. Set `worktree_label` in one of them.",
                    existing.dir.display(),
                    hosts.dir.display(),
                ));
                colliding.push(label);
            }
            None => {
                kept.insert(label, hosts);
            }
        }
    }
    for label in colliding {
        kept.remove(&label);
    }
    (kept, errors)
}

/// How long a project's worktree discovery stays cached.
///
/// `pitchfork list` derives a hostname for every daemon it prints, and each one
/// would otherwise re-enumerate the project's worktrees. Only the enumeration
/// is cached: configuration is re-read every time, so a label that starts
/// colliding stops being advertised at once, and only a brand-new worktree
/// directory can take up to this long to appear.
const WORKTREE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

struct WorktreeCache {
    entries: HashMap<PathBuf, (std::time::Instant, std::sync::Arc<Vec<PathBuf>>)>,
}

static WORKTREE_CACHE: once_cell::sync::Lazy<std::sync::Mutex<WorktreeCache>> =
    once_cell::sync::Lazy::new(|| {
        std::sync::Mutex::new(WorktreeCache {
            entries: HashMap::new(),
        })
    });

/// The linked worktree roots of a project, cached briefly.
fn cached_worktree_dirs(primary: &Path) -> std::sync::Arc<Vec<PathBuf>> {
    let now = std::time::Instant::now();
    {
        let cache = WORKTREE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((expires_at, dirs)) = cache.entries.get(primary)
            && now < *expires_at
        {
            return std::sync::Arc::clone(dirs);
        }
    } // lock released before any I/O

    let dirs = std::sync::Arc::new(worktree_dirs(primary));

    let mut cache = WORKTREE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.entries.retain(|_, (expires_at, _)| now < *expires_at);
    cache.entries.insert(
        primary.to_path_buf(),
        (now + WORKTREE_CACHE_TTL, std::sync::Arc::clone(&dirs)),
    );
    dirs
}

/// Report a configuration problem once per distinct message.
///
/// A label collision persists until someone renames something, while the
/// registry behind it is rebuilt every couple of seconds and on every CLI
/// invocation. Logging each rebuild would fill the supervisor log with the same
/// line; the user needs to read it once.
pub fn warn_once(message: &str) {
    static SEEN: once_cell::sync::Lazy<std::sync::Mutex<std::collections::HashSet<String>>> =
        once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    // Distinct messages are bounded by the configuration, but a pathological
    // one should not grow the set forever.
    if seen.len() > 256 {
        seen.clear();
    }
    if seen.insert(message.to_string()) {
        log::warn!("{message}");
    }
}

/// Directories that may contain a project pitchfork knows about.
///
/// Only persisted knowledge counts: the namespace registry, the legacy slug
/// registry, and the directories of daemons in the state file. The current
/// directory is deliberately absent, because it differs between the supervisor
/// and each CLI invocation, and a registry that depended on it would let the
/// CLI advertise a hostname the proxy does not route — or refuse one it does.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for (_, entry) in PitchforkToml::read_global_namespaces() {
        candidates.push(entry.dir);
    }
    for (_, entry) in PitchforkToml::read_global_slugs() {
        if let Some(dir) = entry.resolve_dir() {
            candidates.push(dir);
        }
    }
    if let Ok(state) = crate::state_file::StateFile::read(&*crate::env::PITCHFORK_STATE_FILE) {
        for daemon in state.daemons.values() {
            if let Some(dir) = &daemon.dir {
                candidates.push(dir.clone());
            }
        }
    }
    candidates
}

/// Whether another known project claims the same project label.
///
/// Such a label routes to neither project, so nothing may advertise a URL
/// under it. This reads configuration files but never enumerates worktrees,
/// which keeps it cheap enough for the per-daemon display paths.
fn project_label_is_ambiguous(label: &str, primary: &Path) -> bool {
    let mut seen: Vec<PathBuf> = Vec::new();
    for dir in candidate_dirs() {
        if !dir.exists() {
            continue;
        }
        let other = detect_checkout(&dir).primary;
        if other == primary || seen.contains(&other) {
            continue;
        }
        seen.push(other.clone());
        if project_label(&other).as_deref() == Some(label) {
            return true;
        }
    }
    false
}

/// Linked worktree roots recorded in a checkout's git common directory.
///
/// Each `<common>/worktrees/<name>/gitdir` holds the path of that worktree's
/// own `.git` file, whose parent is the worktree root. Reading them directly
/// costs no subprocess and still works when `git` is unavailable, so it
/// complements the `git worktree list` discovery that also covers jj.
fn linked_worktree_dirs(primary: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(primary.join(".git/worktrees")) else {
        return vec![];
    };
    entries
        .filter_map(|entry| {
            let gitdir = std::fs::read_to_string(entry.ok()?.path().join("gitdir")).ok()?;
            let git_file = PathBuf::from(gitdir.trim());
            git_file.parent().map(Path::to_path_buf)
        })
        .collect()
}

/// Every linked worktree root of a project, from both discovery sources.
///
/// `git worktree list` also covers jj workspaces, and the `gitdir` pointers
/// cover repositories where `git` is unavailable or errors.
fn worktree_dirs(primary: &Path) -> Vec<PathBuf> {
    if !crate::settings::settings().general.worktree {
        return vec![];
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let found = crate::proxy::worktree::discover_worktrees(primary)
        .into_iter()
        .map(|entry| entry.path)
        .chain(linked_worktree_dirs(primary));
    for path in found {
        let Some(wt_dir) = detect_checkout(&path).worktree else {
            continue; // the primary checkout itself
        };
        if !dirs.contains(&wt_dir) {
            dirs.push(wt_dir);
        }
    }
    dirs
}

/// Build one project's checkouts: its primary and every linked worktree.
///
/// Returns the project together with the label collisions found inside it.
/// Configuration is read fresh on every call, so a collision introduced by an
/// edit takes effect immediately; only `worktree_dirs` may be cached.
fn build_project_hosts(
    primary: &Path,
    label: &str,
    worktrees: &[PathBuf],
) -> Option<(ProjectHosts, Vec<String>)> {
    let (primary_hosts, mut errors) = CheckoutHosts::load(primary)?;
    let mut project = ProjectHosts {
        label: label.to_string(),
        primary: primary_hosts,
        worktrees: HashMap::new(),
    };

    let mut found: Vec<(String, CheckoutHosts)> = Vec::new();
    for wt_dir in worktrees {
        let (Some(wt_label), Some((hosts, wt_errors))) =
            (worktree_label(wt_dir), CheckoutHosts::load(wt_dir))
        else {
            continue;
        };
        errors.extend(wt_errors);
        found.push((wt_label, hosts));
    }
    let (worktrees, wt_errors) = group_worktrees(label, found);
    project.worktrees = worktrees;
    errors.extend(wt_errors);

    Some((project, errors))
}

/// Build a project the way the proxy routes it, reusing a brief cache of its
/// worktree enumeration.
fn project_hosts_for(primary: &Path, label: &str) -> Option<ProjectHosts> {
    let worktrees = cached_worktree_dirs(primary);
    let (project, errors) = build_project_hosts(primary, label, &worktrees)?;
    for err in errors {
        warn_once(&err);
    }
    Some(project)
}

impl HostRegistry {
    /// Build the registry from every project pitchfork knows about.
    ///
    /// Candidate directories come from the current directory, the namespace
    /// registry, the legacy slug registry, and the directories of daemons in
    /// the state file. Each is mapped to its primary checkout, whose linked
    /// worktrees are then discovered.
    ///
    /// A project the supervisor has never seen — never started, never
    /// registered — is therefore not routable from another directory yet.
    pub fn build() -> Self {
        Self::from_dirs(&candidate_dirs())
    }

    /// Build the registry from an explicit list of project directories.
    pub fn from_dirs(dirs: &[PathBuf]) -> Self {
        // Collapse the candidates to distinct primary checkouts.
        let mut primaries: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            if !dir.exists() {
                continue;
            }
            let primary = detect_checkout(dir).primary;
            if !primaries.contains(&primary) {
                primaries.push(primary);
            }
        }

        let mut registry = Self::default();
        let mut colliding: Vec<String> = Vec::new();
        for primary in primaries {
            let Some(label) = project_label(&primary) else {
                continue;
            };

            if let Some(existing) = registry.projects.get(&label) {
                if existing.primary.dir != primary {
                    registry.errors.push(format!(
                        "project label '{label}' is claimed by two directories: {} and {}. \
                         Set a distinct top-level `namespace` in one of them.",
                        existing.primary.dir.display(),
                        primary.display(),
                    ));
                    // Neither is routed: answering for one of them would serve
                    // the wrong project's daemons under a URL the other
                    // checkout advertises for itself.
                    colliding.push(label);
                }
                continue;
            }

            let worktrees = worktree_dirs(&primary);
            let Some((project, errors)) = build_project_hosts(&primary, &label, &worktrees) else {
                continue;
            };
            registry.errors.extend(errors);
            // A directory with no routable daemon anywhere in it is not a
            // project worth a hostname; an ad-hoc daemon's working directory
            // would otherwise become an empty one.
            if project.checkouts().all(|c| c.daemons.is_empty()) {
                continue;
            }
            registry.projects.insert(label, project);
        }
        for label in colliding {
            registry.projects.remove(&label);
        }

        registry
    }

    /// Whether more than one checkout across all known projects would run this
    /// daemon ID.
    ///
    /// A namespace comes from the checkout's own directory name or config, not
    /// from the project above it, so two unrelated projects that each have a
    /// `fix-1` worktree with an `api` daemon both produce `fix-1/api`. The
    /// state file holds one record per ID, so in that case a request has to be
    /// matched to the checkout it names, exactly as for two checkouts of one
    /// project.
    pub fn shares_daemon_id(&self, namespace: &str, daemon: &str) -> bool {
        self.projects
            .values()
            .flat_map(ProjectHosts::checkouts)
            .filter(|c| c.namespace == namespace && c.daemons.values().any(|n| n == daemon))
            .count()
            > 1
    }

    /// Sorted project labels.
    pub fn project_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.projects.keys().cloned().collect();
        labels.sort();
        labels
    }

    /// Resolve a hostname's labels (the host with the TLD already stripped).
    ///
    /// Resolution runs right to left: the last label must name a project, an
    /// optional worktree label follows, and the label before the daemon's is
    /// where extra leading labels become wildcard subdomains of the same
    /// daemon. A label that names both a worktree and a daemon is read as the
    /// worktree.
    pub fn resolve(&self, subdomain: &str, wildcard: bool) -> HostTarget {
        let labels: Vec<String> = subdomain
            .split('.')
            .map(|l| l.to_ascii_lowercase())
            .collect();
        let Some((project_label, rest)) = labels.split_last() else {
            return HostTarget::UnknownProject {
                known: self.project_labels(),
            };
        };
        let Some(project) = self.projects.get(project_label) else {
            return HostTarget::UnknownProject {
                known: self.project_labels(),
            };
        };
        if rest.is_empty() {
            return HostTarget::ProjectPage {
                project: project.label.clone(),
            };
        }

        // A worktree label directly left of the project consumes one label.
        let (checkout, worktree, rest) = match rest.split_last() {
            Some((maybe_worktree, head)) => match project.worktrees.get(maybe_worktree) {
                Some(checkout) => (checkout, Some(maybe_worktree.clone()), head),
                None => (&project.primary, None, rest),
            },
            None => (&project.primary, None, rest),
        };

        let Some((daemon_label, extra)) = rest.split_last() else {
            return HostTarget::WorktreePage {
                project: project.label.clone(),
                worktree: worktree.unwrap_or_default(),
            };
        };
        if !extra.is_empty() && !wildcard {
            return HostTarget::UnknownDaemon {
                project: project.label.clone(),
                worktree,
                known: checkout.labels(),
            };
        }

        match checkout.daemons.get(daemon_label) {
            Some(name) => HostTarget::Daemon {
                project: project.label.clone(),
                worktree,
                dir: checkout.dir.clone(),
                namespace: checkout.namespace.clone(),
                daemon: name.clone(),
            },
            None => HostTarget::UnknownDaemon {
                project: project.label.clone(),
                worktree,
                known: checkout.labels(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path as `detect_checkout` reports it: canonical, and without the
    /// `\\?\` verbatim prefix Windows canonicalization adds.
    fn canonical(path: &Path) -> PathBuf {
        dunce::simplified(&path.canonicalize().unwrap()).to_path_buf()
    }

    fn checkout(dir: &str, namespace: &str, daemons: &[(&str, &str)]) -> CheckoutHosts {
        CheckoutHosts {
            dir: PathBuf::from(dir),
            namespace: namespace.to_string(),
            daemons: daemons
                .iter()
                .map(|(l, n)| (l.to_string(), n.to_string()))
                .collect(),
        }
    }

    #[test]
    fn test_sanitize_label() {
        assert_eq!(sanitize_label("api").as_deref(), Some("api"));
        assert_eq!(sanitize_label("My App").as_deref(), Some("my-app"));
        assert_eq!(
            sanitize_label("feature/my_branch").as_deref(),
            Some("feature-my-branch")
        );
        assert_eq!(sanitize_label("--weird--").as_deref(), Some("weird"));
        assert_eq!(sanitize_label("café").as_deref(), Some("caf"));
        assert_eq!(sanitize_label("---"), None);
        assert_eq!(sanitize_label(""), None);
        assert_eq!(sanitize_label(&"a".repeat(80)).unwrap().len(), 63);
    }

    #[test]
    fn test_daemon_label_override_and_opt_out() {
        assert_eq!(daemon_label("api", None).as_deref(), Some("api"));
        assert_eq!(
            daemon_label("api", Some(&ProxyConfig::Enabled)).as_deref(),
            Some("api")
        );
        assert_eq!(
            daemon_label("api", Some(&ProxyConfig::Name("Web UI".into()))).as_deref(),
            Some("web-ui")
        );
        assert_eq!(daemon_label("api", Some(&ProxyConfig::Disabled)), None);
    }

    /// A primary checkout has a `.git` directory and no worktree label.
    #[test]
    fn test_detect_checkout_primary() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("sub/dir")).unwrap();

        let found = detect_checkout(&repo.join("sub/dir"));
        assert_eq!(found.primary, canonical(&repo));
        assert_eq!(found.worktree, None);
    }

    /// A linked worktree's `.git` file points into `<common>/worktrees/<name>`.
    #[test]
    fn test_detect_checkout_linked_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir_all(repo.join(".git/worktrees/fix-1")).unwrap();
        let wt = temp.path().join("fix-1");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/fix-1").display()),
        )
        .unwrap();

        let found = detect_checkout(&wt);
        assert_eq!(found.primary, canonical(&repo));
        assert_eq!(found.worktree, Some(canonical(&wt)));
        assert_eq!(found.root(), canonical(&wt));
    }

    /// A working directory that no longer exists still belongs to the checkout
    /// above it, so a daemon whose directory was deleted keeps its hostname.
    #[test]
    fn test_checkout_root_of_missing_directory() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        assert_eq!(
            checkout_root_of(&repo.join("gone/deeper")),
            canonical(&repo)
        );
    }

    /// The primary checkout read from a worktree's `gitdir:` pointer is spelled
    /// the same as the one found by walking into the primary directly, whatever
    /// alias the pointer took.
    #[cfg(unix)]
    #[test]
    fn test_detect_checkout_primary_spelling_matches_through_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        let admin = repo.join(".git/worktrees/fix-1");
        std::fs::create_dir_all(&admin).unwrap();
        let wt = temp.path().join("fix-1");
        std::fs::create_dir_all(&wt).unwrap();

        // Point the worktree at the repository through a symlinked alias, the
        // way a checkout under a symlinked home directory would.
        let alias = temp.path().join("alias");
        symlink(&repo, &alias).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", alias.join(".git/worktrees/fix-1").display()),
        )
        .unwrap();

        assert_eq!(detect_checkout(&wt).primary, detect_checkout(&repo).primary);
        assert_eq!(detect_checkout(&wt).primary, canonical(&repo));
    }

    /// A bare repository has no working tree to be the project, so its linked
    /// worktrees stand alone rather than being grouped under the directory that
    /// happens to hold the bare repositories.
    #[test]
    fn test_detect_checkout_bare_repository_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let bare = temp.path().join("my-repo.git");
        let admin = bare.join("worktrees/fix-1");
        std::fs::create_dir_all(&admin).unwrap();
        let wt = temp.path().join("fix-1");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();

        let found = detect_checkout(&wt);
        assert_eq!(found.primary, canonical(&wt));
        assert_eq!(found.worktree, None);
    }

    /// A daemon declared in a global config has no project, so it gets no
    /// automatic hostname; the config directory is not one.
    #[test]
    fn test_auto_host_skips_global_config_daemons() {
        let config = PitchforkTomlDaemon {
            run: "server".to_string(),
            port: Some(crate::config_types::PortConfig {
                expect: vec![3000],
                ..Default::default()
            }),
            path: Some(crate::env::PITCHFORK_GLOBAL_CONFIG_USER.clone()),
            ..PitchforkTomlDaemon::default()
        };
        let id = DaemonId::try_new("global", "api").unwrap();
        assert_eq!(auto_host_for_daemon(&id, &config), None);
    }

    /// A registered slug wins over the automatic hostname, because the proxy
    /// resolves slugs first.
    #[test]
    fn test_host_for_daemon_prefers_a_registered_slug() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("slug-repo");
        std::fs::create_dir_all(&repo).unwrap();
        write_config(&repo, &[("api", "")]);
        let (id, config) = daemon_config(&repo, "api");

        let empty = indexmap::IndexMap::new();
        assert_eq!(
            host_for_daemon(&id, Some(&config), &empty).as_deref(),
            Some("api.slug-repo")
        );

        // A slug registered for the same daemon replaces it, dots and all.
        let mut slugs = indexmap::IndexMap::new();
        slugs.insert(
            "myapp".to_string(),
            crate::pitchfork_toml::SlugEntry {
                dir: Some(repo.clone()),
                namespace: Some(id.namespace().to_string()),
                daemon: Some("api".to_string()),
            },
        );
        assert_eq!(
            host_for_daemon(&id, Some(&config), &slugs).as_deref(),
            Some("myapp")
        );
    }

    /// A `.git` file that is not a worktree pointer (a submodule) is its own
    /// checkout rather than a worktree of something else.
    #[test]
    fn test_detect_checkout_submodule_pointer() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir_all(repo.join(".git/modules/sub")).unwrap();
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), "gitdir: ../.git/modules/sub\n").unwrap();

        let found = detect_checkout(&sub);
        assert_eq!(found.primary, canonical(&sub));
        assert_eq!(found.worktree, None);
    }

    /// Without a `.git` anywhere above it, a directory is its own project.
    #[test]
    fn test_detect_checkout_without_git() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();

        let found = detect_checkout(&dir);
        assert_eq!(found.primary, canonical(&dir));
        assert_eq!(found.worktree, None);
    }

    fn registry() -> HostRegistry {
        let mut projects = HashMap::new();
        let mut worktrees = HashMap::new();
        worktrees.insert(
            "fix-1".to_string(),
            checkout("/repos/fix-1", "fix-1", &[("api", "api"), ("web", "web")]),
        );
        projects.insert(
            "myproj".to_string(),
            ProjectHosts {
                label: "myproj".to_string(),
                primary: checkout("/repos/myproj", "myproj", &[("api", "api")]),
                worktrees,
            },
        );
        HostRegistry {
            projects,
            errors: vec![],
        }
    }

    #[test]
    fn test_resolve_primary_checkout_daemon() {
        let target = registry().resolve("api.myproj", true);
        assert_eq!(
            target,
            HostTarget::Daemon {
                project: "myproj".into(),
                worktree: None,
                dir: PathBuf::from("/repos/myproj"),
                namespace: "myproj".into(),
                daemon: "api".into(),
            }
        );
    }

    #[test]
    fn test_resolve_worktree_daemon() {
        let target = registry().resolve("web.fix-1.myproj", true);
        assert_eq!(
            target,
            HostTarget::Daemon {
                project: "myproj".into(),
                worktree: Some("fix-1".into()),
                dir: PathBuf::from("/repos/fix-1"),
                namespace: "fix-1".into(),
                daemon: "web".into(),
            }
        );
    }

    #[test]
    fn test_resolve_is_case_insensitive() {
        assert!(matches!(
            registry().resolve("API.MyProj", true),
            HostTarget::Daemon { .. }
        ));
    }

    /// Project and stack pages are reserved: they never resolve to a daemon.
    #[test]
    fn test_resolve_reserved_pages() {
        assert_eq!(
            registry().resolve("myproj", true),
            HostTarget::ProjectPage {
                project: "myproj".into()
            }
        );
        assert_eq!(
            registry().resolve("fix-1.myproj", true),
            HostTarget::WorktreePage {
                project: "myproj".into(),
                worktree: "fix-1".into(),
            }
        );
    }

    /// When a worktree label equals a daemon name, the worktree wins: the
    /// worktree is consumed first, so `api.myproj` is that stack's page and
    /// the primary checkout's `api` daemon is unreachable under that spelling.
    #[test]
    fn test_resolve_worktree_beats_daemon_of_same_name() {
        let mut reg = registry();
        // A worktree whose label is also a daemon name of the primary checkout.
        reg.projects.get_mut("myproj").unwrap().worktrees.insert(
            "api".to_string(),
            checkout("/repos/api-wt", "api-wt", &[("api", "api")]),
        );
        assert_eq!(
            reg.resolve("api.myproj", true),
            HostTarget::WorktreePage {
                project: "myproj".into(),
                worktree: "api".into(),
            }
        );
        // The daemon inside that worktree is still reachable.
        assert!(matches!(
            reg.resolve("api.api.myproj", true),
            HostTarget::Daemon { .. }
        ));
    }

    #[test]
    fn test_resolve_wildcard_subdomain() {
        let target = registry().resolve("tenant.api.myproj", true);
        assert!(matches!(target, HostTarget::Daemon { daemon, .. } if daemon == "api"));
        // Wildcards off: the extra label is not a daemon of its own.
        assert!(matches!(
            registry().resolve("tenant.api.myproj", false),
            HostTarget::UnknownDaemon { .. }
        ));
    }

    #[test]
    fn test_resolve_unknown_names() {
        assert_eq!(
            registry().resolve("api.other", true),
            HostTarget::UnknownProject {
                known: vec!["myproj".to_string()]
            }
        );
        assert_eq!(
            registry().resolve("nope.myproj", true),
            HostTarget::UnknownDaemon {
                project: "myproj".into(),
                worktree: None,
                known: vec!["api".to_string()],
            }
        );
    }

    /// Build a primary checkout and a linked worktree of it on disk, with the
    /// same pair of pointers real `git worktree add` writes.
    fn git_project(temp: &Path, name: &str, worktree: &str) -> (PathBuf, PathBuf) {
        let repo = temp.join(name);
        let admin = repo.join(format!(".git/worktrees/{worktree}"));
        std::fs::create_dir_all(&admin).unwrap();
        let wt = temp.join(worktree);
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        std::fs::write(
            admin.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        (repo, wt)
    }

    /// Without an explicit namespace the project label is the primary
    /// checkout's directory name, sanitized.
    #[test]
    fn test_project_label_from_directory_name() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("My App");
        std::fs::create_dir_all(&repo).unwrap();
        assert_eq!(project_label(&repo).as_deref(), Some("my-app"));
    }

    /// A project that declares a namespace uses that name instead.
    #[test]
    fn test_project_label_from_explicit_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("checkout-dir");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("pitchfork.toml"), "namespace = \"storefront\"\n").unwrap();
        assert_eq!(project_label(&repo).as_deref(), Some("storefront"));
    }

    /// Write a `pitchfork.toml` with one daemon per entry, each with a port.
    fn write_config(dir: &Path, daemons: &[(&str, &str)]) {
        let body: String = daemons
            .iter()
            .enumerate()
            .map(|(i, (name, extra))| {
                format!(
                    "[daemons.{name}]\nrun = \"server\"\nport = {}\n{extra}\n",
                    3000 + i
                )
            })
            .collect();
        std::fs::write(dir.join("pitchfork.toml"), body).unwrap();
    }

    /// The daemon config as the merged config holds it, anchored at `dir`.
    fn daemon_config(dir: &Path, name: &str) -> (DaemonId, PitchforkTomlDaemon) {
        let pt = PitchforkToml::all_merged_from(dir).unwrap();
        let (id, config) = pt
            .daemons
            .iter()
            .find(|(id, _)| id.name() == name)
            .unwrap_or_else(|| panic!("no daemon '{name}' in {}", dir.display()));
        (id.clone(), config.clone())
    }

    /// A daemon in the primary checkout omits the worktree label; the same
    /// daemon in a linked worktree carries it.
    #[test]
    fn test_auto_host_primary_and_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let (repo, wt) = git_project(temp.path(), "my-repo", "fix-1");
        write_config(&repo, &[("api", "")]);
        write_config(&wt, &[("api", "")]);

        let (id, config) = daemon_config(&repo, "api");
        assert_eq!(
            auto_host_for_daemon(&id, &config).as_deref(),
            Some("api.my-repo")
        );

        let (wt_id, wt_config) = daemon_config(&wt, "api");
        assert_eq!(
            auto_host_for_daemon(&wt_id, &wt_config).as_deref(),
            Some("api.fix-1.my-repo")
        );
    }

    /// `worktree_label` in the worktree's own config replaces its directory name.
    #[test]
    fn test_worktree_label_override() {
        let temp = tempfile::tempdir().unwrap();
        let (_repo, wt) = git_project(temp.path(), "my-repo", "sleepy-kapitsa-9f02fc");
        assert_eq!(
            worktree_label(&wt).as_deref(),
            Some("sleepy-kapitsa-9f02fc")
        );

        std::fs::write(wt.join("pitchfork.toml"), "worktree_label = \"Fix 1\"\n").unwrap();
        assert_eq!(worktree_label(&wt).as_deref(), Some("fix-1"));
    }

    /// A daemon opts out with `proxy = false` and gets no hostname; one without
    /// a port never had one to begin with; a string renames its label.
    #[test]
    fn test_auto_host_requires_port_and_opt_in() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("opt-in-repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("pitchfork.toml"),
            "[daemons.api]\nrun = \"server\"\nport = 3000\n\n\
             [daemons.admin]\nrun = \"server\"\nport = 3001\nproxy = false\n\n\
             [daemons.web-frontend]\nrun = \"server\"\nport = 3002\nproxy = \"web\"\n\n\
             [daemons.worker]\nrun = \"server\"\n",
        )
        .unwrap();

        let host = |name: &str| {
            let (id, config) = daemon_config(&repo, name);
            auto_host_for_daemon(&id, &config)
        };
        assert_eq!(host("api").as_deref(), Some("api.opt-in-repo"));
        assert_eq!(host("admin"), None);
        assert_eq!(host("web-frontend").as_deref(), Some("web.opt-in-repo"));
        assert_eq!(host("worker"), None);
    }

    /// Two daemons whose labels reduce to the same string are both dropped,
    /// and neither is advertised as a URL.
    #[test]
    fn test_daemon_label_collision_drops_both() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("dupe-repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("pitchfork.toml"),
            "[daemons.foo_bar]\nrun = \"server\"\nport = 3000\n\n\
             [daemons.foo-bar]\nrun = \"server\"\nport = 3001\n\n\
             [daemons.other]\nrun = \"server\"\nport = 3002\n",
        )
        .unwrap();

        let (hosts, errors) = CheckoutHosts::load(&repo).unwrap();
        assert_eq!(hosts.labels(), vec!["other".to_string()]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("foo-bar"), "{}", errors[0]);

        for name in ["foo_bar", "foo-bar"] {
            let (id, config) = daemon_config(&repo, name);
            assert_eq!(auto_host_for_daemon(&id, &config), None, "{name}");
        }
        let (id, config) = daemon_config(&repo, "other");
        assert_eq!(
            auto_host_for_daemon(&id, &config).as_deref(),
            Some("other.dupe-repo")
        );
    }

    /// A name that cannot fit alongside the configured TLD is refused by every
    /// surface, so nothing advertises an address DNS would reject.
    #[test]
    fn test_hostname_fits() {
        // The default TLD leaves room for three maximum-length labels.
        assert!(hostname_fits(&format!(
            "{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63)
        )));
        assert!(!hostname_fits(&"a".repeat(MAX_HOSTNAME_LEN)));
    }

    /// Two worktrees whose labels collide are dropped from the project, and
    /// neither advertises a URL the proxy would refuse to route.
    #[test]
    fn test_auto_host_none_for_colliding_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("wt-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write_config(&repo, &[("api", "")]);

        let mut worktrees = Vec::new();
        for dir_name in ["fix-1", "fix.1"] {
            let admin = repo.join(format!(".git/worktrees/{dir_name}"));
            std::fs::create_dir_all(&admin).unwrap();
            let wt = temp.path().join(dir_name);
            std::fs::create_dir_all(&wt).unwrap();
            std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
            std::fs::write(
                admin.join("gitdir"),
                format!("{}\n", wt.join(".git").display()),
            )
            .unwrap();
            write_config(&wt, &[("api", "")]);
            worktrees.push(wt);
        }

        for wt in &worktrees {
            let (id, config) = daemon_config(wt, "api");
            assert_eq!(
                auto_host_for_daemon(&id, &config),
                None,
                "{} must not advertise a hostname",
                wt.display()
            );
        }
        // The primary checkout is unaffected by its worktrees' collision.
        let (id, config) = daemon_config(&repo, "api");
        assert_eq!(
            auto_host_for_daemon(&id, &config).as_deref(),
            Some("api.wt-repo")
        );
    }

    /// Checkouts that share a namespace cannot be told apart by daemon ID,
    /// which the proxy has to know before trusting a state record. Namespaces
    /// come from directory names, so the clash can span unrelated projects.
    #[test]
    fn test_shares_daemon_id() {
        let project = |label: &str, primary: CheckoutHosts, wts: Vec<(&str, CheckoutHosts)>| {
            (
                label.to_string(),
                ProjectHosts {
                    label: label.to_string(),
                    primary,
                    worktrees: wts.into_iter().map(|(l, c)| (l.to_string(), c)).collect(),
                },
            )
        };

        // One project, one worktree, each with its own namespace.
        let mut registry = HostRegistry {
            projects: HashMap::from([project(
                "myproj",
                checkout("/repos/myproj", "myproj", &[("api", "api")]),
                vec![(
                    "fix-1",
                    checkout("/repos/fix-1", "fix-1", &[("api", "api")]),
                )],
            )]),
            errors: vec![],
        };
        assert!(!registry.shares_daemon_id("myproj", "api"));
        assert!(!registry.shares_daemon_id("fix-1", "api"));

        // A worktree inheriting the project's explicit namespace clashes with
        // the primary checkout.
        registry
            .projects
            .get_mut("myproj")
            .unwrap()
            .worktrees
            .insert(
                "fix-2".to_string(),
                checkout("/repos/fix-2", "myproj", &[("api", "api")]),
            );
        assert!(registry.shares_daemon_id("myproj", "api"));

        // So does an identically named worktree of an unrelated project, whose
        // namespace is its directory name.
        let (label, other) = project(
            "other",
            checkout("/repos/other", "other", &[("api", "api")]),
            vec![(
                "fix-1",
                checkout("/repos/other/fix-1", "fix-1", &[("api", "api")]),
            )],
        );
        registry.projects.insert(label, other);
        assert!(registry.shares_daemon_id("fix-1", "api"));

        // A daemon only one checkout defines stays unambiguous.
        assert!(!registry.shares_daemon_id("myproj", "worker"));
    }

    /// Two worktrees reducing to one label are both dropped, and the error
    /// names both directories.
    #[test]
    fn test_worktree_label_collision_drops_both() {
        let found = vec![
            (
                "fix-1".to_string(),
                checkout("/repos/fix-1", "fix-1", &[("api", "api")]),
            ),
            (
                "fix-1".to_string(),
                checkout("/repos/fix.1", "fix-1b", &[("api", "api")]),
            ),
            (
                "fix-2".to_string(),
                checkout("/repos/fix-2", "fix-2", &[("api", "api")]),
            ),
        ];
        let (kept, errors) = group_worktrees("myproj", found);
        assert_eq!(kept.keys().collect::<Vec<_>>(), vec!["fix-2"]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("/repos/fix-1"), "{}", errors[0]);
        assert!(errors[0].contains("/repos/fix.1"), "{}", errors[0]);
    }

    /// Two projects that reduce to one label are reported, and neither is
    /// routed: answering for one would serve the wrong project's daemons.
    #[test]
    fn test_project_label_collision_routes_neither() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a/shop");
        let b = temp.path().join("b/shop");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        write_config(&a, &[("api", "")]);
        write_config(&b, &[("api", "")]);

        let registry = HostRegistry::from_dirs(&[a.clone(), b.clone()]);
        assert!(registry.project_labels().is_empty());
        assert_eq!(registry.errors.len(), 1);
        assert!(
            registry.errors[0].contains("shop"),
            "{}",
            registry.errors[0]
        );
        // The error names the checkout as the registry resolved it, which is
        // not always how the test spelled the path.
        assert!(
            registry.errors[0].contains(&canonical(&b).display().to_string()),
            "{}",
            registry.errors[0]
        );
        assert_eq!(
            registry.resolve("api.shop", true),
            HostTarget::UnknownProject { known: vec![] }
        );
    }
}
