//! Configuration loading, defaults, and path helpers for bots and global data.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// The openbot home directory (`~/.openbot`).
pub fn openbot_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("$HOME not set"))?;
    Ok(PathBuf::from(home).join(".openbot"))
}

/// Return the path to a bot's directory (`~/.openbot/bots/<name>`).
pub fn bot_dir(name: &str) -> Result<PathBuf> {
    Ok(openbot_home()?.join("bots").join(name))
}

/// Global skills directory (`~/.openbot/skills`).
pub fn global_skills_dir() -> Result<PathBuf> {
    Ok(openbot_home()?.join("skills"))
}

/// Bot-local skills directory (`~/.openbot/bots/<name>/skills`).
pub fn bot_skills_dir(name: &str) -> Result<PathBuf> {
    Ok(bot_dir(name)?.join("skills"))
}

/// Bot memory path (`~/.openbot/bots/<name>/memory.json`).
pub fn bot_memory_path(name: &str) -> Result<PathBuf> {
    Ok(bot_dir(name)?.join("memory.json"))
}

/// Per-project memory path (`~/.openbot/bots/<name>/workspaces/<slug>/memory.json`).
pub fn bot_workspace_memory_path(name: &str, slug: &str) -> Result<PathBuf> {
    Ok(bot_dir(name)?
        .join("workspaces")
        .join(slug)
        .join("memory.json"))
}

/// Bot config path (`~/.openbot/bots/<name>/config.md`).
pub fn bot_config_path(name: &str) -> Result<PathBuf> {
    Ok(bot_dir(name)?.join("config.md"))
}

/// Global skills manifest (`~/.openbot/skills/manifest.json`).
pub fn global_skills_manifest_path() -> Result<PathBuf> {
    Ok(global_skills_dir()?.join("manifest.json"))
}

/// Bot-local skills manifest (`~/.openbot/bots/<name>/skills/manifest.json`).
pub fn bot_skills_manifest_path(name: &str) -> Result<PathBuf> {
    Ok(bot_skills_dir(name)?.join("manifest.json"))
}

/// Ensure the bot directory structure exists.
pub fn ensure_bot_dirs(name: &str) -> Result<()> {
    std::fs::create_dir_all(bot_dir(name)?)?;
    std::fs::create_dir_all(bot_skills_dir(name)?)?;
    Ok(())
}

/// Ensure the global openbot directories exist.
pub fn ensure_global_dirs() -> Result<()> {
    std::fs::create_dir_all(global_skills_dir()?)?;
    Ok(())
}

/// List all bot names by scanning `~/.openbot/bots/`.
pub fn list_bots() -> Result<Vec<String>> {
    let bots_dir = openbot_home()?.join("bots");
    if !bots_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(bots_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// TOML frontmatter fields from config.md.
/// Instructions come from the markdown body, not from frontmatter.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Frontmatter {
    description: Option<String>,
    max_iterations: Option<u32>,
    sleep_secs: Option<u64>,
    stop_phrase: Option<String>,
    model: Option<String>,
    sandbox: Option<String>,
    skip_git_check: Option<bool>,
}

/// Runtime configuration for a bot run.
/// Loaded from the bot's `config.md` (TOML frontmatter + markdown body).
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Short description of the bot.
    pub description: String,
    /// Base prompt/instructions for the agent (markdown body).
    pub instructions: String,
    /// Maximum iterations (`0` means unlimited).
    pub max_iterations: u32,
    /// Seconds between iterations (`0` means no sleep).
    pub sleep_secs: u64,
    /// Phrase that ends the loop.
    pub stop_phrase: Option<String>,
    /// Model override.
    pub model: Option<String>,
    /// Sandbox mode: "read-only", "workspace-write", or "danger-full-access".
    pub sandbox: String,
    /// If true, skip the git repository requirement.
    pub skip_git_check: bool,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            description: String::new(),
            instructions: "You are an autonomous AI agent. Complete tasks thoroughly and report your progress.".into(),
            max_iterations: 10,
            sleep_secs: 30,
            stop_phrase: Some("TASK COMPLETE".into()),
            model: None,
            sandbox: "workspace-write".into(),
            skip_git_check: false,
        }
    }
}

