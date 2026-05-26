//! Git worktree / jj workspace auto-discovery for proxy slug routing.
//!
//! Detects all git worktrees or jj workspaces for a project directory.
//! Each entry carries the path, branch/workspace name, and a sanitized
//! name suitable for use as a URL subdomain prefix.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: String,
    pub sanitized_branch: String,
    /// Namespace resolved at discovery time (cached to avoid per-request I/O).
    pub namespace: Option<String>,
}

pub fn discover_worktrees(project_dir: &Path) -> Vec<WorktreeEntry> {
    // Prefer jj workspace if .jj exists, fall back to git worktree if .git exists.
    if project_dir.join(".jj").exists() {
        discover_jj_workspaces(project_dir)
    } else if project_dir.join(".git").exists() {
        discover_git_worktrees(project_dir)
    } else {
        vec![]
    }
}

// ─── jj workspace discovery ───────────────────────────────────────────────────

fn discover_jj_workspaces(project_dir: &Path) -> Vec<WorktreeEntry> {
    let output = match Command::new("jj")
        .args(["workspace", "list"])
        .current_dir(project_dir)
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return vec![],
    };

    let names = parse_jj_workspace_names(&output);
    if names.is_empty() {
        return vec![];
    }

    // Resolve paths in parallel for non-default workspaces.
    let non_default: Vec<&str> = names
        .iter()
        .filter(|n| **n != "default")
        .map(|n| n.as_str())
        .collect();
    let mut roots = std::collections::HashMap::with_capacity(non_default.len());

    std::thread::scope(|s| {
        let handles: Vec<_> = non_default
            .iter()
            .map(|name| s.spawn(move || (*name, get_jj_workspace_root(project_dir, name))))
            .collect();

        for handle in handles {
            let (name, root) = handle.join().unwrap();
            roots.insert(name.to_string(), root);
        }
    });

    let mut entries = Vec::with_capacity(names.len());
    for name in &names {
        let path = if name == "default" {
            Some(project_dir.to_path_buf())
        } else {
            roots.get(name).cloned().unwrap_or(None)
        };

        let Some(path) = path else {
            continue;
        };

        let sanitized = sanitize_branch(name);
        if sanitized.is_empty() {
            log::warn!(
                "Skipping jj workspace '{}' because its sanitized name is empty \
                 (no ASCII alphanumeric characters)",
                name,
            );
            continue;
        }
        entries.push(WorktreeEntry {
            path,
            branch: name.to_string(),
            sanitized_branch: sanitized,
            namespace: None,
        });
    }

    entries
}
fn parse_jj_workspace_names(stdout: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(stdout);
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            line.split_once(':')
                .map(|(name, _)| name.trim().to_string())
                .filter(|n| !n.is_empty())
        })
        .collect()
}

#[allow(dead_code)]
fn parse_jj_workspace_list(
    stdout: &[u8],
    mut resolve_path: impl FnMut(&str) -> Option<PathBuf>,
) -> Vec<WorktreeEntry> {
    let text = String::from_utf8_lossy(stdout);
    let mut entries = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some((name, _)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }

        let path = match resolve_path(name) {
            Some(p) => p,
            None => continue,
        };

        let sanitized = sanitize_branch(name);
        if sanitized.is_empty() {
            log::warn!(
                "Skipping jj workspace '{}' because its sanitized name is empty \
                 (no ASCII alphanumeric characters)",
                name,
            );
            continue;
        }
        entries.push(WorktreeEntry {
            path,
            branch: name.to_string(),
            sanitized_branch: sanitized,
            namespace: None,
        });
    }

    entries
}

fn get_jj_workspace_root(project_dir: &Path, name: &str) -> Option<PathBuf> {
    let output = Command::new("jj")
        .args(["workspace", "root", "--name", name])
        .current_dir(project_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path_str = String::from_utf8_lossy(&output.stdout);
    let trimmed = path_str.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(PathBuf::from(trimmed))
}

// ─── git worktree discovery ───────────────────────────────────────────────────

fn discover_git_worktrees(project_dir: &Path) -> Vec<WorktreeEntry> {
    let output = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(project_dir)
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return vec![],
    };

    parse_git_worktree_output(&output)
}

fn parse_git_worktree_output(stdout: &[u8]) -> Vec<WorktreeEntry> {
    let text = String::from_utf8_lossy(stdout);
    let mut entries = Vec::new();
    let mut current_path = None;
    let mut current_branch = None;

    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path.trim()));
            current_branch = None;
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = Some(
                branch
                    .trim()
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch.trim())
                    .to_string(),
            );
        }
        if line.is_empty() {
            flush_git_entry(&mut entries, &mut current_path, &mut current_branch);
        }
    }

    flush_git_entry(&mut entries, &mut current_path, &mut current_branch);

    entries
}

