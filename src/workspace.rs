//! Workspace registry: track which projects a bot has been used in
//! and scope memory per-project.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A single registered workspace/project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    /// Short slug used in directory names and CLI flags.
    pub slug: String,
    /// When this workspace was first registered.
    pub first_seen: DateTime<Utc>,
    /// When this workspace was last used.
    pub last_used: DateTime<Utc>,
}

/// Registry of all known workspaces for a bot, keyed by canonical path.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceRegistry {
    pub workspaces: BTreeMap<String, WorkspaceEntry>,
}

impl WorkspaceRegistry {
    /// Load from disk, returning an empty registry if the file is missing.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))
    }

    /// Persist registry to disk as pretty-printed JSON.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).with_context(|| "serializing workspaces")?;
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
    }

    /// Register (or update) a workspace for the given canonical path.
    /// Returns the slug for this workspace.
    ///
    /// If a slug collision occurs with a path that no longer exists on disk,
    /// the stale entry is evicted so the new path inherits the slug (and its
    /// memory directory). This keeps things portable across machines.
    pub fn register(&mut self, canonical_path: &str) -> String {
        let now = Utc::now();
        if let Some(entry) = self.workspaces.get_mut(canonical_path) {
            entry.last_used = now;
            return entry.slug.clone();
        }

        let base_slug = slug_from_path(canonical_path);

        // If the slug is taken by a path that no longer exists, evict it so
        // we inherit the slug (and its memory directory).
        self.evict_stale_for_slug(&base_slug);

        let slug = self.unique_slug(&base_slug);

        self.workspaces.insert(
            canonical_path.to_string(),
            WorkspaceEntry {
                slug: slug.clone(),
                first_seen: now,
                last_used: now,
            },
        );

        slug
    }

    /// Look up a workspace entry by its slug.
    pub fn find_by_slug(&self, slug: &str) -> Option<(&String, &WorkspaceEntry)> {
        self.workspaces
            .iter()
            .find(|(_, entry)| entry.slug == slug)
    }

    /// Remove entries whose path no longer exists on disk if they hold the
    /// given slug. This lets a new path inherit the slug (and its memory)
    /// when the original path is gone (e.g. different machine, moved dir).
    fn evict_stale_for_slug(&mut self, slug: &str) {
        let stale_keys: Vec<String> = self
            .workspaces
            .iter()
            .filter(|(path, entry)| entry.slug == slug && !Path::new(path.as_str()).exists())
            .map(|(path, _)| path.clone())
            .collect();
        for key in stale_keys {
            self.workspaces.remove(&key);
        }
    }

    /// Ensure the slug is unique across existing workspaces.
    /// Appends a short hash suffix on collision.
    fn unique_slug(&self, base: &str) -> String {
        let existing: Vec<&str> = self
            .workspaces
            .values()
            .map(|e| e.slug.as_str())
            .collect();

        if !existing.contains(&base) {
            return base.to_string();
        }

        // Append incrementing suffix until unique.
        for i in 2..=100 {
            let candidate = format!("{base}-{i}");
            if !existing.contains(&candidate.as_str()) {
                return candidate;
            }
        }

        // Extremely unlikely fallback: use timestamp.
        format!("{base}-{}", Utc::now().timestamp())
    }
}

/// Detect the project root for a working directory.
///
/// Uses `git rev-parse --show-toplevel` so that worktrees of the same repo
/// resolve to the main repo root. Falls back to the provided directory itself.
pub fn detect_project_root(cwd: &Path) -> PathBuf {
    crate::git::resolve_repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

/// Derive a URL/filesystem-safe slug from a path.
///
/// Takes the last component (directory name) and lowercases it, replacing
/// non-alphanumeric characters with hyphens.
fn slug_from_path(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());

    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();

    // Trim leading/trailing hyphens and collapse runs.
    let slug = slug.trim_matches('-').to_string();
    let mut prev_hyphen = false;
    slug.chars()
        .filter(|&c| {
            if c == '-' {
                if prev_hyphen {
                    return false;
                }
                prev_hyphen = true;
            } else {
                prev_hyphen = false;
            }
            true
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_from_typical_path() {
        assert_eq!(slug_from_path("/home/user/my-project"), "my-project");
        assert_eq!(slug_from_path("/home/user/MyApp"), "myapp");
        assert_eq!(slug_from_path("/home/user/backend_api"), "backend-api");
    }

    #[test]
    fn register_returns_same_slug() {
        let mut reg = WorkspaceRegistry::default();
        let slug1 = reg.register("/home/user/project-a");
        let slug2 = reg.register("/home/user/project-a");
        assert_eq!(slug1, slug2);
    }

    #[test]
    fn register_handles_collision() {
        // Both paths are nonexistent so the first gets evicted — but to test
        // a genuine collision we need both paths to "exist". We can't easily
        // fake that, so we test unique_slug directly.
        let mut reg = WorkspaceRegistry::default();
        // Manually insert so eviction doesn't remove it (eviction only runs
        // during register, and unique_slug is called after eviction).
        let now = chrono::Utc::now();
        reg.workspaces.insert(
            "/tmp/this-path-exists-for-test/myapp".into(),
            WorkspaceEntry {
                slug: "myapp".into(),
                first_seen: now,
                last_used: now,
            },
        );
        let slug2 = reg.unique_slug("myapp");
        assert_eq!(slug2, "myapp-2");
    }

    #[test]
    fn register_evicts_stale_path() {
        let mut reg = WorkspaceRegistry::default();
        // Register a path that doesn't exist on disk.
        let slug1 = reg.register("/nonexistent/old-machine/myapp");
        assert_eq!(slug1, "myapp");

        // Register a new (also nonexistent) path with the same dir name.
        // The old entry should be evicted so the new one gets the same slug.
        let slug2 = reg.register("/different/path/myapp");
        assert_eq!(slug2, "myapp");
        assert_eq!(reg.workspaces.len(), 1);
        assert!(reg.workspaces.contains_key("/different/path/myapp"));
    }
}
