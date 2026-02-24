//! Ratatui-based terminal UI using an inline viewport.
//!
//! Output lines are inserted above a fixed 2-line footer (status bar + input
//! prompt) so they scroll naturally into terminal scrollback.  After the
//! session ends the output is still visible — no replay needed.

use std::io::{self, Stderr};

use crossterm::event::{Event, EventStream, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{ExecutableCommand, cursor};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;

// ── Events ──────────────────────────────────────────────────────────────

/// Events produced by the background input/timer task.
#[derive(Debug)]
#[allow(dead_code)]
pub enum TuiEvent {
    /// A user keystroke.
    Key(KeyEvent),
    /// Time to redraw (~30fps).
    Render,
    /// Terminal was resized.
    Resize(u16, u16),
}

// ── AppState ────────────────────────────────────────────────────────────

/// Observable UI state owned by the runner.
///
/// Output is pushed to `pending_lines` which get flushed above the inline
/// viewport on each render tick.
pub struct AppState {
    /// User's typing buffer.
    pub input_buf: String,
    /// Status bar text.
    pub status: String,
    /// Styled lines waiting to be flushed above the viewport.
    pending_lines: Vec<Line<'static>>,
    /// Delta accumulator for streaming text (partial line).
    partial_line: String,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            input_buf: String::new(),
            status: String::new(),
            pending_lines: Vec::new(),
            partial_line: String::new(),
        }
    }

    /// Queue a fully styled line to be flushed above the viewport.
    pub fn flush_line(&mut self, line: Line<'static>) {
        self.pending_lines.push(line);
    }

    /// Accumulate streaming delta text.  Completed lines (split on `\n`) are
    /// flushed with the `"· "` agent prefix.
    pub fn append_delta(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                let finished = std::mem::take(&mut self.partial_line);
                self.pending_lines.push(styled_agent(&finished));
            } else {
                self.partial_line.push(ch);
            }
        }
    }

    /// Flush any remaining partial line (e.g. at end of agent turn).
    pub fn flush_partial(&mut self) {
        if !self.partial_line.is_empty() {
            let finished = std::mem::take(&mut self.partial_line);
            self.pending_lines.push(styled_agent(&finished));
        }
    }

    /// Drain pending lines for `insert_before`.
    pub fn take_pending(&mut self) -> Vec<Line<'static>> {
        std::mem::take(&mut self.pending_lines)
    }

    /// Push a character into the input buffer.
    pub fn push_char(&mut self, ch: char) {
        self.input_buf.push(ch);
    }

    /// Remove the last character from the input buffer.
    pub fn backspace(&mut self) {
        self.input_buf.pop();
    }

    /// Take the input buffer contents, clearing it.
    pub fn take_input(&mut self) -> String {
        std::mem::take(&mut self.input_buf)
    }
}

// ── Tui ─────────────────────────────────────────────────────────────────

/// Manages the ratatui terminal and background event reader.
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stderr>>,
    event_rx: mpsc::UnboundedReceiver<TuiEvent>,
    /// Whether the viewport has been drawn at least once (guards against
    /// ghost artifacts from rendering the footer before any content).
    started: bool,
}

impl Tui {
    /// Enable raw mode and create a 2-line inline viewport on stderr.
    pub fn new() -> anyhow::Result<Self> {
        // Print newlines *before* entering raw mode to push the cursor near
        // the bottom of the terminal.  This ensures the inline viewport starts
        // at the bottom so `insert_before` immediately scrolls content upward
        // instead of slowly pushing the viewport down through empty space.
        let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let pad = rows.saturating_sub(2);
        if pad > 0 {
            eprint!("{}", "\n".repeat(pad as usize));
        }

        enable_raw_mode()?;

        let backend = CrosstermBackend::new(io::stderr());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(2),
            },
        )?;

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(event_task(tx));

        Ok(Self {
            terminal,
            event_rx: rx,
            started: false,
        })
    }

    /// Receive the next TUI event (for use inside `tokio::select!`).
    pub async fn next_event(&mut self) -> Option<TuiEvent> {
        self.event_rx.recv().await
    }

    /// Render: flush pending lines above viewport, then redraw footer.
    pub fn draw(&mut self, state: &mut AppState) -> anyhow::Result<()> {
        let pending = state.take_pending();

        // Don't render the footer until we have content to insert.  Drawing
        // the footer at the initial cursor position before any insert_before
        // leaves a ghost artifact when the viewport later moves down.
        if pending.is_empty() && !self.started {
            return Ok(());
        }

        if !pending.is_empty() {
            self.started = true;
            let count = pending.len() as u16;
            self.terminal.insert_before(count, |buf| {
                let area = buf.area;
                for (i, line) in pending.iter().enumerate() {
                    if i as u16 >= area.height {
                        break;
                    }
                    let line_area = Rect {
                        x: area.x,
                        y: area.y + i as u16,
                        width: area.width,
                        height: 1,
                    };
                    Paragraph::new(line.clone()).render(line_area, buf);
                }
            })?;
        }

        self.terminal.draw(|frame| footer(frame, state))?;
        Ok(())
    }

    /// Clean up: disable raw mode and clear the 2-line inline viewport.
    pub fn restore(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        // Clear the inline viewport area so the footer doesn't linger.
        self.terminal.clear()?;
        io::stderr().execute(cursor::Show)?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Best-effort cleanup if restore() wasn't called explicitly.
        let _ = disable_raw_mode();
        let _ = io::stderr().execute(cursor::Show);
    }
}

