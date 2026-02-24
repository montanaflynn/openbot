//! Runtime loop that orchestrates codex sessions, memory persistence, and
//! optional worktree isolation for autonomous bot runs.

use anyhow::{Context, Result};
use codex_core::config::{ConfigBuilder, ConfigOverrides, find_codex_home};
use codex_core::{AuthManager, ThreadManager};
use codex_protocol::dynamic_tools::{
    DynamicToolCallOutputContentItem, DynamicToolResponse, DynamicToolSpec,
};
use codex_protocol::protocol::{AskForApproval, EventMsg, Op, SessionSource};
use codex_protocol::user_input::UserInput;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tracing::{error, warn};

use crate::config::BotConfig;
use crate::git::{self, WorktreeGuard, WorktreeInfo};
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
                }
            },
            "required": ["summary"]
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
    {
        let skills = load_skills(&skill_dirs).unwrap_or_else(|_| Vec::new());
        if !skills.is_empty() {
            eprintln!("Loaded {} skill(s)", skills.len());
            for skill in &skills {
                eprintln!("  - {}: {}", skill.name, skill.description);
            }
        }
    }

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
            eprintln!("Worktree: {} (branch: {})", wt.path.display(), wt.branch);
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
        let slug = slug_from_path(&project_root);
        eprintln!("Project: {} (workspace: {})", project_root.display(), slug);
        slug
    };

    let memory_path = crate::config::bot_workspace_memory_path(bot_name, &workspace_slug)?;
    let mut memory = MemoryStore::load(&memory_path).with_context(|| "loading memory")?;
    eprintln!(
        "Memory: {} entries, {} history records (workspace: {})",
        memory.memory.entries.len(),
        memory.memory.history.len(),
        workspace_slug,
    );

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
                eprintln!("Resuming session {session_id}...");
                thread_manager
                    .resume_thread_from_rollout(codex_config.clone(), path, auth_manager.clone())
                    .await
                    .with_context(|| "resuming session")?
            }
            None => {
                eprintln!("Session {session_id} not found, starting new session.");
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
    eprintln!(
        "Session {} (model: {})",
        &session_id, &session_configured.model
    );

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

    let session_limit = if max_sessions == 0 {
        u32::MAX
    } else {
        max_sessions
    };

    'outer: for session_num in 1..=session_limit {
        if *shutdown_rx.borrow() {
            break;
        }

        eprintln!("\n--- Session {} ---", session_num);

        // Reload skills each session so newly created ones get picked up.
        let skills = load_skills(&skill_dirs).unwrap_or_else(|e| {
            warn!("failed to reload skills: {e}");
            Vec::new()
        });

        let bot_skill_dir = crate::config::bot_skills_dir(bot_name)
            .unwrap_or_else(|_| std::path::PathBuf::from("skills"));
        let wt_info = worktree
            .as_ref()
            .map(|wt| (wt.branch.as_str(), wt.base_branch.as_str()));
        let prompt = build_prompt(
            &config.instructions,
            &skills,
            &memory,
            session_num,
            &bot_skill_dir,
            Some(&workspace_slug),
            wt_info,
        );

        let items = vec![UserInput::Text {
            text: prompt.clone(),
            text_elements: Vec::new(),
        }];

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

        let mut last_message = String::new();
        let mut session_completed = false;
        let mut completion_summary = String::new();

        loop {
            let event = thread
                .next_event()
                .await
                .with_context(|| "receiving event")?;

            match &event.msg {
                EventMsg::AgentMessage(msg) => {
                    if !msg.message.is_empty() {
                        println!("{}", msg.message);
                        last_message = msg.message.clone();
                    }
                }
                EventMsg::AgentMessageDelta(delta) => {
                    if !delta.delta.is_empty() {
                        print!("{}", delta.delta);
                        last_message.push_str(&delta.delta);
                    }
                }
                EventMsg::ExecCommandBegin(cmd) => {
                    eprintln!("[exec] {}", cmd.command.join(" "));
                }
                EventMsg::ExecCommandEnd(result) => {
                    if result.exit_code != 0 {
                        eprintln!("[exec] exited with code {}", result.exit_code);
                    }
                }
                EventMsg::DynamicToolCallRequest(req) if req.tool == "session_complete" => {
                    let summary = req
                        .arguments
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    eprintln!("\n[session_complete] {summary}");
                    completion_summary = summary;
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
                _ => {}
            }
        }

        // Save session results.
        let prompt_summary = truncate_string(&config.instructions, 100);
        let response_summary = if completion_summary.is_empty() {
            truncate_string(&last_message, 500)
        } else {
            completion_summary.clone()
        };
        memory.add_iteration(session_num, &prompt_summary, &response_summary);
        memory.save().with_context(|| "saving memory")?;

        if *shutdown_rx.borrow() {
            break;
        }

        if session_completed {
            eprintln!("\nSession complete.");
            break;
        }

        if session_num >= session_limit {
            break;
        }

        // Sleep between sessions, but wake on user input or ctrl-c.
        if config.sleep_secs > 0 {
            eprintln!(
                "Sleeping {} seconds (type to wake and inject input)...",
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

    // Print summary.
    let total = memory.memory.history.len();
    eprintln!("\n--- Summary ---");
    eprintln!("Completed {} session(s)", total);
    if let Some(last) = memory.memory.history.last() {
        eprintln!(
            "Last response: {}",
            truncate_string(&last.response_summary, 200)
        );
    }

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

    // Always print resume hint so the user can pick up where they left off.
    eprintln!("\nTo resume this session:\n  openbot run --resume {session_id}");

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
