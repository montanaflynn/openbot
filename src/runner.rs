//! Runtime loop that orchestrates codex sessions, memory persistence, and
//! optional worktree isolation for autonomous bot runs.

use anyhow::{Context, Result};
use chrono::Utc;
use codex_core::config::{ConfigBuilder, ConfigOverrides, find_codex_home};
use codex_core::{AuthManager, ThreadManager};
use codex_protocol::dynamic_tools::{
    DynamicToolCallOutputContentItem, DynamicToolResponse, DynamicToolSpec,
};
use codex_protocol::protocol::{
    AskForApproval, EventMsg, Op, RateLimitSnapshot, SessionSource, TokenUsageInfo,
};
use codex_protocol::user_input::UserInput;
use crossterm::event::{KeyCode, KeyModifiers};
use serde_json::json;
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{error, warn};

use crate::config::BotConfig;
use crate::git::{self, WorktreeGuard, WorktreeInfo};
use crate::history::{
    self, CommandEntry, SessionEvent, SessionRecord, SessionWriter, TokenSnapshot,
};
use crate::memory::MemoryStore;
use crate::prompt::build_prompt;
use crate::skills::load_skills;
use crate::tui::{AppState, Tui, TuiEvent};
use crate::workspace::{detect_project_root, slug_from_path};

/// Build the dynamic tool specs registered with each codex session.
fn session_tools() -> Vec<DynamicToolSpec> {
    vec![
        DynamicToolSpec {
            name: "session_complete".into(),
            description: "Signal that you have finished your work for this session. \
                Call this when you have completed your task or made all the progress you can."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Brief summary of what you accomplished this session"
                    },
                    "action": {
                        "type": "string",
                        "enum": ["merge", "review", "discard"],
                        "description": "What to do with your changes: 'merge' to merge your branch into the base branch, 'review' to leave the branch for human review, 'discard' to drop your changes"
                    }
                },
                "required": ["summary", "action"]
            }),
        },
        DynamicToolSpec {
            name: "session_history".into(),
            description: "Browse previous session history. Use action='list' for an overview \
                or action='view' with a session_number to read full transcript and commands. \
                Supports pagination with offset/limit."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "view"],
                        "description": "Action to perform: 'list' shows all sessions, 'view' shows details for a specific session"
                    },
                    "session_number": {
                        "type": "integer",
                        "description": "Session number to view (required for 'view' action)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line offset for pagination (default 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max lines to return (default 50)"
                    },
                    "section": {
                        "type": "string",
                        "enum": ["response", "commands", "all"],
                        "description": "Which section to view: 'response', 'commands', or 'all' (default 'all')"
                    }
                },
                "required": ["action"]
            }),
        },
    ]
}

/// Dual-mode output helper: TUI when interactive, plain stderr when piped.
fn emit(state: &mut Option<AppState>, text: &str, newline: bool) {
    match state {
        Some(s) => {
            if newline {
                s.append_line(text);
            } else {
                s.append_text(text);
            }
        }
        None => {
            if newline {
                eprintln!("{text}");
            } else {
                eprint!("{text}");
            }
        }
    }
}

