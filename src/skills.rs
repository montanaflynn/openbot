//! Skill loading and formatting utilities.
//!
//! Skills are markdown documents optionally prefixed with lightweight YAML-like
//! frontmatter (`name`, `description`).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A skill loaded from a markdown file.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Display name of the skill.
    pub name: String,
    /// One-line description shown in prompt and CLI output.
    pub description: String,
    /// Markdown body content (instructions and examples).
    pub body: String,
    /// Source file path for provenance/debugging.
    pub source_path: String,
}

/// Load all markdown skills from the given directories.
///
/// Non-markdown files are ignored. Individual invalid skill files are skipped
/// with a warning so one bad file does not block startup.
pub fn load_skills(dirs: &[impl AsRef<Path>]) -> Result<Vec<Skill>> {
    let mut skills = Vec::new();

    for dir in dirs {
        let dir = dir.as_ref();
        if !dir.exists() {
            continue;
        }

        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("reading skill directory {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                match parse_skill_file(&path) {
                    Ok(skill) => skills.push(skill),
                    Err(e) => {
                        tracing::warn!("skipping skill file {}: {e}", path.display());
                    }
                }
            }
        }
    }

    Ok(skills)
}

/// Parse a single markdown skill file.
fn parse_skill_file(path: &Path) -> Result<Skill> {
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let (name, description, body) = parse_frontmatter(&contents, path)?;

    Ok(Skill {
        name,
        description,
        body,
        source_path: path.display().to_string(),
    })
}

/// Parse optional frontmatter from markdown content.
///
/// Expected format:
/// ```text
/// ---
/// name: skill-name
/// description: What this skill does
/// ---
/// Body content here
/// ```
///
/// If frontmatter is missing or malformed, this falls back to filename-based
/// naming and treats the full file as body.
fn parse_frontmatter(content: &str, path: &Path) -> Result<(String, String, String)> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        // No frontmatter: derive skill name from filename.
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());
        return Ok((name, String::new(), content.to_string()));
    }

    // Find the closing delimiter after the opening `---`.
    let after_first = &trimmed[3..];
    let Some(end_idx) = after_first.find("\n---") else {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());
        return Ok((name, String::new(), content.to_string()));
    };

    let frontmatter = &after_first[..end_idx];
    let body_start = 3 + end_idx + 4; // "---" + frontmatter + "\n---"
    let body = trimmed[body_start..].trim_start().to_string();

    // Parse known keys with a minimal line-based parser.
    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(value.trim().to_string());
        }
    }

    let name = name.unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into())
    });

    Ok((name, description.unwrap_or_default(), body))
}

// ---------------------------------------------------------------------------
// Manifest: tracking registry-installed skills
// ---------------------------------------------------------------------------

/// Metadata for a single registry-installed skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    /// Full registry id, e.g. "obra/superpowers/brainstorming".
    pub id: String,
    /// Short skill name, e.g. "brainstorming".
    pub skill_id: String,
    /// Source repository, e.g. "obra/superpowers".
    pub source: String,
    /// When the skill was installed.
    pub installed_at: DateTime<Utc>,
}

/// Manifest tracking all registry-installed skills in a given scope.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillManifest {
    /// Map of short skill id to installation metadata.
    pub skills: BTreeMap<String, InstalledSkill>,
}

/// Load a skill manifest from disk, returning an empty one if the file is missing.
pub fn load_manifest(path: &Path) -> Result<SkillManifest> {
    if !path.exists() {
        return Ok(SkillManifest::default());
    }
    let data =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))
}

/// Write a skill manifest to disk as pretty-printed JSON.
pub fn save_manifest(path: &Path, manifest: &SkillManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(manifest)?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

/// Install a skill: write the markdown file and update the manifest.
pub fn install_skill(
    skill_dir: &Path,
    manifest_path: &Path,
    skill_id: &str,
    source: &str,
    registry_id: &str,
    content: &str,
) -> Result<()> {
    std::fs::create_dir_all(skill_dir)?;

    let md_path = skill_dir.join(format!("{skill_id}.md"));
    std::fs::write(&md_path, content).with_context(|| format!("writing {}", md_path.display()))?;

    let mut manifest = load_manifest(manifest_path)?;
    manifest.skills.insert(
        skill_id.to_string(),
        InstalledSkill {
            id: registry_id.to_string(),
            skill_id: skill_id.to_string(),
            source: source.to_string(),
            installed_at: Utc::now(),
        },
    );
    save_manifest(manifest_path, &manifest)?;

    Ok(())
}

/// Remove a skill: delete the markdown file and its manifest entry.
/// Returns `true` if the skill was found and removed.
pub fn remove_skill(skill_dir: &Path, manifest_path: &Path, skill_id: &str) -> Result<bool> {
    let md_path = skill_dir.join(format!("{skill_id}.md"));

    let file_removed = if md_path.exists() {
        std::fs::remove_file(&md_path)
            .with_context(|| format!("removing {}", md_path.display()))?;
        true
    } else {
        false
    };

    let mut manifest = load_manifest(manifest_path)?;
    let entry_removed = manifest.skills.remove(skill_id).is_some();
    if entry_removed {
        save_manifest(manifest_path, &manifest)?;
    }

    Ok(file_removed || entry_removed)
}

// ---------------------------------------------------------------------------
// Prompt formatting
// ---------------------------------------------------------------------------

/// Format loaded skills into a prompt section.
pub fn format_skills_section(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Available Skills\n\n");
    for skill in skills {
        out.push_str(&format!("### {}\n", skill.name));
        if !skill.description.is_empty() {
            out.push_str(&format!("{}\n", skill.description));
        }
        if !skill.body.is_empty() {
            out.push_str(&format!("\n{}\n", skill.body));
        }
        out.push('\n');
    }
    out
}
