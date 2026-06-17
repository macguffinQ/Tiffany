//! Git worktree pool: allocate a worktree per task, release on completion.

use anyhow::{Context, Result};
use git2::Repository;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

pub struct WorktreePool {
    base: PathBuf,
}

impl WorktreePool {
    pub fn new(base: &Path) -> Self {
        Self {
            base: base.to_path_buf(),
        }
    }

    /// Allocate a worktree for a task. Returns the worktree path.
    /// If `repo_root` is None, allocates an isolated directory (not a real worktree).
    pub fn acquire(&self, task_id: Uuid, repo_root: Option<&Path>) -> Result<PathBuf> {
        let path = self.base.join(task_id.to_string());
        // Only create the parent dir (`worktree_base`) — let `git worktree add`
        // create the leaf dir itself. Pre-creating the leaf makes git refuse
        // with "already exists".
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating worktree base {}", parent.display()))?;
        }

        if let Some(repo) = repo_root {
            // `git worktree add` the path. Capture stderr so failures tell
            // us WHY git refused (dir exists, lock contention, etc.).
            let output = Command::new("git")
                .args(["worktree", "add"])
                .arg(&path)
                .arg("HEAD")
                .current_dir(repo)
                .output()
                .with_context(|| "running git worktree add")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // Clean up the empty dir we just created so the next attempt
                // can start fresh.
                let _ = std::fs::remove_dir(&path);
                // Single-line error message; terminal chat stage_detail is a single
                // line, and newlines in there would overflow the layout.
                let stderr_oneline = stderr.trim().replace('\n', " | ");
                anyhow::bail!(
                    "git worktree add failed (exit {:?}): {}",
                    output.status.code(),
                    stderr_oneline
                );
            }
        }
        Ok(path)
    }

    /// Release (remove) a worktree. Best-effort: removes the dir even if
    /// `git worktree remove` fails.
    pub fn release(&self, task_id: Uuid, repo_root: Option<&Path>) -> Result<()> {
        let path = self.base.join(task_id.to_string());
        if let Some(repo) = repo_root {
            let _ = Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(&path)
                .current_dir(repo)
                .status();
        }
        if path.exists() {
            std::fs::remove_dir_all(&path).ok();
        }
        Ok(())
    }

    /// Compute a unified diff of the worktree against its starting commit.
    pub fn diff(&self, task_id: Uuid) -> Result<String> {
        let path = self.base.join(task_id.to_string());
        let output = Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(&path)
            .output()
            .with_context(|| "running git diff")?;
        if !output.status.success() {
            // Not a git repo — return empty.
            return Ok(String::new());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run a no-op operation in the worktree to confirm it exists.
    pub fn exists(&self, task_id: Uuid) -> bool {
        self.base.join(task_id.to_string()).exists()
    }

    /// Detect the current working directory's repo root (if any).
    pub fn detect_repo_root() -> Option<PathBuf> {
        Self::detect_repo_root_from(Path::new("."))
    }

    /// Detect a repo root starting from a specific path.
    pub fn detect_repo_root_from(start: &Path) -> Option<PathBuf> {
        Repository::discover(start)
            .ok()
            .and_then(|r| r.workdir().map(|p| p.to_path_buf()))
    }
}
