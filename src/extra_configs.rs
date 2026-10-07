//! External files are associated with a project, never with their storage directory.
use crate::Result;
use crate::env;
use crate::pitchfork_toml::{NamespaceEntry, NamespaceEntryRaw, PitchforkToml, current_meta};
use indexmap::IndexMap;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub namespace: String,
    pub dir: PathBuf,
    pub config: Vec<PathBuf>,
    /// Hostname label the registration names for the project, if any.
    pub label: Option<String>,
    pub source: &'static str,
}

#[derive(Default, Deserialize)]
struct Registrations {
    #[serde(default)]
    namespaces: IndexMap<String, NamespaceEntryRaw>,
}

#[derive(Default)]
struct Cache {
    initialized: bool,
    meta: Option<(SystemTime, u64)>,
    entries: Vec<Entry>,
    /// Registered labels keyed by the primary checkout they name, built from
    /// `entries` on first use so a hostname lookup never walks the filesystem.
    label_index: Option<(
        std::time::Instant,
        std::collections::HashMap<PathBuf, String>,
    )>,
}
static CACHE: Lazy<Mutex<Cache>> = Lazy::new(|| Mutex::new(Cache::default()));

/// Canonicalize existing paths and normalize missing paths for unregistering.
pub fn normalize(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return dunce::simplified(&path).to_path_buf();
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::CWD.join(path)
    };
    if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name())
        && let Ok(parent) = parent.canonicalize()
    {
        return dunce::simplified(&parent).join(name);
    }
    let mut result = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            part => result.push(part.as_os_str()),
        }
    }
    result
}

/// Whether the filesystem positively reports `path` as nonexistent. An I/O
/// error (permissions, a flaky mount) is not "gone": callers delete on this.
pub fn path_is_gone(path: &Path) -> bool {
    matches!(std::fs::exists(path), Ok(false))
}

/// Whether the state file records a running daemon in `namespace`. A
/// namespace with live daemons must keep its registration, or a new project
/// registered under the name would find the old project's running daemons.
pub fn namespace_has_running_daemon(namespace: &str) -> bool {
    crate::state_file::StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .map(|state| {
            state
                .daemons
                .iter()
                .any(|(id, d)| d.pid.is_some() && id.namespace() == namespace)
        })
        // Unknown state: err on the side of keeping the registration.
        .unwrap_or(true)
}

/// Registration observed before a start runs hooks or waits for ports.
pub fn namespace_start_snapshot(namespace: &str) -> Result<Option<NamespaceEntry>> {
    Ok(PitchforkToml::read(&*env::PITCHFORK_GLOBAL_CONFIG_USER)
        .ok()
        .and_then(|config| config.namespaces.get(namespace).cloned()))
}

/// Serialize spawning and publishing its PID with registry mutations. Hooks
/// run before this lock, so they can safely register or start other daemons.
pub fn namespace_start_guard(
    namespace: &str,
    expected: Option<&NamespaceEntry>,
) -> Result<Option<xx::fslock::LockFile>> {
    namespace_start_guard_in(&env::PITCHFORK_GLOBAL_CONFIG_USER, namespace, expected)
}

fn namespace_start_guard_in(
    path: &Path,
    namespace: &str,
    expected: Option<&NamespaceEntry>,
) -> Result<Option<xx::fslock::LockFile>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).into_diagnostic()?;
    }
    let lock = xx::fslock::get(path, false).into_diagnostic()?;
    let pt = match std::fs::read_to_string(path) {
        Ok(raw) => match PitchforkToml::parse_str(&raw, path) {
            Ok(config) => config,
            // Registry writers cannot mutate an unreadable configuration.
            // Keep their lock, but let valid project daemons start as before.
            Err(_) => return Ok(lock),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            PitchforkToml::new(path.to_path_buf())
        }
        Err(_) => return Ok(lock),
    };
    if pt.namespaces.get(namespace) != expected {
        miette::bail!(
            "namespace '{namespace}' changed while starting; retry with its current configuration"
        );
    }
    Ok(lock)
}