fn flush_git_entry(
    entries: &mut Vec<WorktreeEntry>,
    path: &mut Option<PathBuf>,
    branch: &mut Option<String>,
) {
    if let (Some(p), Some(b)) = (path.take(), branch.take()) {
        let sanitized = sanitize_branch(&b);
        if sanitized.is_empty() {
            log::warn!(
                "Skipping git worktree at '{}' because branch '{}' sanitizes to empty \
                 (no ASCII alphanumeric characters)",
                p.display(),
                b,
            );
            return;
        }
        entries.push(WorktreeEntry {
            path: p,
            branch: b,
            sanitized_branch: sanitized,
            namespace: None,
        });
    }
}

// ─── shared ───────────────────────────────────────────────────────────────────

fn sanitize_branch(branch: &str) -> String {
    let sanitized: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    sanitized.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_branch_simple() {
        assert_eq!(sanitize_branch("feature-a"), "feature-a");
    }

    #[test]
    fn test_sanitize_branch_with_slash() {
        assert_eq!(
            sanitize_branch("feature/my-endpoint"),
            "feature-my-endpoint"
        );
    }

    #[test]
    fn test_sanitize_branch_with_underscore() {
        assert_eq!(sanitize_branch("fix_bug_123"), "fix-bug-123");
    }

    // ─── jj workspace tests ───────────────────────────────────────────────

    #[test]
    fn test_parse_jj_workspace_list_two_workspaces() {
        let input =
            b"default: kkqmkqnm 6aa0ec8e main\nfeature-a: rrqxmqnm 8e9b1c2d feature/my-endpoint\n";
        let entries = parse_jj_workspace_list(input, |name| {
            Some(PathBuf::from(format!("/home/user/{}-ws", name)))
        });
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/default-ws"));
        assert_eq!(entries[0].branch, "default");
        assert_eq!(entries[0].sanitized_branch, "default");
        assert_eq!(entries[1].path, PathBuf::from("/home/user/feature-a-ws"));
        assert_eq!(entries[1].branch, "feature-a");
        assert_eq!(entries[1].sanitized_branch, "feature-a");
    }

    #[test]
    fn test_parse_jj_workspace_list_no_colon() {
        let input = b"some invalid line without colon\n";
        let entries = parse_jj_workspace_list(input, |_| {
            panic!("should not be called for unparseable lines")
        });
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_parse_jj_workspace_list_no_trailing_newline() {
        let input = b"default: kkqmkqnm 6aa0ec8e main";
        let entries = parse_jj_workspace_list(input, |_| Some(PathBuf::from("/home/user/myapp")));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/myapp"));
        assert_eq!(entries[0].branch, "default");
        assert_eq!(entries[0].sanitized_branch, "default");
    }

    #[test]
    fn test_parse_jj_workspace_list_skips_unresolved() {
        let input = b"default: abc123\norphan: def456\n";
        let entries = parse_jj_workspace_list(input, |name| {
            if name == "default" {
                Some(PathBuf::from("/home/user/myapp"))
            } else {
                None
            }
        });
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch, "default");
    }

    // ─── git worktree tests ───────────────────────────────────────────────

    #[test]
    fn test_parse_git_worktree_output_two_worktrees() {
        let input = b"worktree /home/user/myapp\nHEAD abc123\nbranch refs/heads/main\n\nworktree /home/user/myapp-feature-a\nHEAD def456\nbranch refs/heads/feature-a\n";
        let entries = parse_git_worktree_output(input);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/myapp"));
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[0].sanitized_branch, "main");
        assert_eq!(entries[1].path, PathBuf::from("/home/user/myapp-feature-a"));
        assert_eq!(entries[1].branch, "feature-a");
        assert_eq!(entries[1].sanitized_branch, "feature-a");
    }

    #[test]
    fn test_parse_git_worktree_output_detached_head() {
        let input = b"worktree /home/user/myapp\nHEAD abc123\n\n";
        let entries = parse_git_worktree_output(input);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_parse_git_worktree_output_no_trailing_blank() {
        let input = b"worktree /home/user/myapp\nHEAD abc123\nbranch refs/heads/main";
        let entries = parse_git_worktree_output(input);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch, "main");
    }

    #[test]
    fn test_sanitize_branch_non_ascii() {
        assert_eq!(sanitize_branch("fix-バグ"), "fix");
        assert_eq!(sanitize_branch("fix-ü"), "fix");
        assert_eq!(sanitize_branch("fix-中"), "fix");
    }

    #[test]
    fn test_sanitize_branch_empty() {
        assert_eq!(sanitize_branch("---"), "");
        assert_eq!(sanitize_branch("///"), "");
        assert_eq!(sanitize_branch("___"), "");
    }

    #[test]
    fn test_parse_git_worktree_output_empty_sanitized() {
        let input = b"worktree /home/user/myapp\nHEAD abc123\nbranch refs/heads/---\n\n";
        let entries = parse_git_worktree_output(input);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_parse_jj_workspace_list_empty_sanitized() {
        let input = b"---: kkqmkqnm 6aa0ec8e main\n";
        let entries = parse_jj_workspace_list(input, |_| Some(PathBuf::from("/home/user")));
        assert_eq!(entries.len(), 0);
    }
}
