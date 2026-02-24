//! Skill loading and formatting utilities.
//!
//! Skills are markdown documents optionally prefixed with lightweight YAML-like
//! frontmatter (`name`, `description`).

use anyhow::{Context, Result};
use chrono::Utc;
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
    /// Registry source repo (e.g. "obra/superpowers"), if installed from registry.
    pub source: Option<String>,
    /// When the skill was installed from the registry.
    pub installed_at: Option<String>,
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

    let fm = parse_frontmatter(&contents, path)?;

    Ok(Skill {
        name: fm.name,
        description: fm.description,
        body: fm.body,
        source_path: path.display().to_string(),
        source: fm.source,
        installed_at: fm.installed_at,
    })
}

/// Parsed frontmatter fields from a skill markdown file.
struct SkillFrontmatter {
    name: String,
    description: String,
    body: String,
    source: Option<String>,
    installed_at: Option<String>,
}

/// Parse optional frontmatter from markdown content.
///
/// Expected format:
/// ```text
/// ---
/// name: skill-name
/// description: What this skill does
/// source: obra/superpowers
/// installed_at: 2026-02-24T05:00:00Z
/// ---
/// Body content here
/// ```
///
/// If frontmatter is missing or malformed, this falls back to filename-based
/// naming and treats the full file as body.
fn parse_frontmatter(content: &str, path: &Path) -> Result<SkillFrontmatter> {
    let fallback_name = || {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into())
    };

    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Ok(SkillFrontmatter {
            name: fallback_name(),
            description: String::new(),
            body: content.to_string(),
            source: None,
            installed_at: None,
        });
    }

    // Find the closing delimiter after the opening `---`.
    let after_first = &trimmed[3..];
    let Some(end_idx) = after_first.find("\n---") else {
        return Ok(SkillFrontmatter {
            name: fallback_name(),
            description: String::new(),
            body: content.to_string(),
            source: None,
            installed_at: None,
        });
    };

    let frontmatter = &after_first[..end_idx];
    let body_start = 3 + end_idx + 4; // "---" + frontmatter + "\n---"
    let body = trimmed[body_start..].trim_start().to_string();

    // Parse known keys with a minimal line-based parser.
    let mut name = None;
    let mut description = None;
    let mut source = None;
    let mut installed_at = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("source:") {
            source = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("installed_at:") {
            installed_at = Some(value.trim().to_string());
        }
    }

    Ok(SkillFrontmatter {
        name: name.unwrap_or_else(fallback_name),
        description: description.unwrap_or_default(),
        body,
        source,
        installed_at,
    })
}

// ---------------------------------------------------------------------------
// Install / remove skills
// ---------------------------------------------------------------------------

/// Install a skill: write the markdown file with registry metadata in frontmatter.
///
/// If the fetched content already has frontmatter, `source` and `installed_at`
/// fields are injected into it. Otherwise a new frontmatter block is prepended.
pub fn install_skill(
    skill_dir: &Path,
    skill_id: &str,
    source: &str,
    content: &str,
) -> Result<()> {
    std::fs::create_dir_all(skill_dir)?;

    let now = Utc::now().to_rfc3339();
    let enriched = inject_frontmatter_fields(content, source, &now);

    let md_path = skill_dir.join(format!("{skill_id}.md"));
    std::fs::write(&md_path, enriched)
        .with_context(|| format!("writing {}", md_path.display()))?;

    Ok(())
}

/// Inject `source` and `installed_at` into existing frontmatter, or prepend new frontmatter.
fn inject_frontmatter_fields(content: &str, source: &str, installed_at: &str) -> String {
    let trimmed = content.trim_start();
    if trimmed.starts_with("---") {
        let after_first = &trimmed[3..];
        if let Some(end_idx) = after_first.find("\n---") {
            // Insert before the closing ---
            let fm = &after_first[..end_idx];
            let rest = &trimmed[3 + end_idx..];
            return format!(
                "---{fm}\nsource: {source}\ninstalled_at: {installed_at}{rest}"
            );
        }
    }
    // No valid frontmatter — prepend one.
    format!(
        "---\nsource: {source}\ninstalled_at: {installed_at}\n---\n{content}"
    )
}

/// Remove a skill by deleting its markdown file.
/// Returns `true` if the skill was found and removed.
pub fn remove_skill(skill_dir: &Path, skill_id: &str) -> Result<bool> {
    let md_path = skill_dir.join(format!("{skill_id}.md"));

    if md_path.exists() {
        std::fs::remove_file(&md_path)
            .with_context(|| format!("removing {}", md_path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
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
