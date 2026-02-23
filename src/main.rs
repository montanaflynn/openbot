//! CLI entry point for OpenBot.
//!
//! This module is intentionally thin: it parses command-line arguments, loads
//! configuration, and delegates behavior to feature modules.

mod config;
mod memory;
mod prompt;
mod runner;
mod skills;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Top-level CLI parser.
#[derive(Parser)]
#[command(name = "openbot", about = "AI agent loop powered by codex-core")]
struct Cli {
    /// Selected subcommand.
    #[command(subcommand)]
    command: Commands,
}

/// Supported CLI subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Run the agent loop
    Run {
        /// The prompt/instructions for the agent
        #[arg(short, long)]
        prompt: Option<String>,

        /// Maximum number of iterations (0 = unlimited)
        #[arg(short = 'n', long, default_value = "10")]
        max_iterations: u32,

        /// Model to use (e.g. o4-mini, gpt-4.1)
        #[arg(short, long)]
        model: Option<String>,

        /// Allow running outside git repositories
        #[arg(long)]
        skip_git_check: bool,

        /// Seconds to sleep between iterations (overrides config)
        #[arg(short, long)]
        sleep: Option<u64>,

        /// Resume a previous session by ID
        #[arg(long)]
        resume: Option<String>,
    },

    /// List available skills
    Skills,

    /// Manage persistent memory
    Memory {
        /// Memory management operation.
        #[command(subcommand)]
        action: MemoryAction,
    },
}

/// Actions for the `memory` management subcommand.
#[derive(Subcommand)]
enum MemoryAction {
    /// Show all memory entries and history
    Show,
    /// Set a key-value pair
    Set {
        /// The key
        key: String,
        /// The value
        value: String,
    },
    /// Remove a key
    Remove {
        /// The key to remove
        key: String,
    },
    /// Clear all memory
    Clear,
}

/// Program entrypoint.
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging/tracing. We default to `error` level unless
    // overridden via tracing environment variables.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    // Route execution to the selected command.
    match cli.command {
        Commands::Run {
            prompt,
            max_iterations,
            model,
            skip_git_check,
            sleep,
            resume,
        } => {
            let config = config::OpenBotConfig::load(&cwd)?.with_overrides(
                prompt,
                Some(max_iterations),
                model,
                skip_git_check,
                sleep,
            );

            runner::run(config, resume).await?;
        }

        Commands::Skills => {
            // Resolve configured skill directories and print discovered skills.
            let config = config::OpenBotConfig::load(&cwd)?;
            let skill_dirs = config.resolved_skill_dirs();
            let skills = skills::load_skills(&skill_dirs)?;

            if skills.is_empty() {
                println!("No skills found.");
                println!("Skill directories searched:");
                for dir in &skill_dirs {
                    println!("  {}", dir.display());
                }
            } else {
                println!("Available skills ({}):\n", skills.len());
                for skill in &skills {
                    println!("  {} - {}", skill.name, skill.description);
                    println!("    source: {}", skill.source_path);
                }
            }
        }

        Commands::Memory { action } => {
            // Load memory store and perform the requested management action.
            let config = config::OpenBotConfig::load(&cwd)?;
            let mut store = memory::MemoryStore::load(&config.memory_path)?;

            match action {
                MemoryAction::Show => {
                    print!("{}", store.display());
                }
                MemoryAction::Set { key, value } => {
                    store.set(key.clone(), value.clone());
                    store.save()?;
                    println!("Set {key} = {value}");
                }
                MemoryAction::Remove { key } => {
                    if store.remove(&key).is_some() {
                        store.save()?;
                        println!("Removed {key}");
                    } else {
                        println!("Key {key} not found");
                    }
                }
                MemoryAction::Clear => {
                    store.clear();
                    store.save()?;
                    println!("Memory cleared.");
                }
            }
        }
    }

    Ok(())
}