/// Run the main agent loop, optionally resuming a previous session.
pub async fn run(
    bot_name: &str,
    config: BotConfig,
    resume_session: Option<String>,
    project: Option<String>,
    no_worktree: bool,
) -> Result<()> {
    let skill_dirs = BotConfig::skill_dirs(bot_name)?;

    let _codex_home = find_codex_home().with_context(|| "finding codex home")?;

    let sandbox_mode = config.sandbox_mode();
    // openbot sessions are unattended, so command approvals are always auto-approved.
    let approval_policy = Some(AskForApproval::Never);

    // Resolve repo root and create worktree before building codex config so we
    // can point codex at the worktree's cwd.
    let cwd_for_check = std::env::current_dir().with_context(|| "getting current directory")?;
    let repo_root = git::resolve_repo_root(&cwd_for_check);

    if !config.skip_git_check && repo_root.is_none() {
        anyhow::bail!("Not inside a git repository. Use --skip-git-check to run anyway.");
    }

    let worktree: Option<WorktreeInfo> = if !no_worktree {
        if let Some(ref root) = repo_root {
            let wt =
                git::create_worktree(root, bot_name).with_context(|| "creating git worktree")?;
            Some(wt)
        } else {
            None
        }
    } else {
        None
    };

    // Guard removes the worktree directory on exit (keeps the branch).
    let _worktree_guard = worktree
        .as_ref()
        .map(|wt| WorktreeGuard::new(wt.path.clone()));

    let overrides = ConfigOverrides {
        model: config.model.clone(),
        review_model: None,
        config_profile: None,
        approval_policy,
        sandbox_mode: Some(sandbox_mode),
        cwd: worktree.as_ref().map(|wt| wt.path.clone()),
        model_provider: None,
        codex_linux_sandbox_exe: None,
        js_repl_node_path: None,
        js_repl_node_module_dirs: None,
        zsh_path: None,
        base_instructions: None,
        developer_instructions: None,
        personality: None,
        compact_prompt: None,
        include_apply_patch_tool: None,
        show_raw_agent_reasoning: None,
        tools_web_search_request: None,
        ephemeral: None,
        additional_writable_roots: Vec::new(),
    };

    let codex_config = ConfigBuilder::default()
        .harness_overrides(overrides)
        .build()
        .await
        .with_context(|| "building codex config")?;

    // Derive a workspace slug from the project root directory name.
    // Use the original cwd (not the worktree) so worktrees of the same repo
    // share one workspace.
    let workspace_slug = if let Some(ref slug) = project {
        slug.clone()
    } else {
        let project_root = detect_project_root(&cwd_for_check);
        slug_from_path(&project_root)
    };

    let memory_path = crate::config::bot_workspace_memory_path(bot_name, &workspace_slug)?;
    let memory = MemoryStore::load(&memory_path).with_context(|| "loading memory")?;
    let history_dir = crate::config::bot_workspace_history_dir(bot_name, &workspace_slug)?;
    let history_count = history::count(&history_dir);

    let auth_manager = AuthManager::shared(
        codex_config.codex_home.clone(),
        true,
        codex_config.cli_auth_credentials_store_mode,
    );

    let thread_manager = Arc::new(ThreadManager::new(
        codex_config.codex_home.clone(),
        auth_manager.clone(),
        SessionSource::Exec,
        codex_config.model_catalog.clone(),
    ));

    // Start or resume a session.
    let codex_core::NewThread {
        thread_id: _,
        thread,
        session_configured,
    } = if let Some(ref session_id) = resume_session {
        // Try to find and resume the previous session by ID.
        let rollout_path =
            codex_core::find_thread_path_by_id_str(&codex_config.codex_home, session_id)
                .await
                .with_context(|| format!("looking up session {session_id}"))?;
        match rollout_path {
            Some(path) => thread_manager
                .resume_thread_from_rollout(codex_config.clone(), path, auth_manager.clone())
                .await
                .with_context(|| "resuming session")?,
            None => thread_manager
                .start_thread_with_tools(codex_config.clone(), session_tools(), false)
                .await
                .with_context(|| "starting codex thread")?,
        }
    } else {
        thread_manager
            .start_thread_with_tools(codex_config.clone(), session_tools(), false)
            .await
            .with_context(|| "starting codex thread")?
    };

    let session_id = session_configured.session_id.to_string();

    let default_cwd = codex_config.cwd.to_path_buf();
    let default_approval_policy = codex_config.permissions.approval_policy.value();
    let default_sandbox_policy = codex_config.permissions.sandbox_policy.get();
    let default_effort = codex_config.model_reasoning_effort;
    let default_summary = codex_config.model_reasoning_summary;

    let default_model = {
        use codex_core::models_manager::manager::RefreshStrategy;
        thread_manager
            .get_models_manager()
            .get_default_model(&codex_config.model, RefreshStrategy::OnlineIfUncached)
            .await
    };

    let max_sessions = config.max_iterations;
    let sleep_duration = Duration::from_secs(config.sleep_secs);

    // Detect whether we have an interactive terminal.
    let is_tty = std::io::stderr().is_terminal();

    // Interactive: ratatui TUI with alternate screen.
    // Non-interactive: plain stderr + line-buffered stdin.
    let mut tui: Option<Tui> = if is_tty {
        Some(Tui::new().with_context(|| "initializing TUI")?)
    } else {
        None
    };
    let mut state: Option<AppState> = if is_tty { Some(AppState::new()) } else { None };

    // Fallback line reader for non-interactive (piped) mode.
    let stdin = tokio::io::stdin();
    let mut stdin_reader = if !is_tty {
        Some(BufReader::new(stdin).lines())
    } else {
        None
    };

    let mut pending_input: Option<String> = None;
    let mut last_token_info: Option<TokenUsageInfo> = None;
    let mut last_rate_limits: Option<RateLimitSnapshot> = None;
    let mut worktree_result: Option<String> = None;
    let mut duration_secs: u64 = 0;
    let mut prompt_summary = String::new();
    let mut response_summary = String::new();
    let mut last_message = String::new();
    let mut commands_log: Vec<CommandEntry> = Vec::new();

    let session_limit = if max_sessions == 0 {
        u32::MAX
    } else {
        max_sessions
    };

    let mut event_writer: Option<SessionWriter> = None;

    'outer: for session_num in 1..=session_limit {
        // Reload skills each session so newly created ones get picked up.
        let skills = load_skills(&skill_dirs).unwrap_or_else(|e| {
            warn!("failed to reload skills: {e}");
            Vec::new()
        });

        let total_session = history_count + session_num as usize;

        let bot_skill_dir = crate::config::bot_skills_dir(bot_name)
            .unwrap_or_else(|_| std::path::PathBuf::from("skills"));
        let wt_info = worktree
            .as_ref()
            .map(|wt| (wt.branch.as_str(), wt.base_branch.as_str()));
        let recent_history = history::recent(&history_dir, 5).unwrap_or_default();
        let prompt = build_prompt(
            &config.instructions,
            &skills,
            &memory,
            &recent_history,
            total_session,
            &bot_skill_dir,
            Some(&workspace_slug),
            wt_info,
            pending_input.as_deref(),
        );

        // Consume pending input once it's included in the prompt.
        pending_input = None;

        let items = vec![UserInput::Text {
            text: prompt.clone(),
            text_elements: Vec::new(),
        }];

        // Print session header with config details.
        emit(
            &mut state,
            &format!("\n## Session {}\n", total_session),
            true,
        );
        emit(&mut state, &format!("Model:     {}", default_model), true);
        emit(&mut state, &format!("Workspace: {}", workspace_slug), true);
        if let Some(ref wt) = worktree {
            emit(&mut state, &format!("Branch:    {}", wt.branch), true);
        }
        emit(&mut state, &format!("Skills:    {}", skills.len()), true);
        emit(
            &mut state,
            &format!("Memory:    {} entries", memory.memory.entries.len()),
            true,
        );
        emit(
            &mut state,
            &format!("History:   {} sessions", history_count),
            true,
        );

        // Update status bar for TUI mode.
        if let Some(ref mut s) = state {
            let mut status_parts = vec![default_model.clone()];
            status_parts.push(format!("session {}", total_session));
            s.status = status_parts.join(" | ");
        }

        let session_start = Instant::now();

        // Create the event writer to stream events to disk.
        let initial_record = SessionRecord {
            session_id: session_id.clone(),
            session_number: total_session,
            started_at: Utc::now(),
            duration_secs: 0,
            model: default_model.clone(),
            prompt_summary: truncate_string(&config.instructions, 100),
            response_summary: String::new(),
            action: None,
            tokens: None,
            command_count: Some(0),
        };
        event_writer = SessionWriter::create(&history_dir, &initial_record)
            .map_err(|e| warn!("failed to create event writer: {e}"))
            .ok();

        thread
            .submit(Op::UserTurn {
                items,
                cwd: default_cwd.clone(),
                approval_policy: default_approval_policy,
                sandbox_policy: default_sandbox_policy.clone(),
                model: default_model.clone(),
                effort: default_effort,
                summary: default_summary,
                final_output_json_schema: None,
                collaboration_mode: None,
                personality: None,
            })
            .await
            .with_context(|| "submitting user turn")?;

        emit(&mut state, "\n### Output\n", true);
        last_message.clear();
        commands_log.clear();
        let mut session_completed = false;
        let mut completion_summary = String::new();
        let mut completion_action = String::new();

        loop {
            // Listen for codex events, TUI events, and piped stdin.
            let event = tokio::select! {
                ev = thread.next_event() => ev.with_context(|| "receiving event")?,

                // TUI events (interactive mode).
                Some(tui_event) = async {
                    match tui.as_mut() {
                        Some(t) => t.next_event().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match tui_event {
                        TuiEvent::Key(key) => {
                            match (key.code, key.modifiers) {
                                (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                                    break 'outer;
                                }
                                (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
                                    let empty = state.as_ref().is_none_or(|s| s.input_buf.is_empty());
                                    if empty { break 'outer; }
                                }
                                (KeyCode::Esc, _) => {
                                    emit(&mut state, "  [interrupting...]", true);
                                    thread.submit(Op::Interrupt).await.ok();
                                }
                                (KeyCode::Enter, _) => {
                                    if let Some(ref mut s) = state {
                                        let text = s.take_input();
                                        if !text.trim().is_empty() {
                                            let items = vec![UserInput::Text {
                                                text: text.clone(),
                                                text_elements: Vec::new(),
                                            }];
                                            match thread.steer_input(items, None).await {
                                                Ok(_) => emit(&mut state, &format!("  [steered: {}]", text), true),
                                                Err(_) => {
                                                    emit(&mut state, &format!("  [queued: {}]", text), true);
                                                    pending_input = Some(text);
                                                }
                                            }
                                        }
                                    }
                                }
                                (KeyCode::Backspace, _) => {
                                    if let Some(ref mut s) = state {
                                        s.backspace();
                                    }
                                }
                                (KeyCode::Char(ch), m) if !m.contains(KeyModifiers::CONTROL) => {
                                    if let Some(ref mut s) = state {
                                        s.push_char(ch);
                                    }
                                }
                                (KeyCode::PageUp, _) => {
                                    if let Some(ref mut s) = state {
                                        s.scroll_up(10);
                                    }
                                }
                                (KeyCode::PageDown, _) => {
                                    if let Some(ref mut s) = state {
                                        s.scroll_down(10);
                                    }
                                }
                                _ => {}
                            }
                        }
                        TuiEvent::Render => {
                            if let (Some(t), Some(s)) = (tui.as_mut(), state.as_ref()) {
                                t.draw(s).ok();
                            }
                        }
                        TuiEvent::Resize(_, _) => {
                            // ratatui handles resize automatically on next draw.
                        }
                    }
                    continue;
                }

                // Fallback: line-buffered stdin for non-interactive mode.
                result = async {
                    match stdin_reader.as_mut() {
                        Some(reader) => reader.next_line().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match result {
                        Ok(Some(input)) if !input.trim().is_empty() => {
                            let items = vec![UserInput::Text {
                                text: input.clone(),
                                text_elements: Vec::new(),
                            }];
                            match thread.steer_input(items, None).await {
                                Ok(_) => emit(&mut state, &format!("  [steered: {}]", input), true),
                                Err(_) => {
                                    emit(&mut state, &format!("  [queued: {}]", input), true);
                                    pending_input = Some(input);
                                }
                            }
                        }
                        Ok(None) => {
                            break 'outer;
                        }
                        _ => {}
                    }
                    continue;
                }
            };

            match &event.msg {
                EventMsg::AgentMessage(msg) => {
                    // AgentMessage contains the full accumulated text; prefer
                    // streaming deltas when available and only use this as a
                    // fallback so the message isn't printed twice.
                    if !msg.message.is_empty() {
                        if last_message.is_empty() {
                            emit(&mut state, &msg.message, true);
                        }
                        last_message = msg.message.clone();
                    }
                }
                EventMsg::AgentMessageDelta(delta) => {
                    if !delta.delta.is_empty() {
                        emit(&mut state, &delta.delta, false);
                        last_message.push_str(&delta.delta);
                        if let Some(ref mut w) = event_writer {
                            w.append_event(&SessionEvent::Message {
                                content: delta.delta.clone(),
                            })
                            .ok();
                        }
                    }
                }
                EventMsg::ExecCommandBegin(cmd) => {
                    emit(&mut state, &format!("  $ {}", cmd.command.join(" ")), true);
                }
                EventMsg::ExecCommandEnd(result) => {
                    if result.exit_code != 0 {
                        emit(
                            &mut state,
                            &format!("  exit code {}", result.exit_code),
                            true,
                        );
                    }
                    let cmd = result.command.join(" ");
                    let dur = result.duration.as_millis() as u64;
                    commands_log.push(CommandEntry {
                        command: cmd.clone(),
                        exit_code: result.exit_code,
                        duration_ms: dur,
                    });
                    if let Some(ref mut w) = event_writer {
                        w.append_event(&SessionEvent::Command {
                            command: cmd,
                            exit_code: result.exit_code,
                            duration_ms: dur,
                        })
                        .ok();
                    }
                }
                EventMsg::DynamicToolCallRequest(req) if req.tool == "session_history" => {
                    let result_text = handle_session_history_tool(&req.arguments, &history_dir);
                    thread
                        .submit(Op::DynamicToolResponse {
                            id: req.call_id.clone(),
                            response: DynamicToolResponse {
                                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                                    text: result_text,
                                }],
                                success: true,
                            },
                        })
                        .await
                        .ok();
                }
                EventMsg::DynamicToolCallRequest(req) if req.tool == "session_complete" => {
                    let summary = req
                        .arguments
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let action = req
                        .arguments
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("review")
                        .to_string();
                    completion_summary = summary;
                    completion_action = action;
                    session_completed = true;

                    // Respond to the tool call so the turn can finish.
                    thread
                        .submit(Op::DynamicToolResponse {
                            id: req.call_id.clone(),
                            response: DynamicToolResponse {
                                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                                    text: "Session complete. Good work.".into(),
                                }],
                                success: true,
                            },
                        })
                        .await
                        .ok();
                }
                EventMsg::TurnComplete(_) => {
                    break;
                }
                EventMsg::TurnAborted(_) => {
                    break;
                }
                EventMsg::Error(e) => {
                    error!("Error from codex: {:?}", e);
                    break;
                }
                EventMsg::ExecApprovalRequest(req) => {
                    let id = req.approval_id.clone().unwrap_or_default();
                    thread
                        .submit(Op::ExecApproval {
                            id,
                            turn_id: Some(req.turn_id.clone()),
                            decision: codex_protocol::protocol::ReviewDecision::Approved,
                        })
                        .await
                        .ok();
                }
                EventMsg::TokenCount(tc) => {
                    if let Some(ref info) = tc.info {
                        last_token_info = Some(info.clone());
                        if let Some(ref mut w) = event_writer {
                            let u = &info.total_token_usage;
                            w.append_event(&SessionEvent::TokenCount {
                                input_tokens: u.input_tokens,
                                cached_input_tokens: u.cached_input_tokens,
                                output_tokens: u.output_tokens,
                                reasoning_output_tokens: u.reasoning_output_tokens,
                                context_window: info.model_context_window,
                            })
                            .ok();
                        }
                    }
                    if let Some(ref rl) = tc.rate_limits {
                        last_rate_limits = Some(rl.clone());
                    }
                }
                _ => {}
            }
        }

        // Ensure a clean newline after streamed LLM output.
        emit(&mut state, "", true);

        // Save session results.
        duration_secs = session_start.elapsed().as_secs();
        prompt_summary = truncate_string(&config.instructions, 100);
        response_summary = if completion_summary.is_empty() {
            truncate_string(&last_message, 500)
        } else {
            completion_summary.clone()
        };

        if session_completed {
            // Post-hook: execute the action the LLM chose.
            if let Some(ref wt) = worktree {
                let result = match completion_action.as_str() {
                    "merge" => {
                        let mut result = format!("merged {} into {}", wt.branch, wt.base_branch);
                        let output = std::process::Command::new("git")
                            .args(["checkout", &wt.base_branch])
                            .current_dir(&cwd_for_check)
                            .output();
                        if let Ok(o) = output
                            && o.status.success()
                        {
                            let merge = std::process::Command::new("git")
                                .args(["merge", "--ff-only", &wt.branch])
                                .current_dir(&cwd_for_check)
                                .output();
                            match merge {
                                Ok(m) if !m.status.success() => {
                                    result = format!(
                                        "merge failed; branch {} available for manual merge",
                                        wt.branch
                                    );
                                }
                                Err(_) => {
                                    result = format!(
                                        "merge failed; branch {} available for manual merge",
                                        wt.branch
                                    );
                                }
                                _ => {}
                            }
                        }
                        result
                    }
                    "discard" => {
                        format!("discarded (branch {} kept)", wt.branch)
                    }
                    _ => {
                        format!(
                            "review branch {}\n  git log {}..{}\n  git merge {}",
                            wt.branch, wt.base_branch, wt.branch, wt.branch
                        )
                    }
                };
                worktree_result = Some(result);
            }
            break;
        }

        if session_num >= session_limit {
            break;
        }

        // Sleep between sessions, wake on user input or ctrl-c.
        if config.sleep_secs > 0 {
            emit(
                &mut state,
                &format!("\nSleeping {}s (type to wake)...", config.sleep_secs),
                true,
            );

            // Update status bar during sleep.
            if let Some(ref mut s) = state {
                s.status = format!("{} | sleeping...", s.status);
            }

            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {}

                // TUI events during sleep.
                Some(tui_event) = async {
                    match tui.as_mut() {
                        Some(t) => t.next_event().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match tui_event {
                        TuiEvent::Key(key) => {
                            match (key.code, key.modifiers) {
                                (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                                    break 'outer;
                                }
                                (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
                                    let empty = state.as_ref().is_none_or(|s| s.input_buf.is_empty());
                                    if empty { break 'outer; }
                                }
                                (KeyCode::Enter, _) => {
                                    if let Some(ref mut s) = state {
                                        let text = s.take_input();
                                        if !text.trim().is_empty() {
                                            emit(&mut state, &format!("Received: {}", text), true);
                                            pending_input = Some(text);
                                        }
                                    }
                                }
                                (KeyCode::Backspace, _) => {
                                    if let Some(ref mut s) = state {
                                        s.backspace();
                                    }
                                }
                                (KeyCode::Char(ch), m) if !m.contains(KeyModifiers::CONTROL) => {
                                    if let Some(ref mut s) = state {
                                        s.push_char(ch);
                                    }
                                }
                                _ => {}
                            }
                        }
                        TuiEvent::Render => {
                            if let (Some(t), Some(s)) = (tui.as_mut(), state.as_ref()) {
                                t.draw(s).ok();
                            }
                        }
                        TuiEvent::Resize(_, _) => {}
                    }
                }

                // Fallback: line-buffered stdin for non-interactive mode.
                result = async {
                    match stdin_reader.as_mut() {
                        Some(reader) => reader.next_line().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match result {
                        Ok(Some(input)) if !input.trim().is_empty() => {
                            emit(&mut state, &format!("Received: {}", input), true);
                            pending_input = Some(input);
                        }
                        Ok(None) => {
                            break 'outer;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Build and save the session record.
    let tokens = last_token_info.as_ref().map(|info| {
        let u = &info.total_token_usage;
        TokenSnapshot {
            input_tokens: u.input_tokens,
            cached_input_tokens: u.cached_input_tokens,
            output_tokens: u.output_tokens,
            reasoning_output_tokens: u.reasoning_output_tokens,
            context_window: info.model_context_window,
        }
    });

    let record = SessionRecord {
        session_id: session_id.clone(),
        session_number: history_count + 1,
        started_at: Utc::now(),
        duration_secs,
        model: default_model.clone(),
        prompt_summary,
        response_summary: response_summary.clone(),
        action: worktree_result.clone(),
        tokens,
        command_count: Some(commands_log.len()),
    };
    if let Some(writer) = event_writer.take() {
        writer.finalize(&record).ok();
    }

    // Restore the terminal before printing the summary so it appears in
    // normal scrollback (visible after the alternate screen exits).
    if let Some(ref mut t) = tui {
        t.restore().ok();
    }

    // Replay session output to stderr so it's visible in scrollback after
    // the alternate screen exits.
    if let Some(ref s) = state {
        for line in &s.output_lines {
            eprintln!("{line}");
        }
        if !s.current_line.is_empty() {
            eprintln!("{}", s.current_line);
        }
    }

    // Drop the TUI and state so no further draws happen.
    drop(tui);
    drop(state);

    // Print summary to plain stderr (alternate screen already exited).
    eprintln!("\n### Summary\n");
    eprintln!("Result:    {}", truncate_string(&response_summary, 200));
    if let Some(ref wt_result) = worktree_result {
        eprintln!("Action:    {}", wt_result);
    }
    eprintln!("Duration:  {}s", duration_secs);
    if let Some(ref info) = last_token_info {
        let u = &info.total_token_usage;
        eprintln!(
            "Tokens:    {} input ({} cached) / {} output ({} reasoning)",
            u.input_tokens, u.cached_input_tokens, u.output_tokens, u.reasoning_output_tokens,
        );
        if let Some(ctx) = info.model_context_window {
            let pct = u.percent_of_context_window_remaining(ctx);
            eprintln!("Context:   {}% remaining ({} window)", pct, ctx);
        }
    }
    if let Some(ref rl) = last_rate_limits {
        if let Some(ref primary) = rl.primary {
            let reset_str = match primary.resets_at {
                Some(ts) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let remaining = (ts - now).max(0);
                    if remaining >= 60 {
                        format!(" (resets in {}m)", remaining / 60)
                    } else {
                        format!(" (resets in {}s)", remaining)
                    }
                }
                None => String::new(),
            };
            eprintln!("Rate:      {:.0}% used{}", primary.used_percent, reset_str);
        }
        if let Some(ref credits) = rl.credits {
            if credits.unlimited {
                eprintln!("Credits:   unlimited");
            } else if let Some(ref balance) = credits.balance {
                eprintln!("Credits:   ${}", balance);
            }
        }
        if let Some(ref plan) = rl.plan_type {
            eprintln!("Plan:      {:?}", plan);
        }
    }
    eprintln!("Resume:    openbot run --resume {session_id}");

    // Shut down codex with a timeout.
    thread.submit(Op::Shutdown).await.ok();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match thread.next_event().await {
                Ok(event) if matches!(event.msg, EventMsg::ShutdownComplete) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    })
    .await;

    Ok(())
}

/// Handle calls to the `session_history` dynamic tool.
fn handle_session_history_tool(args: &serde_json::Value, history_dir: &std::path::Path) -> String {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("list");

    match action {
        "list" => {
            let records = match history::list(history_dir) {
                Ok(r) => r,
                Err(e) => return format!("Error loading history: {e}"),
            };
            if records.is_empty() {
                return "No previous sessions found.".into();
            }
            let mut out = String::from("Session | Date | Duration | Commands | Summary\n");
            out.push_str("--------|------|----------|----------|--------\n");
            for r in &records {
                let date = r.started_at.format("%Y-%m-%d %H:%M");
                let cmd_count = r.command_count.unwrap_or(0);
                let summary = truncate_string(&r.response_summary, 80);
                out.push_str(&format!(
                    "{} | {} | {}s | {} | {}\n",
                    r.session_number, date, r.duration_secs, cmd_count, summary,
                ));
            }
            out.push_str(&format!(
                "\n{} sessions total. Use action='view' with session_number to see details.",
                records.len()
            ));
            out
        }
        "view" => {
            let session_number = args
                .get("session_number")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            if session_number == 0 {
                return "session_number is required for the 'view' action.".into();
            }

            let records = match history::list(history_dir) {
                Ok(r) => r,
                Err(e) => return format!("Error loading history: {e}"),
            };
            let record = records.iter().find(|r| r.session_number == session_number);
            let record = match record {
                Some(r) => r,
                None => return format!("Session {session_number} not found."),
            };

            let section = args
                .get("section")
                .and_then(|v| v.as_str())
                .unwrap_or("all");
            // offset = how many lines back from the end to start (0 = last page)
            let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

            let mut lines: Vec<String> = Vec::new();

            // Header (always at the top of content)
            lines.push(format!("# Session {}", record.session_number));
            lines.push(format!(
                "Date: {} | Model: {} | Duration: {}s",
                record.started_at.format("%Y-%m-%d %H:%M:%S"),
                record.model,
                record.duration_secs,
            ));
            lines.push(format!("Summary: {}", record.response_summary));
            lines.push(String::new());

            // Load events from events.jsonl (empty vec for legacy sessions).
            let events = history::load_events(history_dir, &record.session_id).unwrap_or_default();

            if section == "all" || section == "commands" {
                lines.push("## Commands".into());
                let cmds = history::extract_commands(&events);
                if cmds.is_empty() {
                    lines.push("(no commands executed)".into());
                } else {
                    for cmd in &cmds {
                        let status = if cmd.exit_code == 0 {
                            "ok".to_string()
                        } else {
                            format!("exit {}", cmd.exit_code)
                        };
                        lines.push(format!(
                            "$ {} [{}] ({}ms)",
                            cmd.command, status, cmd.duration_ms
                        ));
                    }
                }
                lines.push(String::new());
            }

            if section == "all" || section == "response" {
                lines.push("## Full Response".into());
                let response = history::reconstruct_response(&events);
                if response.is_empty() {
                    lines.push("(Full response not available for this session)".into());
                } else {
                    for line in response.lines() {
                        lines.push(line.to_string());
                    }
                }
            }

            // Paginate from the end: offset=0 shows the last `limit` lines.
            let total = lines.len();
            let end = total.saturating_sub(offset);
            let start = end.saturating_sub(limit);
            let page: Vec<&str> = lines[start..end].iter().map(|s| s.as_str()).collect();

            let mut out = page.join("\n");
            out.push_str(&format!("\n\n[lines {}-{} of {}]", start + 1, end, total));
            if start > 0 {
                out.push_str(&format!(
                    " Earlier content: offset={}, limit={}",
                    offset + limit,
                    limit
                ));
            }
            out
        }
        _ => format!("Unknown action '{action}'. Use 'list' or 'view'."),
    }
}

/// Return a truncated display string with an ellipsis when over max bytes.
fn truncate_string(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
