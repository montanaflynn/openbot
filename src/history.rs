//! Session history stored as directory-per-session with metadata + event stream.
//!
//! Each completed session is saved as `history/{session_id}/` inside the
//! bot's workspace directory, containing:
//! - `metadata.json` — session-level summary
//! - `events.jsonl`  — append-only event stream
//!
//! Legacy `history/{session_id}.json` files are still readable for backward
//! compatibility.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};

/// A command executed during a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEntry {
    pub command: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

/// Token usage snapshot captured at the end of a session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenSnapshot {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub context_window: Option<i64>,
}

/// A single completed session record (metadata only).
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
    /// Number of commands executed (for quick display without reading events).
    #[serde(default)]
    pub command_count: Option<usize>,
}

/// An event captured during a session, streamed to `events.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    Message {
        content: String,
    },
    Command {
        command: String,
        exit_code: i32,
        duration_ms: u64,
    },
    TokenCount {
        input_tokens: i64,
        cached_input_tokens: i64,
        output_tokens: i64,
        reasoning_output_tokens: i64,
        context_window: Option<i64>,
    },
}

/// Streams session events to disk as they happen.
pub struct SessionWriter {
    session_dir: PathBuf,
    writer: BufWriter<File>,
}

impl SessionWriter {
    /// Create a new session directory, write initial metadata, and open the events file.
    pub fn create(history_dir: &Path, record: &SessionRecord) -> Result<Self> {
        let session_dir = history_dir.join(&record.session_id);
        fs::create_dir_all(&session_dir)
            .with_context(|| format!("creating session dir {}", session_dir.display()))?;

        // Write initial metadata.
        let meta_path = session_dir.join("metadata.json");
        let json =
            serde_json::to_string_pretty(record).with_context(|| "serializing initial metadata")?;
        fs::write(&meta_path, json).with_context(|| "writing initial metadata")?;

        // Open events file for appending.
        let events_path = session_dir.join("events.jsonl");
        let file = File::create(&events_path)
            .with_context(|| format!("creating {}", events_path.display()))?;
        let writer = BufWriter::new(file);

        Ok(Self {
            session_dir,
            writer,
        })
    }

    /// Append a single event to the events.jsonl file.
    pub fn append_event(&mut self, event: &SessionEvent) -> Result<()> {
        let line = serde_json::to_string(event).with_context(|| "serializing event")?;
        writeln!(self.writer, "{line}").with_context(|| "writing event")?;
        self.writer.flush().with_context(|| "flushing events")?;
        Ok(())
    }

    /// Overwrite metadata.json with final values and drop the file handle.
    pub fn finalize(self, record: &SessionRecord) -> Result<()> {
        let meta_path = self.session_dir.join("metadata.json");
        let json =
            serde_json::to_string_pretty(record).with_context(|| "serializing final metadata")?;
        fs::write(&meta_path, json).with_context(|| "writing final metadata")?;
        // writer is dropped here, closing events.jsonl
        Ok(())
    }
}

/// Load a single session record by ID (directory format first, then legacy .json).
pub fn load(history_dir: &Path, session_id: &str) -> Result<SessionRecord> {
    // Try new directory format first.
    let meta_path = history_dir.join(session_id).join("metadata.json");
    if meta_path.exists() {
        let contents = fs::read_to_string(&meta_path)
            .with_context(|| format!("reading {}", meta_path.display()))?;
        return serde_json::from_str(&contents).with_context(|| "parsing session metadata JSON");
    }

    // Fall back to legacy single-file format.
    let path = history_dir.join(format!("{session_id}.json"));
    let contents =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&contents).with_context(|| "parsing session JSON")
}

/// List all session records, sorted by session number.
/// Reads from both directory-based and legacy .json formats.
pub fn list(history_dir: &Path) -> Result<Vec<SessionRecord>> {
    if !history_dir.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(history_dir).with_context(|| "reading history dir")? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // New directory format: read metadata.json
            let meta_path = path.join("metadata.json");
            if meta_path.exists() {
                let contents = fs::read_to_string(&meta_path)
                    .with_context(|| format!("reading {}", meta_path.display()))?;
                if let Ok(record) = serde_json::from_str::<SessionRecord>(&contents) {
                    records.push(record);
                }
            }
        } else if path.extension().is_some_and(|ext| ext == "json") {
            // Legacy single-file format.
            let contents =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
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
    fs::read_dir(history_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let path = e.path();
                    // Count directories with metadata.json or legacy .json files
                    if path.is_dir() {
                        path.join("metadata.json").exists()
                    } else {
                        path.extension().is_some_and(|ext| ext == "json")
                    }
                })
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

/// Load all events from a session's events.jsonl file.
pub fn load_events(history_dir: &Path, session_id: &str) -> Result<Vec<SessionEvent>> {
    let events_path = history_dir.join(session_id).join("events.jsonl");
    if !events_path.exists() {
        return Ok(Vec::new());
    }
    let file =
        File::open(&events_path).with_context(|| format!("opening {}", events_path.display()))?;
    let reader = std::io::BufReader::new(file);
    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line.with_context(|| "reading event line")?;
        if !line.trim().is_empty()
            && let Ok(event) = serde_json::from_str::<SessionEvent>(&line)
        {
            events.push(event);
        }
    }
    Ok(events)
}

/// Reconstruct the full agent response text by joining all Message events.
pub fn reconstruct_response(events: &[SessionEvent]) -> String {
    let mut response = String::new();
    for event in events {
        if let SessionEvent::Message { content } = event {
            response.push_str(content);
        }
    }
    response
}

/// Extract all command entries from the event stream.
pub fn extract_commands(events: &[SessionEvent]) -> Vec<CommandEntry> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Command {
                command,
                exit_code,
                duration_ms,
            } => Some(CommandEntry {
                command: command.clone(),
                exit_code: *exit_code,
                duration_ms: *duration_ms,
            }),
            _ => None,
        })
        .collect()
}
