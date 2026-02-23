use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Persistent memory stored as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Memory {
    /// Arbitrary key-value store the agent can use.
    pub entries: BTreeMap<String, String>,
    /// Record of each iteration's outcome.
    pub history: Vec<IterationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationRecord {
    pub iteration: u32,
    pub timestamp: DateTime<Utc>,
    pub prompt_summary: String,
    pub response_summary: String,
}

/// Handle for loading/saving memory to disk.
pub struct MemoryStore {
    path: PathBuf,
    pub memory: Memory,
}

impl MemoryStore {
    /// Load memory from the given path, or create empty if it doesn't exist.
    pub fn load(path: &Path) -> Result<Self> {
        let memory = if path.exists() {
            let contents =
                std::fs::read_to_string(path).with_context(|| "reading memory file")?;
            serde_json::from_str(&contents).with_context(|| "parsing memory JSON")?
        } else {
            Memory::default()
        };
        Ok(Self {
            path: path.to_path_buf(),
            memory,
        })
    }

    /// Persist memory to disk.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&self.memory)
            .with_context(|| "serializing memory")?;
        std::fs::write(&self.path, json).with_context(|| "writing memory file")?;
        Ok(())
    }

    /// Record the result of an iteration.
    pub fn add_iteration(
        &mut self,
        iteration: u32,
        prompt_summary: &str,
        response_summary: &str,
    ) {
        self.memory.history.push(IterationRecord {
            iteration,
            timestamp: Utc::now(),
            prompt_summary: prompt_summary.to_string(),
            response_summary: response_summary.to_string(),
        });
    }

    /// Set a key-value entry.
    pub fn set(&mut self, key: String, value: String) {
        self.memory.entries.insert(key, value);
    }

    /// Remove a key.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.memory.entries.remove(key)
    }

    /// Clear all entries and history.
    pub fn clear(&mut self) {
        self.memory.entries.clear();
        self.memory.history.clear();
    }

    /// Format memory for display.
    pub fn display(&self) -> String {
        let mut out = String::new();

        if self.memory.entries.is_empty() {
            out.push_str("No memory entries.\n");
        } else {
            out.push_str("## Entries\n");
            for (k, v) in &self.memory.entries {
                out.push_str(&format!("  {k} = {v}\n"));
            }
        }

        if self.memory.history.is_empty() {
            out.push_str("No iteration history.\n");
        } else {
            out.push_str(&format!("\n## History ({} iterations)\n", self.memory.history.len()));
            for record in &self.memory.history {
                out.push_str(&format!(
                    "  [{}] iteration {}: {}\n",
                    record.timestamp.format("%Y-%m-%d %H:%M:%S"),
                    record.iteration,
                    truncate(&record.response_summary, 100),
                ));
            }
        }

        out
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
