use anyhow::{Context, Result};
use std::path::Path;

/// A skill loaded from a markdown file with YAML frontmatter.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub source_path: String,
}

/// Load all skills from the given directories.
/// Skills are `.md` files with optional YAML frontmatter delimited by `---`.
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

/// Parse a single skill markdown file.
fn parse_skill_file(path: &Path) -> Result<Skill> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;

    let (name, description, body) = parse_frontmatter(&contents, path)?;

    Ok(Skill {
        name,
        description,
        body,
        source_path: path.display().to_string(),
    })
}

/// Parse YAML frontmatter from markdown content.
/// Expected format:
/// ```
/// ---
/// name: skill-name
/// description: What this skill does
/// ---
/// Body content here
/// ```
fn parse_frontmatter(content: &str, path: &Path) -> Result<(String, String, String)> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        // No frontmatter - use filename as name
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());
        return Ok((name, String::new(), content.to_string()));
    }

    // Find the closing `---`
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

    // Simple YAML-like parsing for name and description
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

/// Format skills into a prompt section.
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
