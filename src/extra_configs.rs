//! External files are associated with a project, never with their storage directory.
use crate::Result;
use crate::env;
use crate::pitchfork_toml::{NamespaceEntryRaw, PitchforkToml, current_meta};
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
    }
    let mut entries = cache.entries.clone();
    drop(cache);
    if let Some(value) = std::env::var_os("PITCHFORK_CONFIG") {
        let dir = normalize(&env::CWD);
        let config = std::env::split_paths(&value)
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| resolve_path(&dir, &p.to_string_lossy()))
            .collect();
        // Avoid calling namespace_for_project_dir here: namespace discovery uses this registry.
        entries.push(Entry {
            namespace: String::new(),
            dir,
            config,
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

/// Mutate under the same lock as the existing namespace and slug writers.
pub fn add(namespace: &str, dir: &Path, file: &Path) -> Result<bool> {
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
        });
    if entry.config.contains(&file) {
        return Ok(false);
    }
    entry.config.push(file);
    pt.write_unlocked()?;
    Ok(true)
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
        let before = entry.config.len();
        entry.config.retain(|p| p != &file);
        if before != entry.config.len() {
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
        };
        assert!(!toml::to_string(&raw).unwrap().contains("config"));
    }
}