/// Parse a config.md file into (frontmatter, body).
/// Frontmatter is delimited by `+++` lines.
fn parse_config_md(contents: &str) -> Result<(Frontmatter, String)> {
    let trimmed = contents.trim_start();
    if !trimmed.starts_with("+++") {
        // No frontmatter -- entire file is instructions.
        return Ok((Frontmatter::default(), contents.trim().to_string()));
    }

    // Find the closing +++.
    let after_open = &trimmed[3..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close = after_open
        .find("\n+++")
        .ok_or_else(|| anyhow::anyhow!("config.md: missing closing +++"))?;

    let frontmatter_str = &after_open[..close];
    let body_start = close + 4; // skip \n+++
    let body = if body_start < after_open.len() {
        after_open[body_start..].trim().to_string()
    } else {
        String::new()
    };

    let frontmatter: Frontmatter =
        toml::from_str(frontmatter_str).with_context(|| "parsing config.md frontmatter")?;

    Ok((frontmatter, body))
}

/// Serialize a BotConfig back to config.md format.
pub fn serialize_config_md(config: &BotConfig) -> String {
    let mut fm = String::from("+++\n");

    if !config.description.is_empty() {
        fm.push_str(&format!("description = {:?}\n", config.description));
    }

    let defaults = BotConfig::default();

    if config.max_iterations != defaults.max_iterations {
        fm.push_str(&format!("max_iterations = {}\n", config.max_iterations));
    }
    if config.sleep_secs != defaults.sleep_secs {
        fm.push_str(&format!("sleep_secs = {}\n", config.sleep_secs));
    }
    if config.stop_phrase != defaults.stop_phrase {
        if let Some(ref phrase) = config.stop_phrase {
            fm.push_str(&format!("stop_phrase = {:?}\n", phrase));
        }
    }
    if let Some(ref model) = config.model {
        fm.push_str(&format!("model = {:?}\n", model));
    }
    if config.sandbox != defaults.sandbox {
        fm.push_str(&format!("sandbox = {:?}\n", config.sandbox));
    }
    if config.skip_git_check {
        fm.push_str("skip_git_check = true\n");
    }

    fm.push_str("\n+++\n\n");
    fm.push_str(&config.instructions);
    fm.push('\n');
    fm
}

impl BotConfig {
    /// Load config for a bot. Falls back to defaults if no config.md exists.
    pub fn load(bot_name: &str) -> Result<Self> {
        let config_path = bot_config_path(bot_name)?;
        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)
                .with_context(|| format!("reading {}", config_path.display()))?;
            let (fm, body) = parse_config_md(&contents)?;

            let defaults = Self::default();
            Ok(Self {
                description: fm.description.unwrap_or_default(),
                instructions: if body.is_empty() {
                    defaults.instructions
                } else {
                    body
                },
                max_iterations: fm.max_iterations.unwrap_or(defaults.max_iterations),
                sleep_secs: fm.sleep_secs.unwrap_or(defaults.sleep_secs),
                stop_phrase: fm.stop_phrase.or(defaults.stop_phrase),
                model: fm.model,
                sandbox: fm.sandbox.unwrap_or(defaults.sandbox),
                skip_git_check: fm.skip_git_check.unwrap_or(defaults.skip_git_check),
            })
        } else {
            Ok(Self::default())
        }
    }

    /// Apply CLI overrides.
    pub fn with_overrides(
        mut self,
        prompt: Option<String>,
        max_iterations: Option<u32>,
        model: Option<String>,
        skip_git_check: bool,
        sleep_secs: Option<u64>,
    ) -> Self {
        if let Some(prompt) = prompt {
            self.instructions = prompt;
        }
        if let Some(n) = max_iterations {
            self.max_iterations = n;
        }
        if model.is_some() {
            self.model = model;
        }
        if skip_git_check {
            self.skip_git_check = true;
        }
        if let Some(s) = sleep_secs {
            self.sleep_secs = s;
        }
        self
    }

    /// Convert sandbox string to codex SandboxMode.
    pub fn sandbox_mode(&self) -> codex_protocol::config_types::SandboxMode {
        match self.sandbox.as_str() {
            "read-only" => codex_protocol::config_types::SandboxMode::ReadOnly,
            "danger-full-access" => codex_protocol::config_types::SandboxMode::DangerFullAccess,
            _ => codex_protocol::config_types::SandboxMode::WorkspaceWrite,
        }
    }

    /// Return skill directories for this bot: global + bot-local.
    pub fn skill_dirs(bot_name: &str) -> Result<Vec<PathBuf>> {
        Ok(vec![global_skills_dir()?, bot_skills_dir(bot_name)?])
    }

    /// Return the memory path for this bot.
    pub fn memory_path(bot_name: &str) -> Result<PathBuf> {
        bot_memory_path(bot_name)
    }
}
