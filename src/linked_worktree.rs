use std::io;
use std::path::{Path, PathBuf};

/// Persisted at spawn so removal can be detected even if a daemon recreates
/// build-cache directories inside its former worktree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkedWorktree {
    git_file: PathBuf,
    git_dir: PathBuf,
}

impl LinkedWorktree {
    pub(crate) async fn discover(dir: &Path) -> io::Result<Option<Self>> {
        let dir = tokio::fs::canonicalize(dir).await?;
        for root in dir.ancestors() {
            let git_file = root.join(".git");
            let metadata = match tokio::fs::metadata(&git_file).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            // Main checkouts and submodules have different lifecycles. Stop at
            // the nearest repository boundary rather than adopting an outer one.
            if !metadata.is_file() {
                return Ok(None);
            }
            let contents = tokio::fs::read_to_string(&git_file).await?;
            let Some(target) = contents.trim().strip_prefix("gitdir: ") else {
                return Ok(None);
            };
            let git_dir = tokio::fs::canonicalize(root.join(target)).await?;
            if tokio::fs::try_exists(git_dir.join("commondir")).await?
                && tokio::fs::try_exists(git_dir.join("gitdir")).await?
            {
                return Ok(Some(Self { git_file, git_dir }));
            }
            return Ok(None);
        }
        Ok(None)
    }

    pub(crate) async fn is_removed(&self) -> io::Result<bool> {
        Ok(!tokio::fs::try_exists(&self.git_file).await?
            || !tokio::fs::try_exists(&self.git_dir).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_removal_despite_recreated_cache_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("checkout");
        let git_dir = temp.path().join("metadata");
        tokio::fs::create_dir_all(root.join("app")).await.unwrap();
        tokio::fs::create_dir_all(&git_dir).await.unwrap();
        tokio::fs::write(root.join(".git"), "gitdir: ../metadata\n")
            .await
            .unwrap();
        tokio::fs::write(git_dir.join("commondir"), "..\n")
            .await
            .unwrap();
        tokio::fs::write(git_dir.join("gitdir"), "../../checkout/.git\n")
            .await
            .unwrap();

        let worktree = LinkedWorktree::discover(&root.join("app"))
            .await
            .unwrap()
            .unwrap();
        assert!(!worktree.is_removed().await.unwrap());
        let saved = toml::to_string(&worktree).unwrap();
        let restored: LinkedWorktree = toml::from_str(&saved).unwrap();

        tokio::fs::remove_dir_all(&root).await.unwrap();
        tokio::fs::create_dir_all(root.join("app/.vite"))
            .await
            .unwrap();
        assert!(restored.is_removed().await.unwrap());

        tokio::fs::write(root.join(".git"), "gitdir: ../metadata\n")
            .await
            .unwrap();
        tokio::fs::remove_dir_all(&git_dir).await.unwrap();
        assert!(restored.is_removed().await.unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_errors_are_not_treated_as_removal() {
        let temp = tempfile::tempdir().unwrap();
        let git_file = temp.path().join(".git");
        std::os::unix::fs::symlink(".git", &git_file).unwrap();
        let worktree = LinkedWorktree {
            git_file,
            git_dir: temp.path().to_path_buf(),
        };
        assert!(worktree.is_removed().await.is_err());
        assert!(LinkedWorktree::discover(temp.path()).await.is_err());
    }

    #[tokio::test]
    async fn ignores_main_checkouts_submodules_and_non_git_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert!(LinkedWorktree::discover(root).await.unwrap().is_none());
        tokio::fs::create_dir(root.join(".git")).await.unwrap();
        assert!(LinkedWorktree::discover(root).await.unwrap().is_none());
        tokio::fs::remove_dir(root.join(".git")).await.unwrap();
        tokio::fs::create_dir(root.join("metadata")).await.unwrap();
        tokio::fs::write(root.join(".git"), "gitdir: metadata\n")
            .await
            .unwrap();
        assert!(LinkedWorktree::discover(root).await.unwrap().is_none());
    }
}
