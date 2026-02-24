//! Ratatui-based terminal UI.
//!
//! Provides a buffered alternate-screen interface with a scrolling output area,
//! a status bar, and a fixed input prompt at the bottom. A background task
//! translates crossterm events and a 30fps render timer into [`TuiEvent`]s
//! consumed by the runner's main `tokio::select!` loop.

use std::io::{self, Stderr};

use crossterm::event::{Event, EventStream, KeyEvent};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, cursor};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use tokio::sync::mpsc;

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

/// Observable UI state owned by the runner.
pub struct AppState {
    /// Completed output lines (scrollback buffer).
    pub output_lines: Vec<String>,
    /// Partial line being assembled from streaming deltas.
    pub current_line: String,
    /// User's typing buffer.
    pub input_buf: String,
    /// Status bar text.
    pub status: String,
    /// Vertical scroll offset (lines from the bottom).
    pub scroll_offset: u16,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            output_lines: Vec::new(),
            current_line: String::new(),
            input_buf: String::new(),
            status: String::new(),
            scroll_offset: 0,
        }
    }

    /// Append streaming text. Newlines within `text` flush completed lines.
    pub fn append_text(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                let finished = std::mem::take(&mut self.current_line);
                self.output_lines.push(finished);
            } else {
                self.current_line.push(ch);
            }
        }
        // Auto-scroll to bottom when new content arrives.
        self.scroll_offset = 0;
    }

    /// Append a complete line (flushes any partial current_line first).
    pub fn append_line(&mut self, text: &str) {
        if !self.current_line.is_empty() {
            let finished = std::mem::take(&mut self.current_line);
            self.output_lines.push(finished);
        }
        self.output_lines.push(text.to_string());
        self.scroll_offset = 0;
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

    /// Scroll up by `n` lines.
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
    }

    /// Scroll down by `n` lines (towards bottom).
    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }
}

/// Manages the ratatui terminal and background event reader.
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stderr>>,
    event_rx: mpsc::UnboundedReceiver<TuiEvent>,
}

impl Tui {
    /// Enter alternate screen, enable raw mode, and start the event reader.
    pub fn new() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        io::stderr().execute(EnterAlternateScreen)?;
        io::stderr().execute(cursor::Hide)?;

        let backend = CrosstermBackend::new(io::stderr());
        let terminal = Terminal::new(backend)?;

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(event_task(tx));

        Ok(Self {
            terminal,
            event_rx: rx,
        })
    }

    /// Receive the next TUI event (for use inside `tokio::select!`).
    pub async fn next_event(&mut self) -> Option<TuiEvent> {
        self.event_rx.recv().await
    }

    /// Render the UI from the current `AppState`.
    pub fn draw(&mut self, state: &AppState) -> anyhow::Result<()> {
        self.terminal.draw(|frame| ui(frame, state))?;
        Ok(())
    }

    /// Leave alternate screen and disable raw mode, restoring the terminal.
    pub fn restore(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        io::stderr().execute(LeaveAlternateScreen)?;
        io::stderr().execute(cursor::Show)?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Best-effort cleanup if restore() wasn't called explicitly.
        let _ = disable_raw_mode();
        let _ = io::stderr().execute(LeaveAlternateScreen);
        let _ = io::stderr().execute(cursor::Show);
    }
}

/// Background task: reads crossterm events + fires a 30fps render tick.
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

/// Render the three-area layout: output, status bar, input prompt.
fn ui(frame: &mut ratatui::Frame, state: &AppState) {
    let area = frame.area();

    let chunks = Layout::vertical([
        Constraint::Min(1),    // output area
        Constraint::Length(1), // status bar
        Constraint::Length(1), // input prompt
    ])
    .split(area);

    // --- Output area ---
    // Build the complete text: all finished lines + current partial line.
    let mut lines: Vec<Line> = state
        .output_lines
        .iter()
        .map(|s| Line::from(s.as_str()))
        .collect();
    if !state.current_line.is_empty() {
        lines.push(Line::from(state.current_line.as_str()));
    }

    let output_height = chunks[0].height as usize;
    let total_lines = lines.len();

    // Calculate scroll position: scroll_offset=0 means pinned to bottom.
    let scroll = if total_lines > output_height {
        let max_scroll = (total_lines - output_height) as u16;
        let offset = state.scroll_offset.min(max_scroll);
        max_scroll - offset
    } else {
        0
    };

    let output = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(output, chunks[0]);

    // --- Status bar ---
    let status_line = Line::from(vec![Span::styled(
        format!(" {}", state.status),
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )]);
    let status_bar = Paragraph::new(status_line).style(Style::default().bg(Color::DarkGray));
    frame.render_widget(status_bar, chunks[1]);

    // --- Input prompt ---
    let input_line = Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Green)),
        Span::raw(&state.input_buf),
    ]);
    let input = Paragraph::new(input_line);
    frame.render_widget(input, chunks[2]);

    // Place cursor at end of input text.
    let cursor_x = chunks[2].x + 2 + state.input_buf.len() as u16;
    let cursor_y = chunks[2].y;
    frame.set_cursor_position((cursor_x.min(area.width.saturating_sub(1)), cursor_y));
}
