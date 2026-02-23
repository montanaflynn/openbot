use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Information about a created worktree.
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub base_branch: String,
}

/// Create a git worktree for an isolated bot run.
///
/// The worktree is placed under `<repo>/.git/openbot-worktrees/<bot>-<ts>/`
/// on a new branch `openbot/<bot>-<ts>`.
pub fn create_worktree(repo_root: &Path, bot_name: &str) -> Result<WorktreeInfo> {
    let base_branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_root)
        .output()
        .with_context(|| "running git rev-parse")?;
    let base_branch = String::from_utf8_lossy(&base_branch.stdout)
        .trim()
        .to_string();

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let suffix = format!("{bot_name}-{ts}");
    let branch = format!("openbot/{suffix}");
    let wt_path = repo_root.join(".git/openbot-worktrees").join(&suffix);

    let output = std::process::Command::new("git")
        .args(["worktree", "add", &wt_path.to_string_lossy(), "-b", &branch])
        .current_dir(repo_root)
        .output()
        .with_context(|| "running git worktree add")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed: {stderr}");
    }

    Ok(WorktreeInfo {
        path: wt_path,
        branch,
        base_branch,
    })
}

/// Remove a previously created worktree directory.
///
/// The branch is intentionally kept so uncommitted work isn't lost.
pub fn remove_worktree(path: &Path) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", &path.to_string_lossy()])
        .output()
        .with_context(|| "running git worktree remove")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree remove failed: {stderr}");
    }
    Ok(())
}

/// Resolve the root git project for a directory, handling worktrees correctly.
///
/// Uses `git rev-parse --git-common-dir` so that worktrees of the same repo
/// resolve to the same root. Returns `None` if not inside a git repository.
pub fn resolve_repo_root(cwd: &Path) -> Option<PathBuf> {
    let base = if cwd.is_dir() { cwd } else { cwd.parent()? };

    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(base)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    Some(PathBuf::from(root))
}

/// Drop guard that removes a worktree on exit (normal, error, or panic).
pub struct WorktreeGuard {
    path: PathBuf,
}

impl WorktreeGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        remove_worktree(&self.path).ok();
    }
}
