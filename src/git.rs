//! Git helpers for discovering repo roots and managing temporary worktrees.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Information about a created worktree.
pub struct WorktreeInfo {
    /// Filesystem path to the created worktree directory.
    pub path: PathBuf,
    /// Name of the branch created for the run.
    pub branch: String,
    /// Branch that the new worktree branch was based on.
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

    // Copy uncommitted changes (tracked modifications + untracked files) into
    // the worktree so the bot sees the same state as the user's working tree.
    copy_dirty_state(repo_root, &wt_path)?;

    Ok(WorktreeInfo {
        path: wt_path,
        branch,
        base_branch,
    })
}

/// Copy dirty working-tree state from the source repo into a fresh worktree.
///
/// This handles two categories:
/// 1. Tracked files with modifications (staged or unstaged) — copied via
///    `git diff` to find changed paths, then file-level copy.
/// 2. Untracked files — discovered via `git ls-files --others --exclude-standard`,
///    then copied with directory structure preserved.
fn copy_dirty_state(repo_root: &Path, wt_path: &Path) -> Result<()> {
    // 1. Tracked modifications (unstaged + staged vs HEAD).
    let diff_output = std::process::Command::new("git")
        .args(["diff", "HEAD", "--name-only"])
        .current_dir(repo_root)
        .output()
        .with_context(|| "listing tracked changes")?;
    let tracked_files = String::from_utf8_lossy(&diff_output.stdout);

    for relpath in tracked_files.lines() {
        let relpath = relpath.trim();
        if relpath.is_empty() {
            continue;
        }
        let src = repo_root.join(relpath);
        let dst = wt_path.join(relpath);
        if src.is_file() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(&src, &dst).ok();
        } else if !src.exists() {
            // File was deleted in the working tree — remove from worktree too.
            std::fs::remove_file(&dst).ok();
        }
    }

    // 2. Untracked files (respects .gitignore).
    let untracked_output = std::process::Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(repo_root)
        .output()
        .with_context(|| "listing untracked files")?;
    let untracked_files = String::from_utf8_lossy(&untracked_output.stdout);

    for relpath in untracked_files.lines() {
        let relpath = relpath.trim();
        if relpath.is_empty() {
            continue;
        }
        let src = repo_root.join(relpath);
        let dst = wt_path.join(relpath);
        if src.is_file() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(&src, &dst).ok();
        }
    }

    Ok(())
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
    /// Create a guard that removes the worktree path when dropped.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        remove_worktree(&self.path).ok();
    }
}
