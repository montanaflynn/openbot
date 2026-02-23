//! Core execution loop for OpenBot.
//!
//! This module wires configuration, Codex session management, prompt assembly,
//! streaming event handling, memory persistence, and iteration control.

use anyhow::{Context, Result};
use codex_core::config::{ConfigBuilder, ConfigOverrides, find_codex_home};
use codex_core::{AuthManager, ThreadManager};
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::{AskForApproval, EventMsg, Op, SessionSource};
use codex_protocol::user_input::UserInput;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{error, warn};

use crate::config::OpenBotConfig;
use crate::memory::MemoryStore;
use crate::prompt::build_prompt;
use crate::skills::load_skills;

/// Run the main agent loop.
///
/// Steps performed:
/// 1. Load skills and memory
/// 2. Build Codex runtime configuration and start a session
/// 3. Repeatedly build prompts and submit user turns
/// 4. Stream events, persist summaries, and stop on completion signals
pub async fn run(config: OpenBotConfig) -> Result<()> {
    // Load all configured skills. Failures are downgraded to warnings so the
    // main run can still proceed.
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

    // Load persisted memory (or initialize empty state).
    let mut memory = MemoryStore::load(&config.memory_path).with_context(|| "loading memory")?;
    eprintln!(
        "Memory: {} entries, {} history records",
        memory.memory.entries.len(),
        memory.memory.history.len()
    );

    // Resolve Codex home early so startup errors are immediate and explicit.
    let _codex_home = find_codex_home().with_context(|| "finding codex home")?;

    let sandbox_mode = config.sandbox_mode();

    // Approval policy is currently fixed to `Never` in all sandbox modes.
    // Keeping the match makes future policy differentiation straightforward.
    let approval_policy = match sandbox_mode {
        SandboxMode::DangerFullAccess => Some(AskForApproval::Never),
        _ => Some(AskForApproval::Never),
    };

    // Build harness overrides to inject runtime-level settings.
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

    // Construct final Codex runtime configuration.
    let codex_config = ConfigBuilder::default()
        .harness_overrides(overrides)
        .build()
        .await
        .with_context(|| "building codex config")?;

    // Enforce git repository requirement unless explicitly disabled.
    if !config.skip_git_check {
        let cwd = codex_config.cwd.to_path_buf();
        if codex_core::git_info::get_git_repo_root(&cwd).is_none() {
            anyhow::bail!("Not inside a git repository. Use --skip-git-check to run anyway.");
        }
    }

    // Create shared auth and thread managers used to run Codex sessions.
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

    // Start a new thread/session.
    let codex_core::NewThread {
        thread_id: _,
        thread,
        session_configured,
    } = thread_manager
        .start_thread(codex_config.clone())
        .await
        .with_context(|| "starting codex thread")?;

    eprintln!("Session started (model: {})", &session_configured.model);

    // Cache immutable defaults reused for each submitted turn.
    let default_cwd = codex_config.cwd.to_path_buf();
    let default_approval_policy = codex_config.permissions.approval_policy.value();
    let default_sandbox_policy = codex_config.permissions.sandbox_policy.get();
    let default_effort = codex_config.model_reasoning_effort;
    let default_summary = codex_config.model_reasoning_summary;

    // Resolve model once so repeated turns do not re-fetch default selection.
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

    // Set up stdin reader used to interrupt sleep and inject ad-hoc user input.
    let stdin = tokio::io::stdin();
    let mut stdin_reader = BufReader::new(stdin).lines();

    // `0` means unlimited iterations. Internally represent that with `u32::MAX`.
    let iteration_limit = if max_iterations == 0 {
        u32::MAX
    } else {
        max_iterations
    };

    for iteration in 1..=iteration_limit {
        eprintln!(
            "\n--- Iteration {}/{} ---",
            iteration,
            if max_iterations == 0 {
                "∞".to_string()
            } else {
                max_iterations.to_string()
            }
        );

        // Build fresh prompt for this iteration using latest memory state.
        let prompt = build_prompt(
            &config.instructions,
            &skills,
            &memory,
            iteration,
            max_iterations,
        );

        // Submit prompt as a user turn to the active thread.
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

        // Stream events until turn completion.
        let mut last_message = String::new();

        loop {
            let event = thread
                .next_event()
                .await
                .with_context(|| "receiving event")?;

            match &event.msg {
                EventMsg::AgentMessage(msg) => {
                    // Full message snapshots can appear independently of deltas.
                    if !msg.message.is_empty() {
                        println!("{}", msg.message);
                        last_message = msg.message.clone();
                    }
                }
                EventMsg::AgentMessageDelta(delta) => {
                    // Stream incremental output as it arrives.
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
                EventMsg::Error(e) => {
                    error!("Error from codex: {:?}", e);
                    break;
                }
                EventMsg::ExecApprovalRequest(req) => {
                    // Auto-approve command requests in this autonomous run mode.
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

        // Persist compact summaries from this completed iteration.
        let prompt_summary = truncate_string(&config.instructions, 100);
        let response_summary = truncate_string(&last_message, 500);
        memory.add_iteration(iteration, &prompt_summary, &response_summary);
        memory.save().with_context(|| "saving memory")?;

        // Stop early when the configured completion phrase appears.
        if last_message.contains(&stop_phrase) {
            eprintln!("\nAgent signaled completion: \"{stop_phrase}\"");
            break;
        }

        // Skip sleep handling after the final planned iteration.
        if iteration >= iteration_limit {
            break;
        }

        // Sleep between iterations, but allow input to wake and influence next turn.
        if config.sleep_secs > 0 {
            eprintln!(
                "Sleeping {} seconds (type to wake and inject input)...",
                config.sleep_secs
            );

            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {
                    // Normal wake after timeout.
                }
                line = stdin_reader.next_line() => {
                    match line {
                        Ok(Some(input)) if !input.trim().is_empty() => {
                            eprintln!("User input received, injecting into next iteration.");
                            // Store transient user input so the next prompt can consume it.
                            memory.set("user_input".into(), input);
                            memory.save().ok();
                        }
                        Ok(None) => {
                            // End-of-file on stdin; continue unattended.
                            eprintln!("stdin closed, continuing.");
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Print summary of the run.
    let total = memory.memory.history.len();
    eprintln!("\n--- Summary ---");
    eprintln!("Completed {} iteration(s)", total);
    if let Some(last) = memory.memory.history.last() {
        eprintln!("Last response: {}", truncate_string(&last.response_summary, 200));
    }

    // Request orderly shutdown with a timeout so we never hang.
    eprintln!("Shutting down...");
    thread.submit(Op::Shutdown).await.ok();

    let shutdown_timeout = Duration::from_secs(5);
    let _ = tokio::time::timeout(shutdown_timeout, async {
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

/// Truncate to at most `max` bytes, appending `...` when truncated.
fn truncate_string(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
