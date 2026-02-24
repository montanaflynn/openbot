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
use serde_json::json;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tracing::{error, warn};

use crate::config::BotConfig;
use crate::git::{self, WorktreeGuard, WorktreeInfo};
use crate::history::{self, SessionRecord, TokenSnapshot};
use crate::memory::MemoryStore;
use crate::prompt::build_prompt;
use crate::skills::load_skills;
use crate::workspace::{detect_project_root, slug_from_path};

/// Build the dynamic tool specs registered with each codex session.
fn session_tools() -> Vec<DynamicToolSpec> {
    vec![DynamicToolSpec {
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
    }]
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
    let mut memory = MemoryStore::load(&memory_path).with_context(|| "loading memory")?;
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
            Some(path) => {
                thread_manager
                    .resume_thread_from_rollout(codex_config.clone(), path, auth_manager.clone())
                    .await
                    .with_context(|| "resuming session")?
            }
            None => {
                thread_manager
                    .start_thread_with_tools(codex_config.clone(), session_tools(), false)
                    .await
                    .with_context(|| "starting codex thread")?
            }
        }
    } else {
        thread_manager
            .start_thread_with_tools(codex_config.clone(), session_tools(), false)
            .await
            .with_context(|| "starting codex thread")?
    };

    let session_id = session_configured.session_id.to_string();

    // Set up ctrl-c handler for graceful shutdown.
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let thread_for_ctrlc = thread.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\nInterrupted, shutting down gracefully...");
            // Signal the main loop to stop.
            shutdown_tx.send(true).ok();
            // Tell codex to abort any in-flight work.
            thread_for_ctrlc.submit(Op::Interrupt).await.ok();
        }
    });

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

    let stdin = tokio::io::stdin();
    let mut stdin_reader = BufReader::new(stdin).lines();

    let mut last_token_info: Option<TokenUsageInfo> = None;
    let mut last_rate_limits: Option<RateLimitSnapshot> = None;
    let mut worktree_result: Option<String> = None;
    let mut duration_secs: u64 = 0;
    let mut prompt_summary = String::new();
    let mut response_summary = String::new();

    let session_limit = if max_sessions == 0 {
        u32::MAX
    } else {
        max_sessions
    };

    'outer: for session_num in 1..=session_limit {
        if *shutdown_rx.borrow() {
            break;
        }

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
        );

        let items = vec![UserInput::Text {
            text: prompt.clone(),
            text_elements: Vec::new(),
        }];

        // Print session header with config details.
        eprintln!("\n## Session {}\n", total_session);
        eprintln!("Model:     {}", default_model);
        eprintln!("Workspace: {}", workspace_slug);
        if let Some(ref wt) = worktree {
            eprintln!("Branch:    {}", wt.branch);
        }
        eprintln!("Skills:    {}", skills.len());
        eprintln!("Memory:    {} entries", memory.memory.entries.len());
        eprintln!("History:   {} sessions", history_count);

        let session_start = Instant::now();

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

        eprintln!("\n### Output\n");
        let mut last_message = String::new();
        let mut session_completed = false;
        let mut completion_summary = String::new();
        let mut completion_action = String::new();

        loop {
            let event = thread
                .next_event()
                .await
                .with_context(|| "receiving event")?;

            match &event.msg {
                EventMsg::AgentMessage(msg) => {
                    // AgentMessage contains the full accumulated text; prefer
                    // streaming deltas when available and only use this as a
                    // fallback so the message isn't printed twice.
                    if !msg.message.is_empty() {
                        if last_message.is_empty() {
                            eprintln!("{}", msg.message);
                        }
                        last_message = msg.message.clone();
                    }
                }
                EventMsg::AgentMessageDelta(delta) => {
                    if !delta.delta.is_empty() {
                        eprint!("{}", delta.delta);
                        std::io::stderr().flush().ok();
                        last_message.push_str(&delta.delta);
                    }
                }
                EventMsg::ExecCommandBegin(cmd) => {
                    eprintln!("  $ {}", cmd.command.join(" "));
                }
                EventMsg::ExecCommandEnd(result) => {
                    if result.exit_code != 0 {
                        eprintln!("  exit code {}", result.exit_code);
                    }
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
                    }
                    if let Some(ref rl) = tc.rate_limits {
                        last_rate_limits = Some(rl.clone());
                    }
                }
                _ => {}
            }
        }

        // Ensure a clean newline after streamed LLM output.
        eprintln!();

        // Save session results.
        duration_secs = session_start.elapsed().as_secs();
        prompt_summary = truncate_string(&config.instructions, 100);
        response_summary = if completion_summary.is_empty() {
            truncate_string(&last_message, 500)
        } else {
            completion_summary.clone()
        };

        if *shutdown_rx.borrow() {
            break;
        }

        if session_completed {
            // Post-hook: execute the action the LLM chose.
            if let Some(ref wt) = worktree {
                let result = match completion_action.as_str() {
                    "merge" => {
                        let mut result = format!(
                            "merged {} into {}",
                            wt.branch, wt.base_branch
                        );
                        let output = std::process::Command::new("git")
                            .args(["checkout", &wt.base_branch])
                            .current_dir(&cwd_for_check)
                            .output();
                        if let Ok(o) = output {
                            if o.status.success() {
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

        // Sleep between sessions, but wake on user input or ctrl-c.
        if config.sleep_secs > 0 {
            eprintln!(
                "\n  Sleeping {}s (type to wake)...",
                config.sleep_secs
            );

            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {}
                line = stdin_reader.next_line() => {
                    match line {
                        Ok(Some(input)) if !input.trim().is_empty() => {
                            eprintln!("User input received, injecting into next session.");
                            memory.set("user_input".into(), input);
                            memory.save().ok();
                        }
                        Ok(None) => {
                            eprintln!("stdin closed, continuing.");
                        }
                        _ => {}
                    }
                }
                _ = shutdown_rx.changed() => {
                    break 'outer;
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
    };
    history::save(&history_dir, &record).ok();

    // Print summary.
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

/// Return a truncated display string with an ellipsis when over max bytes.
fn truncate_string(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
