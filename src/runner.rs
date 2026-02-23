use anyhow::{Context, Result};
use codex_core::config::{ConfigBuilder, ConfigOverrides, find_codex_home};
use codex_core::{AuthManager, ThreadManager};
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::{AskForApproval, EventMsg, Op, SessionSource};
use codex_protocol::user_input::UserInput;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tracing::{error, warn};

use crate::config::OpenBotConfig;
use crate::memory::MemoryStore;
use crate::prompt::build_prompt;
use crate::skills::load_skills;

/// Run the main agent loop, optionally resuming a previous session.
pub async fn run(config: OpenBotConfig, resume_session: Option<String>) -> Result<()> {
    let skill_dirs = config.resolved_skill_dirs();
    let skills = load_skills(&skill_dirs).unwrap_or_else(|e| {
        warn!("failed to load skills: {e}");
        Vec::new()
    });

    if !skills.is_empty() {
        eprintln!("Loaded {} skill(s)", skills.len());
        for skill in &skills {
            eprintln!("  - {}: {}", skill.name, skill.description);
        }
    }

    let mut memory = MemoryStore::load(&config.memory_path).with_context(|| "loading memory")?;
    eprintln!(
        "Memory: {} entries, {} history records",
        memory.memory.entries.len(),
        memory.memory.history.len()
    );

    let _codex_home = find_codex_home().with_context(|| "finding codex home")?;

    let sandbox_mode = config.sandbox_mode();
    let approval_policy = match sandbox_mode {
        SandboxMode::DangerFullAccess => Some(AskForApproval::Never),
        _ => Some(AskForApproval::Never),
    };

    let overrides = ConfigOverrides {
        model: config.model.clone(),
        review_model: None,
        config_profile: None,
        approval_policy,
        sandbox_mode: Some(sandbox_mode),
        cwd: None,
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

    if !config.skip_git_check {
        let cwd = codex_config.cwd.to_path_buf();
        if codex_core::git_info::get_git_repo_root(&cwd).is_none() {
            anyhow::bail!("Not inside a git repository. Use --skip-git-check to run anyway.");
        }
    }

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
                    .resume_thread_from_rollout(
                        codex_config.clone(),
                        path,
                        auth_manager.clone(),
                    )
                    .await
                    .with_context(|| "resuming session")?
            }
            None => {
                eprintln!("Session {session_id} not found, starting new session.");
                thread_manager
                    .start_thread(codex_config.clone())
                    .await
                    .with_context(|| "starting codex thread")?
            }
        }
    } else {
        thread_manager
            .start_thread(codex_config.clone())
            .await
            .with_context(|| "starting codex thread")?
    };

    let session_id = session_configured.session_id.to_string();
    eprintln!("Session {} (model: {})", &session_id, &session_configured.model);

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

    let max_iterations = config.max_iterations;
    let stop_phrase = config
        .stop_phrase
        .clone()
        .unwrap_or_else(|| "TASK COMPLETE".into());
    let sleep_duration = Duration::from_secs(config.sleep_secs);

    let stdin = tokio::io::stdin();
    let mut stdin_reader = BufReader::new(stdin).lines();

    let iteration_limit = if max_iterations == 0 {
        u32::MAX
    } else {
        max_iterations
    };

    'outer: for iteration in 1..=iteration_limit {
        // Check if ctrl-c was pressed before starting a new iteration.
        if *shutdown_rx.borrow() {
            break;
        }

        eprintln!(
            "\n--- Iteration {}/{} ---",
            iteration,
            if max_iterations == 0 {
                "∞".to_string()
            } else {
                max_iterations.to_string()
            }
        );

        let prompt = build_prompt(
            &config.instructions,
            &skills,
            &memory,
            iteration,
            max_iterations,
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
                EventMsg::TurnComplete(_) => {
                    break;
                }
                EventMsg::TurnAborted(_) => {
                    // Turn was interrupted (e.g. by ctrl-c).
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

        // Save iteration results.
        let prompt_summary = truncate_string(&config.instructions, 100);
        let response_summary = truncate_string(&last_message, 500);
        memory.add_iteration(iteration, &prompt_summary, &response_summary);
        memory.save().with_context(|| "saving memory")?;

        if *shutdown_rx.borrow() {
            break;
        }

        if last_message.contains(&stop_phrase) {
            eprintln!("\nAgent signaled completion: \"{stop_phrase}\"");
            break;
        }

        if iteration >= iteration_limit {
            break;
        }

        // Sleep between iterations, but wake on user input or ctrl-c.
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
                            eprintln!("User input received, injecting into next iteration.");
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
    eprintln!("Completed {} iteration(s)", total);
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

fn truncate_string(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
