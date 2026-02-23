//! Workspace helpers: detect project root and derive a slug for
//! per-project memory scoping.

use std::path::{Path, PathBuf};

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
pub fn slug_from_path(path: &Path) -> String {
    let name = path
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
        assert_eq!(slug_from_path(Path::new("/home/user/my-project")), "my-project");
        assert_eq!(slug_from_path(Path::new("/home/user/MyApp")), "myapp");
        assert_eq!(slug_from_path(Path::new("/home/user/backend_api")), "backend-api");
    }
}
