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
    #[allow(dead_code)]
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
    common.parent().map(Path::to_path_buf)
}

/// Locate the checkout containing `dir` by walking up to the nearest `.git`.
///
/// A `.git` directory marks the primary checkout. A `.git` file whose
/// `gitdir:` points into `<common>/worktrees/<name>` marks a linked worktree,
/// whose primary checkout is the parent of the common directory. When no
/// `.git` is found the directory is treated as its own primary checkout, so
/// projects that are not git repositories still get a hostname.
pub fn detect_checkout(dir: &Path) -> Checkout {
    let start = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let start = dunce::simplified(&start).to_path_buf();
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
/// Returns `None` when the daemon configures no port, opted out with
/// `proxy = false`, or no label could be derived.
pub fn auto_host_for_daemon(id: &DaemonId, config: &PitchforkTomlDaemon) -> Option<String> {
    config.port.as_ref()?;
    let daemon = daemon_label(id.name(), config.proxy.as_ref())?;
    let base = config
        .path
        .as_deref()
        .and_then(crate::pitchfork_toml::project_dir_for_config)?;
    let checkout = detect_checkout(&base);
    let project = project_label(&checkout.primary)?;
    let worktree = checkout.worktree.as_deref().and_then(worktree_label);
    Some(join_labels(&daemon, worktree.as_deref(), &project))
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
    if let Some(slug) = PitchforkToml::find_slug_for_daemon_in_registry(id, global_slugs) {
        return Some(slug);
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
    fn load(dir: &Path) -> Option<Self> {
        let namespace = PitchforkToml::namespace_for_dir(dir).ok()?;
        let pt = PitchforkToml::all_merged_from(dir).ok()?;
        let mut daemons = HashMap::new();
        for (id, config) in &pt.daemons {
            if id.namespace() != namespace || config.port.is_none() {
                continue;
            }
            if let Some(label) = daemon_label(id.name(), config.proxy.as_ref()) {
                daemons.insert(label, id.name().to_string());
            }
        }
        Some(Self {
            dir: dir.to_path_buf(),
            namespace,
            daemons,
        })
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
        // The current directory's project, so a project is routable from its
        // own checkout before any of its daemons has ever been started.
        let mut candidates: Vec<PathBuf> = vec![crate::env::CWD.clone()];
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
        Self::from_dirs(&candidates)
    }

    /// Build the registry from an explicit list of project directories.
    pub fn from_dirs(dirs: &[PathBuf]) -> Self {
        let worktrees_enabled = crate::settings::settings().general.worktree;

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
        for primary in primaries {
            let Some(label) = project_label(&primary) else {
                continue;
            };
            let Some(primary_hosts) = CheckoutHosts::load(&primary) else {
                continue;
            };

            if let Some(existing) = registry.projects.get(&label) {
                if existing.primary.dir != primary_hosts.dir {
                    registry.errors.push(format!(
                        "project label '{label}' is claimed by two directories: {} and {}. \
                         Set a distinct top-level `namespace` in one of them.",
                        existing.primary.dir.display(),
                        primary_hosts.dir.display(),
                    ));
                }
                continue;
            }

            let mut project = ProjectHosts {
                label: label.clone(),
                primary: primary_hosts,
                worktrees: HashMap::new(),
            };

            if worktrees_enabled {
                let mut found: Vec<(String, CheckoutHosts)> = Vec::new();
                for entry in crate::proxy::worktree::discover_worktrees(&primary) {
                    let checkout = detect_checkout(&entry.path);
                    let Some(wt_dir) = checkout.worktree else {
                        continue; // the primary checkout itself
                    };
                    let (Some(wt_label), Some(hosts)) =
                        (worktree_label(&wt_dir), CheckoutHosts::load(&wt_dir))
                    else {
                        continue;
                    };
                    found.push((wt_label, hosts));
                }
                let (worktrees, errors) = group_worktrees(&label, found);
                project.worktrees = worktrees;
                registry.errors.extend(errors);
            }

            registry.projects.insert(label, project);
        }

        registry
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
        assert_eq!(found.primary, repo.canonicalize().unwrap());
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
        assert_eq!(found.primary, repo.canonicalize().unwrap());
        assert_eq!(found.worktree, Some(wt.canonicalize().unwrap()));
        assert_eq!(found.root(), wt.canonicalize().unwrap());
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
        assert_eq!(found.primary, sub.canonicalize().unwrap());
        assert_eq!(found.worktree, None);
    }

    /// Without a `.git` anywhere above it, a directory is its own project.
    #[test]
    fn test_detect_checkout_without_git() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();

        let found = detect_checkout(&dir);
        assert_eq!(found.primary, dir.canonicalize().unwrap());
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

    /// Build a primary checkout and a linked worktree of it on disk.
    fn git_project(temp: &Path, name: &str, worktree: &str) -> (PathBuf, PathBuf) {
        let repo = temp.join(name);
        std::fs::create_dir_all(repo.join(format!(".git/worktrees/{worktree}"))).unwrap();
        let wt = temp.join(worktree);
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!(
                "gitdir: {}\n",
                repo.join(format!(".git/worktrees/{worktree}")).display()
            ),
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

    /// A daemon in the primary checkout omits the worktree label; the same
    /// daemon in a linked worktree carries it.
    #[test]
    fn test_auto_host_primary_and_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let (repo, wt) = git_project(temp.path(), "my-repo", "fix-1");

        let config = |dir: &Path| PitchforkTomlDaemon {
            run: "server".to_string(),
            port: Some(crate::config_types::PortConfig {
                expect: vec![3000],
                ..Default::default()
            }),
            path: Some(dir.join("pitchfork.toml")),
            ..PitchforkTomlDaemon::default()
        };
        let id = DaemonId::try_new("my-repo", "api").unwrap();

        assert_eq!(
            auto_host_for_daemon(&id, &config(&repo)).as_deref(),
            Some("api.my-repo")
        );
        assert_eq!(
            auto_host_for_daemon(&id, &config(&wt)).as_deref(),
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
    /// a port never had one to begin with.
    #[test]
    fn test_auto_host_requires_port_and_opt_in() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir_all(&repo).unwrap();
        let id = DaemonId::try_new("my-repo", "api").unwrap();

        let no_port = PitchforkTomlDaemon {
            run: "server".to_string(),
            path: Some(repo.join("pitchfork.toml")),
            ..PitchforkTomlDaemon::default()
        };
        assert_eq!(auto_host_for_daemon(&id, &no_port), None);

        let opted_out = PitchforkTomlDaemon {
            port: Some(crate::config_types::PortConfig {
                expect: vec![3000],
                ..Default::default()
            }),
            proxy: Some(ProxyConfig::Disabled),
            ..no_port.clone()
        };
        assert_eq!(auto_host_for_daemon(&id, &opted_out), None);

        let renamed = PitchforkTomlDaemon {
            proxy: Some(ProxyConfig::Name("web".into())),
            ..opted_out.clone()
        };
        assert_eq!(
            auto_host_for_daemon(&id, &renamed).as_deref(),
            Some("web.my-repo")
        );
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

    /// Two projects that reduce to one label are reported, and only the first
    /// directory keeps the label.
    #[test]
    fn test_project_label_collision_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a/shop");
        let b = temp.path().join("b/shop");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        let registry = HostRegistry::from_dirs(&[a.clone(), b.clone()]);
        assert_eq!(registry.project_labels(), vec!["shop".to_string()]);
        assert_eq!(registry.errors.len(), 1);
        assert!(
            registry.errors[0].contains("shop"),
            "{}",
            registry.errors[0]
        );
        assert!(
            registry.errors[0].contains(&b.display().to_string()),
            "{}",
            registry.errors[0]
        );
    }
}
