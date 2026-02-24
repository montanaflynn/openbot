//! Persistent key-value memory for agents.
//!
//! This is the agent's own memory — a simple key-value store that persists
//! across sessions. Agents can use it to remember decisions, preferences,
//! patterns, or anything else useful between runs.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Persistent key-value memory stored as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Memory {
    pub entries: BTreeMap<String, String>,
}

/// Handle for loading, mutating, and saving memory to disk.
pub struct MemoryStore {
    path: PathBuf,
    pub memory: Memory,
}

impl MemoryStore {
    /// Load memory from `path`, or return an empty store when absent.
    pub fn load(path: &Path) -> Result<Self> {
        let memory = if path.exists() {
            let contents = std::fs::read_to_string(path).with_context(|| "reading memory file")?;
            serde_json::from_str(&contents).with_context(|| "parsing memory JSON")?
        } else {
            Memory::default()
        };
        Ok(Self {
            path: path.to_path_buf(),
            memory,
        })
    }

    /// Persist current memory state to disk.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        let json =
            serde_json::to_string_pretty(&self.memory).with_context(|| "serializing memory")?;
        std::fs::write(&self.path, json).with_context(|| "writing memory file")?;
        Ok(())
    }

    /// Set or replace a key-value memory entry.
    pub fn set(&mut self, key: String, value: String) {
        self.memory.entries.insert(key, value);
    }

    /// Remove a memory entry by key.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.memory.entries.remove(key)
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.memory.entries.clear();
    }

    /// Render a human-readable dump for CLI output.
    pub fn display(&self) -> String {
        if self.memory.entries.is_empty() {
            return "No memory entries.\n".to_string();
        }
        let mut out = String::new();
        for (k, v) in &self.memory.entries {
            out.push_str(&format!("  {k} = {v}\n"));
        }
        out
    }
}
