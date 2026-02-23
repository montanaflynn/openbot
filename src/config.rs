use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenBotConfig {
    /// Base prompt/instructions for the agent.
    pub instructions: String,
    /// Maximum iterations (0 = unlimited).
    pub max_iterations: u32,
    /// Seconds between iterations (0 = wait for input only).
    pub sleep_secs: u64,
    /// Phrase that ends the loop when the agent says it.
    pub stop_phrase: Option<String>,
    /// Model override.
    pub model: Option<String>,
    /// Sandbox mode: "read-only", "workspace-write", or "danger-full-access".
    pub sandbox: String,
    /// Where to store persistent memory.
    pub memory_path: PathBuf,
    /// Directories to scan for skill markdown files.
    pub skill_dirs: Vec<PathBuf>,
    /// Allow running outside git repos.
    pub skip_git_check: bool,
}

impl Default for OpenBotConfig {
    fn default() -> Self {
        Self {
            instructions: "You are an autonomous AI agent. Complete tasks thoroughly and report your progress.".into(),
            max_iterations: 10,
            sleep_secs: 30,
            stop_phrase: Some("TASK COMPLETE".into()),
            model: None,
            sandbox: "workspace-write".into(),
            memory_path: PathBuf::from(".openbot/memory.json"),
            skill_dirs: vec![PathBuf::from("skills")],
            skip_git_check: false,
        }
    }
}

impl OpenBotConfig {
    /// Load config from `openbot.toml` in the given directory, falling back to defaults.
    pub fn load(dir: &Path) -> Result<Self> {
        let config_path = dir.join("openbot.toml");
        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)
                .with_context(|| format!("reading {}", config_path.display()))?;
            let config: OpenBotConfig =
                toml::from_str(&contents).with_context(|| "parsing openbot.toml")?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Apply CLI overrides on top of the loaded config.
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

    /// Resolve the sandbox mode string to a codex SandboxMode.
    pub fn sandbox_mode(&self) -> codex_protocol::config_types::SandboxMode {
        match self.sandbox.as_str() {
            "read-only" => codex_protocol::config_types::SandboxMode::ReadOnly,
            "danger-full-access" => codex_protocol::config_types::SandboxMode::DangerFullAccess,
            _ => codex_protocol::config_types::SandboxMode::WorkspaceWrite,
        }
    }

    /// Expand `~` in skill_dirs to the user's home directory.
    pub fn resolved_skill_dirs(&self) -> Vec<PathBuf> {
        self.skill_dirs
            .iter()
            .map(|p| {
                let s = p.to_string_lossy();
                if s.starts_with("~/") {
                    if let Some(home) = dirs_path() {
                        home.join(&s[2..])
                    } else {
                        p.clone()
                    }
                } else {
                    p.clone()
                }
            })
            .collect()
    }
}

fn dirs_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