// ── Background event task ───────────────────────────────────────────────

async fn event_task(tx: mpsc::UnboundedSender<TuiEvent>) {
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(33));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                if tx.send(TuiEvent::Render).is_err() {
                    break;
                }
            }
            ev = reader.next() => {
                match ev {
                    Some(Ok(Event::Key(key))) => {
                        if tx.send(TuiEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Event::Resize(w, h))) => {
                        if tx.send(TuiEvent::Resize(w, h)).is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
    }
}

// ── Footer rendering (2-line viewport) ──────────────────────────────────

fn footer(frame: &mut ratatui::Frame, state: &AppState) {
    let area = frame.area();

    let chunks = Layout::vertical([
        Constraint::Length(1), // status bar
        Constraint::Length(1), // input prompt
    ])
    .split(area);

    // Status bar: dark gray background, white text.
    let status_line = Line::from(vec![Span::styled(
        format!(" {}", state.status),
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )]);
    let status_bar = Paragraph::new(status_line).style(Style::default().bg(Color::DarkGray));
    frame.render_widget(status_bar, chunks[0]);

    // Input prompt: cyan "› " prefix.
    let input_line = Line::from(vec![
        Span::styled(
            "› ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(&state.input_buf),
    ]);
    let input = Paragraph::new(input_line);
    frame.render_widget(input, chunks[1]);

    // Place cursor at end of input text.
    let cursor_x = chunks[1].x + 2 + state.input_buf.len() as u16;
    let cursor_y = chunks[1].y;
    frame.set_cursor_position((cursor_x.min(area.width.saturating_sub(1)), cursor_y));
}

// ── Styled line constructors ────────────────────────────────────────────

/// Bold text for session headers (e.g. "## Session 5").
pub fn styled_header(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// Agent output: dim "· " prefix + text.
pub fn styled_agent(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("· ", Style::default().fg(Color::DarkGray)),
        Span::raw(text.to_string()),
    ])
}

/// Shell command: dim cyan "  $ " prefix + command text.
pub fn styled_command(cmd: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "  $ ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
        ),
        Span::raw(cmd.to_string()),
    ])
}

/// Non-zero exit code in red.
pub fn styled_command_exit(code: i32) -> Line<'static> {
    Line::from(Span::styled(
        format!("  exit code {code}"),
        Style::default().fg(Color::Red),
    ))
}

/// Dim status text like "[steered: ...]", "[queued: ...]", "[interrupting...]".
pub fn styled_status(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  [{text}]"),
        Style::default().fg(Color::DarkGray),
    ))
}

/// User input echo: bold cyan "› " + text.
pub fn styled_user_input(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "› ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(text.to_string()),
    ])
}

/// Empty line (visual separator).
pub fn styled_empty() -> Line<'static> {
    Line::from("")
}

/// Config detail: "  Key:  value" with dim key.
pub fn styled_detail(key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {key:<11}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.to_string()),
    ])
}

// ── Plain-text fallback ─────────────────────────────────────────────────

/// Convert a styled `Line` to plain text for non-TTY fallback.
pub fn line_to_plain(line: &Line<'_>) -> String {
    let mut out = String::new();
    for span in &line.spans {
        out.push_str(span.content.as_ref());
    }
    out
}