pub fn resolve_path(dir: &Path, path: &str) -> PathBuf {
    let path = env::expand_tilde(path);
    normalize(&if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

fn parse_entries(content: &str) -> Result<Vec<Entry>> {
    let raw: Registrations = toml::from_str(content).into_diagnostic()?;
    Ok(raw
        .namespaces
        .into_iter()
        .filter(|(_, e)| !e.config.is_empty())
        .map(|(namespace, entry)| {
            let dir = normalize(&env::expand_tilde(entry.dir));
            let config = entry.config.iter().map(|p| resolve_path(&dir, p)).collect();
            Entry {
                namespace,
                dir,
                config,
                label: entry.label,
                source: "registry",
            }
        })
        .collect())
}

pub fn entries() -> Vec<Entry> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let meta = current_meta(&env::PITCHFORK_GLOBAL_CONFIG_USER);
    if !cache.initialized || cache.meta != meta {
        cache.entries = if meta.is_some() {
            std::fs::read_to_string(&*env::PITCHFORK_GLOBAL_CONFIG_USER)
                .into_diagnostic()
                .and_then(|s| parse_entries(&s))
                .unwrap_or_else(|e| {
                    warn!("cannot read external configuration registry: {e}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        cache.meta = meta;
        cache.initialized = true;
        cache.label_index = None;
    }
    let mut entries = cache.entries.clone();
    drop(cache);
    if let Some(value) = std::env::var_os("PITCHFORK_CONFIG") {
        let cwd = normalize(&env::CWD);
        let dir = xx::file::find_up_all(
            &cwd,
            &[
                "pitchfork.local.toml",
                "pitchfork.toml",
                ".config/pitchfork.local.toml",
                ".config/pitchfork.toml",
            ],
        )
        .into_iter()
        .next()
        .and_then(|path| {
            let parent = path.parent()?;
            Some(
                if parent.file_name().is_some_and(|name| name == ".config") {
                    parent.parent()?.to_path_buf()
                } else {
                    parent.to_path_buf()
                },
            )
        })
        .unwrap_or_else(|| cwd.clone());
        let config = std::env::split_paths(&value)
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| resolve_path(&cwd, &p.to_string_lossy()))
            .collect();
        // Avoid calling namespace_for_project_dir here: namespace discovery uses this registry.
        entries.push(Entry {
            namespace: String::new(),
            dir,
            config,
            label: None,
            source: "env",
        });
    }
    entries
}

pub fn invalidate() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Cache::default();
}

pub fn paths_for(cwd: &Path) -> Vec<PathBuf> {
    let cwd = normalize(cwd);
    let mut entries: Vec<_> = entries()
        .into_iter()
        .filter(|e| cwd.starts_with(&e.dir))
        .collect();
    entries.sort_by_key(|e| (e.source == "env", e.dir.components().count()));
    let mut paths = Vec::new();
    for entry in entries {
        for path in entry.config {
            // Last attachment wins, even if it is also in PITCHFORK_CONFIG.
            paths.retain(|p| p != &path);
            paths.push(path);
        }
    }
    paths
}

/// Configuration files registered for this exact project directory.
///
/// Unlike [`paths_for`], this does not include files registered for an
/// ancestor: they belong to that project, not to this one.
pub fn configs_for_dir(dir: &Path) -> Vec<PathBuf> {
    let dir = normalize(dir);
    entries()
        .into_iter()
        .filter(|e| e.dir == dir)
        .flat_map(|e| e.config)
        .collect()
}

pub fn project_dir(path: &Path) -> Option<PathBuf> {
    let path = normalize(path);
    entries()
        .into_iter()
        .rev()
        .find(|e| e.config.contains(&path))
        .map(|e| e.dir)
}

pub fn namespace_for_dir(dir: &Path) -> Option<String> {
    let dir = normalize(dir);
    entries()
        .into_iter()
        .find(|e| e.source == "registry" && e.dir == dir)
        .map(|e| e.namespace)
}

/// How long a built label index answers lookups. Long enough that one hostname
/// pass over every daemon reads the filesystem once, short enough that a moved
/// checkout is picked up without a registry change.
const LABEL_INDEX_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// The hostname label a registration names for the project whose primary
/// checkout is `primary`.
///
/// A pure lookup in the cached registry. It is what lets a tool register a
/// project under a generated namespace (needed to keep daemon IDs unique) and
/// still get the hostname it advertises.
///
/// A registration made from the primary checkout wins. Failing that, one made
/// from a linked worktree of it counts too: a tool registers the checkout it
/// runs in, which is often a worktree, and the project's hostnames must not
/// change depending on which checkout registered first.
pub fn label_for_checkout(primary: &Path) -> Option<String> {
    // Refreshes the cache, and drops the index with it, when the registry changed.
    drop(entries());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let Cache {
        entries,
        label_index,
        ..
    } = &mut *cache;
    // The index records where Git pointers led, which moving a checkout changes
    // without touching the registry, so it only serves lookups for a moment.
    if label_index
        .as_ref()
        .is_none_or(|(built, _)| built.elapsed() > LABEL_INDEX_TTL)
    {
        *label_index = Some((
            std::time::Instant::now(),
            label_index_of(entries, crate::proxy::hostname::detect_checkout),
        ));
    }
    label_index
        .as_ref()
        .and_then(|(_, index)| index.get(&normalize(primary)).cloned())
}

/// Every registered label, keyed by the primary checkout it names.
///
/// A registration made from the primary itself wins over one made from a linked
/// worktree, and among worktrees the lowest path wins, so the result never
/// depends on registry order.
fn label_index_of(
    entries: &[Entry],
    checkout_of: impl Fn(&Path) -> crate::proxy::hostname::Checkout,
) -> std::collections::HashMap<PathBuf, String> {
    let mut chosen: std::collections::HashMap<PathBuf, (bool, &Path, &str)> = Default::default();
    for entry in entries {
        let Some(label) = entry
            .label
            .as_deref()
            .filter(|_| entry.source == "registry")
        else {
            continue;
        };
        let checkout = checkout_of(&entry.dir);
        // A registration for a subdirectory (a monorepo package, say) names
        // that directory's project, not the repository around it.
        if checkout.root() != entry.dir {
            continue;
        }
        let primary = checkout.primary.clone();
        let candidate = (checkout.worktree.is_some(), entry.dir.as_path(), label);
        match chosen.get(&primary) {
            Some(best) if (best.0, best.1) <= (candidate.0, candidate.1) => {}
            _ => {
                chosen.insert(primary, candidate);
            }
        }
    }
    chosen
        .into_iter()
        .map(|(primary, (_, _, label))| (primary, label.to_string()))
        .collect()
}

/// Mutate under the same lock as the existing namespace and slug writers.
///
/// A `label` replaces the one already recorded for the namespace; `None` leaves
/// it alone. Returns whether anything was written.
pub fn add(namespace: &str, dir: &Path, file: &Path, label: Option<&str>) -> Result<bool> {
    let path = &*env::PITCHFORK_GLOBAL_CONFIG_USER;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).into_diagnostic()?;
    }
    let _lock = xx::fslock::get(path, false).into_diagnostic()?;
    let mut pt = if path.exists() {
        PitchforkToml::parse_str(&std::fs::read_to_string(path).into_diagnostic()?, path)?
    } else {
        PitchforkToml::new(path.clone())
    };
    let dir = normalize(dir);
    let file = normalize(file);
    // A registration for this namespace whose directory is gone (a deleted
    // scratch project, say) must not block registering it at a new directory.
    let mut replaced_stale = false;
    if let Some(old) = pt.namespaces.get(namespace)
        && normalize(&old.dir) != dir
        && path_is_gone(&old.dir)
        && !namespace_has_running_daemon(namespace)
    {
        pt.namespaces.shift_remove(namespace);
        replaced_stale = true;
    }
    for (name, entry) in &pt.namespaces {
        if (name == namespace && normalize(&entry.dir) != dir)
            || (name != namespace
                && (entry.config.contains(&file)
                    || (!entry.config.is_empty() && normalize(&entry.dir) == dir)))
        {
            miette::bail!(
                "external configuration conflicts with namespace '{name}' ({})",
                entry.dir.display()
            );
        }
    }
    let entry = pt
        .namespaces
        .entry(namespace.to_string())
        .or_insert_with(|| crate::pitchfork_toml::NamespaceEntry {
            dir,
            config: Vec::new(),
            label: None,
        });
    let mut changed = replaced_stale;
    if let Some(label) = label
        && entry.label.as_deref() != Some(label)
    {
        entry.label = Some(label.to_string());
        changed = true;
    }
    if !entry.config.contains(&file) {
        entry.config.push(file);
        changed = true;
    }
    if changed {
        pt.write_unlocked()?;
    }
    Ok(changed)
}

/// Detach `file` from a registration, reporting whether it was attached.
///
/// A registration with no attachment left is not a registration: `entries()`
/// drops it, so it neither names the project's namespace nor its label. The
/// label goes with the last attachment so that it cannot silently come back
/// when the directory is registered again without `--label`.
fn detach_file(entry: &mut NamespaceEntry, file: &Path) -> bool {
    let before = entry.config.len();
    entry.config.retain(|p| p != file);
    let detached = before != entry.config.len();
    if detached && entry.config.is_empty() {
        entry.label = None;
    }
    detached
}

pub fn remove(file: &Path) -> Result<Option<String>> {
    let path = &*env::PITCHFORK_GLOBAL_CONFIG_USER;
    if !path.exists() {
        return Ok(None);
    }
    let _lock = xx::fslock::get(path, false).into_diagnostic()?;
    let mut pt = if path.exists() {
        PitchforkToml::parse_str(&std::fs::read_to_string(path).into_diagnostic()?, path)?
    } else {
        PitchforkToml::new(path.clone())
    };
    let file = normalize(file);
    let mut removed = None;
    for (name, entry) in &mut pt.namespaces {
        if detach_file(entry, &file) {
            removed = Some(name.clone());
        }
    }
    if removed.is_some() {
        pt.write_unlocked()?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_start_holds_registry_writers_until_pid_publication() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let guard = namespace_start_guard_in(&path, "app", None).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _lock = xx::fslock::get(&path, false).unwrap();
            locked_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            locked_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(guard);
        locked_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn malformed_global_config_does_not_block_project_starts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[namespaces\ninvalid").unwrap();
        assert!(namespace_start_guard_in(&path, "app", None).is_ok());
        let old = NamespaceEntry {
            dir: PathBuf::from("/project"),
            config: vec![],
            label: None,
        };
        assert!(namespace_start_guard_in(&path, "app", Some(&old)).is_ok());
    }

    #[test]
    fn a_registration_changed_during_start_cannot_spawn_old_options() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[namespaces.app]\ndir = '/new-project'\n").unwrap();
        assert!(namespace_start_guard_in(&path, "app", None).is_err());
        let current = PitchforkToml::read(&path).unwrap();
        let old = NamespaceEntry {
            dir: PathBuf::from("/old-project"),
            config: vec![],
            label: None,
        };
        assert!(namespace_start_guard_in(&path, "app", Some(&old)).is_err());
        assert!(namespace_start_guard_in(&path, "app", current.namespaces.get("app")).is_ok());
        std::fs::remove_file(&path).unwrap();
        assert!(namespace_start_guard_in(&path, "app", current.namespaces.get("app")).is_err());
    }
    #[test]
    fn registry_paths_are_relative_to_project_and_old_entries_are_compatible() {
        let tmp = tempfile::tempdir().unwrap();
        let root = normalize(tmp.path());
        let project = root.join("project");
        let external = root.join("state/app.toml");
        let project_str = project.to_string_lossy().into_owned();
        let external_str = external.to_string_lossy().into_owned();
        let doc = toml::toml! {
            [namespaces.old]
            dir = "/old"
            [namespaces.app]
            dir = (project_str)
            config = ["generated.toml", (external_str)]
        };
        let entries = parse_entries(&toml::to_string(&doc).unwrap()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].config,
            vec![
                normalize(&project.join("generated.toml")),
                normalize(&external)
            ]
        );
        let raw = NamespaceEntryRaw {
            dir: "/old".into(),
            config: vec![],
            label: None,
        };
        let text = toml::to_string(&raw).unwrap();
        assert!(!text.contains("config"));
        assert!(!text.contains("label"));
        assert_eq!(entries[0].label, None);
    }

    /// The label lives exactly as long as the registration: it survives while
    /// any attachment remains and goes with the last one.
    #[test]
    fn detach_file_drops_label_with_last_attachment() {
        let a = PathBuf::from("/gen/a.toml");
        let b = PathBuf::from("/gen/b.toml");
        let mut entry = NamespaceEntry {
            dir: PathBuf::from("/shop"),
            config: vec![a.clone(), b.clone()],
            label: Some("shop".into()),
        };
        assert!(!detach_file(&mut entry, Path::new("/gen/none.toml")));
        assert_eq!(entry.label.as_deref(), Some("shop"));
        assert!(detach_file(&mut entry, &a));
        assert_eq!(entry.config, vec![b.clone()]);
        assert_eq!(entry.label.as_deref(), Some("shop"));
        assert!(detach_file(&mut entry, &b));
        assert!(entry.config.is_empty());
        assert_eq!(entry.label, None);
        // Removing again is a no-op and cannot resurrect anything.
        assert!(!detach_file(&mut entry, &b));
    }

    #[test]
    fn registry_label_parses_and_round_trips() {
        let doc = "[namespaces.shop-528f92b13a6784f0]\ndir = \"/shop\"\n\
                   config = [\"/gen/pitchfork.toml\"]\nlabel = \"shop\"\n";
        let entries = parse_entries(doc).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].namespace, "shop-528f92b13a6784f0");
        assert_eq!(entries[0].label.as_deref(), Some("shop"));
        let raw: Registrations = toml::from_str(doc).unwrap();
        let again = toml::to_string(&raw.namespaces["shop-528f92b13a6784f0"]).unwrap();
        assert!(again.contains("label = \"shop\""), "{again}");
    }

    fn labelled(dir: &Path, label: Option<&str>) -> Entry {
        Entry {
            namespace: format!("ns-{}", dir.display()),
            dir: normalize(dir),
            config: vec![],
            label: label.map(String::from),
            source: "registry",
        }
    }

    /// A real primary checkout with linked worktrees named `names`, so the Git
    /// pointer files that `detect_checkout` follows are what the tests use.
    fn repo_with_worktrees(names: &[&str]) -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("shop");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let worktrees = names
            .iter()
            .map(|name| {
                let admin = repo.join(".git/worktrees").join(name);
                std::fs::create_dir_all(&admin).unwrap();
                let wt = temp.path().join(name);
                std::fs::create_dir_all(&wt).unwrap();
                std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
                wt
            })
            .collect();
        (temp, repo, worktrees)
    }

    fn pick(entries: &[Entry], primary: &Path) -> Option<String> {
        label_index_of(entries, crate::proxy::hostname::detect_checkout)
            .get(&normalize(primary))
            .cloned()
    }

    #[test]
    fn label_registered_from_a_linked_worktree_names_the_project() {
        let (temp, repo, worktrees) = repo_with_worktrees(&["feature"]);
        let entries = vec![labelled(&worktrees[0], Some("shop-web"))];
        assert_eq!(pick(&entries, &repo).as_deref(), Some("shop-web"));
        // Another project is not claimed by it.
        let other = temp.path().join("other");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        assert_eq!(pick(&entries, &other), None);
    }

    #[test]
    fn primary_checkout_label_beats_a_worktree_label() {
        let (_temp, repo, worktrees) = repo_with_worktrees(&["feature"]);
        let entries = vec![
            labelled(&worktrees[0], Some("from-worktree")),
            labelled(&repo, Some("from-primary")),
        ];
        assert_eq!(pick(&entries, &repo).as_deref(), Some("from-primary"));
    }

    #[test]
    fn worktree_labels_are_chosen_independently_of_registry_order() {
        let (_temp, repo, worktrees) = repo_with_worktrees(&["a", "b"]);
        let a = labelled(&worktrees[0], Some("label-a"));
        let b = labelled(&worktrees[1], Some("label-b"));
        for entries in [vec![a.clone(), b.clone()], vec![b, a]] {
            assert_eq!(pick(&entries, &repo).as_deref(), Some("label-a"));
        }
    }

    #[test]
    fn a_label_registered_for_a_subdirectory_does_not_name_the_repository() {
        let (_temp, repo, worktrees) = repo_with_worktrees(&["feature"]);
        let package = repo.join("packages/web");
        let wt_package = worktrees[0].join("packages/web");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(&wt_package).unwrap();
        let entries = vec![
            labelled(&package, Some("web-package")),
            labelled(&wt_package, Some("web-package-wt")),
        ];
        assert_eq!(pick(&entries, &repo), None);
    }

    #[test]
    fn unlabelled_registrations_name_nothing() {
        let (_temp, repo, worktrees) = repo_with_worktrees(&["feature"]);
        let entries = vec![labelled(&worktrees[0], None)];
        assert_eq!(pick(&entries, &repo), None);
    }
}
