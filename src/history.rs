//! Session history stored as individual JSON files.
//!
//! Each completed session is saved as `history/{session_id}.json` inside the
//! bot's workspace directory. This keeps history browsable, git-friendly, and
//! independent of the agent's key-value memory.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Token usage snapshot captured at the end of a session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenSnapshot {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub context_window: Option<i64>,
}

/// A single completed session record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Codex session ID.
    pub session_id: String,
    /// Global 1-based session number.
    pub session_number: usize,
    /// UTC timestamp when the session started.
    pub started_at: DateTime<Utc>,
    /// Wall-clock duration in seconds.
    pub duration_secs: u64,
    /// Model used for the session.
    pub model: String,
    /// Short summary of the prompt/instructions.
    pub prompt_summary: String,
    /// Short summary of what the agent accomplished.
    pub response_summary: String,
    /// What happened to the worktree changes.
    pub action: Option<String>,
    /// Token usage at end of session.
    pub tokens: Option<TokenSnapshot>,
}

/// Save a session record to `history_dir/{session_id}.json`.
pub fn save(history_dir: &Path, record: &SessionRecord) -> Result<()> {
    std::fs::create_dir_all(history_dir)
        .with_context(|| format!("creating history dir {}", history_dir.display()))?;
    let path = history_dir.join(format!("{}.json", record.session_id));
    let json = serde_json::to_string_pretty(record).with_context(|| "serializing session")?;
    std::fs::write(&path, json).with_context(|| "writing session file")?;
    Ok(())
}

/// Load a single session record by ID.
pub fn load(history_dir: &Path, session_id: &str) -> Result<SessionRecord> {
    let path = history_dir.join(format!("{session_id}.json"));
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&contents).with_context(|| "parsing session JSON")
}

/// List all session records, sorted by session number.
pub fn list(history_dir: &Path) -> Result<Vec<SessionRecord>> {
    if !history_dir.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in std::fs::read_dir(history_dir).with_context(|| "reading history dir")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            if let Ok(record) = serde_json::from_str::<SessionRecord>(&contents) {
                records.push(record);
            }
        }
    }
    records.sort_by_key(|r| r.session_number);
    Ok(records)
}

/// Count session records without loading them all.
pub fn count(history_dir: &Path) -> usize {
    if !history_dir.exists() {
        return 0;
    }
    std::fs::read_dir(history_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0)
}

/// Load the N most recent session records.
pub fn recent(history_dir: &Path, n: usize) -> Result<Vec<SessionRecord>> {
    let all = list(history_dir)?;
    let start = all.len().saturating_sub(n);
    Ok(all[start..].to_vec())
}
