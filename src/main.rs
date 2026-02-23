mod config;
mod memory;
mod prompt;
mod registry;
mod runner;
mod skills;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "openbot", about = "AI agent loop powered by codex-core")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a bot
    Run {
        /// Bot name
        #[arg(short, long)]
        bot: String,

        /// Override the bot's instructions
        #[arg(short, long)]
        prompt: Option<String>,

        /// Maximum number of iterations (0 = unlimited)
        #[arg(short = 'n', long)]
        max_iterations: Option<u32>,

        /// Model to use (e.g. o4-mini, gpt-4.1)
        #[arg(short, long)]
        model: Option<String>,

        /// Allow running outside git repositories
        #[arg(long)]
        skip_git_check: bool,

        /// Seconds to sleep between iterations
        #[arg(short, long)]
        sleep: Option<u64>,

        /// Resume a previous session by ID
        #[arg(long)]
        resume: Option<String>,
    },

    /// Manage bots
    #[command(subcommand)]
    Bots(BotsAction),

    /// Manage skills (list, search, install, remove)
    #[command(subcommand)]
    Skills(SkillsAction),

    /// Manage a bot's memory
    Memory {
        /// Bot name
        bot: String,

        #[command(subcommand)]
        action: MemoryAction,
    },
}

#[derive(Subcommand)]
enum BotsAction {
    /// List all bots
    List,
    /// Create a new bot
    Create {
        /// Bot name
        name: String,
        /// Short description of the bot
        #[arg(short, long)]
        description: Option<String>,
        /// Initial instructions for the bot
        #[arg(short, long)]
        prompt: Option<String>,
    },
    /// Show a bot's config and status
    Show {
        /// Bot name
        name: String,
    },
}

#[derive(Subcommand)]
enum SkillsAction {
    /// List skills for a bot
    List {
        /// Bot name
        bot: String,
    },
    /// Search the skills.sh registry
    Search {
        /// Search query
        query: String,
        /// Maximum number of results
        #[arg(short, long, default_value = "10")]
        limit: u32,
    },
    /// Install a skill from the skills.sh registry
    Install {
        /// Skill identifier (owner/repo/skill-name)
        skill: String,
        /// Install globally (~/.openbot/skills/)
        #[arg(short, long)]
        global: bool,
        /// Install for a specific bot
        #[arg(short, long)]
        bot: Option<String>,
    },
    /// Remove an installed skill
    Remove {
        /// Skill short name to remove
        name: String,
        /// Remove from global skills
        #[arg(short, long)]
        global: bool,
        /// Remove from a specific bot
        #[arg(short, long)]
        bot: Option<String>,
    },
}

#[derive(Subcommand)]
enum MemoryAction {
    /// Show all memory entries and history
    Show,
    /// Set a key-value pair
    Set { key: String, value: String },
    /// Remove a key
    Remove { key: String },
    /// Clear all memory
    Clear,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            bot,
            prompt,
            max_iterations,
            model,
            skip_git_check,
            sleep,
            resume,
        } => {
            // Ensure bot exists.
            config::ensure_global_dirs()?;
            config::ensure_bot_dirs(&bot)?;

            let cfg = config::BotConfig::load(&bot)?.with_overrides(
                prompt,
                max_iterations,
                model,
                skip_git_check,
                sleep,
            );

            runner::run(&bot, cfg, resume).await?;
        }

        Commands::Bots(action) => match action {
            BotsAction::List => {
                let bots = config::list_bots()?;
                if bots.is_empty() {
                    println!("No bots yet. Create one with: openbot bots create <name>");
                } else {
                    println!("Bots:\n");
                    for name in &bots {
                        let cfg = config::BotConfig::load(name).unwrap_or_default();
                        let mem_path = config::bot_memory_path(name)?;
                        let has_memory = mem_path.exists();
                        let skill_dir = config::bot_skills_dir(name)?;
                        let skill_count = if skill_dir.exists() {
                            std::fs::read_dir(&skill_dir)?
                                .filter(|e| {
                                    e.as_ref()
                                        .ok()
                                        .and_then(|e| e.path().extension().map(|x| x == "md"))
                                        .unwrap_or(false)
                                })
                                .count()
                        } else {
                            0
                        };
                        if cfg.description.is_empty() {
                            println!(
                                "  {name}  ({skill_count} skills, {})",
                                if has_memory { "has memory" } else { "no memory" }
                            );
                        } else {
                            println!(
                                "  {name} - {}  ({skill_count} skills, {})",
                                cfg.description,
                                if has_memory { "has memory" } else { "no memory" }
                            );
                        }
                    }
                }
            }
            BotsAction::Create { name, description, prompt } => {
                config::ensure_global_dirs()?;
                config::ensure_bot_dirs(&name)?;

                let mut cfg = config::BotConfig::default();
                if let Some(desc) = description {
                    cfg.description = desc;
                }
                if let Some(instructions) = prompt {
                    cfg.instructions = instructions;
                }

                let config_path = config::bot_config_path(&name)?;
                std::fs::write(&config_path, config::serialize_config_md(&cfg))?;

                let bot_dir = config::bot_dir(&name)?;
                println!("Created bot '{name}' at {}", bot_dir.display());
            }
            BotsAction::Show { name } => {
                let dir = config::bot_dir(&name)?;
                if !dir.exists() {
                    println!("Bot '{name}' does not exist.");
                    return Ok(());
                }
                let cfg = config::BotConfig::load(&name)?;
                println!("Bot: {name}");
                if !cfg.description.is_empty() {
                    println!("  Description: {}", cfg.description);
                }
                println!("  Directory: {}", dir.display());
                println!("  Instructions: {}", truncate(&cfg.instructions, 80));
                println!("  Max iterations: {}", cfg.max_iterations);
                println!("  Sleep: {}s", cfg.sleep_secs);
                println!("  Sandbox: {}", cfg.sandbox);
                if let Some(ref model) = cfg.model {
                    println!("  Model: {model}");
                }

                let mem_path = config::bot_memory_path(&name)?;
                if mem_path.exists() {
                    let store = memory::MemoryStore::load(&mem_path)?;
                    println!(
                        "  Memory: {} entries, {} history records",
                        store.memory.entries.len(),
                        store.memory.history.len()
                    );
                }

                let skill_dirs = config::BotConfig::skill_dirs(&name)?;
                let skills = skills::load_skills(&skill_dirs)?;
                if !skills.is_empty() {
                    println!("  Skills:");
                    for skill in &skills {
                        println!("    - {}: {}", skill.name, skill.description);
                    }
                }
            }
        },

        Commands::Skills(action) => match action {
            SkillsAction::List { bot } => {
                let skill_dirs = config::BotConfig::skill_dirs(&bot)?;
                let skills = skills::load_skills(&skill_dirs)?;

                if skills.is_empty() {
                    println!("No skills found for bot '{bot}'.");
                    println!("Skill directories:");
                    for dir in &skill_dirs {
                        println!("  {}", dir.display());
                    }
                } else {
                    println!("Skills for '{bot}' ({}):\n", skills.len());
                    for skill in &skills {
                        println!("  {} - {}", skill.name, skill.description);
                        println!("    source: {}", skill.source_path);
                    }
                }
            }
            SkillsAction::Search { query, limit } => {
                let results = registry::search(&query, limit).await?;

                if results.skills.is_empty() {
                    println!("No skills found for '{query}'.");
                } else {
                    println!(
                        "Found {} skill{} for '{query}':\n",
                        results.count,
                        if results.count == 1 { "" } else { "s" }
                    );
                    for skill in &results.skills {
                        println!("  {} - {}", skill.id, skill.name);
                        println!(
                            "    source: {}  installs: {}",
                            skill.source, skill.installs
                        );
                    }
                    println!("\nInstall with: openbot skills install <id> --global");
                }
            }
            SkillsAction::Install { skill, global, bot } => {
                let (source, skill_id) = parse_skill_identifier(&skill)?;

                let (skill_dir, manifest_path) = if global {
                    config::ensure_global_dirs()?;
                    (
                        config::global_skills_dir()?,
                        config::global_skills_manifest_path()?,
                    )
                } else if let Some(ref bot_name) = bot {
                    config::ensure_bot_dirs(bot_name)?;
                    (
                        config::bot_skills_dir(bot_name)?,
                        config::bot_skills_manifest_path(bot_name)?,
                    )
                } else {
                    anyhow::bail!("specify --global or --bot <name>");
                };

                println!("Fetching {skill_id} from {source}...");
                let content = registry::fetch_skill_md(&source, &skill_id).await?;

                skills::install_skill(
                    &skill_dir,
                    &manifest_path,
                    &skill_id,
                    &source,
                    &skill,
                    &content,
                )?;

                let scope = if global {
                    "global".to_string()
                } else {
                    format!("bot '{}'", bot.unwrap())
                };
                println!("Installed skill '{skill_id}' ({scope}).");
            }
            SkillsAction::Remove { name, global, bot } => {
                let (skill_dir, manifest_path) = if global {
                    (
                        config::global_skills_dir()?,
                        config::global_skills_manifest_path()?,
                    )
                } else if let Some(ref bot_name) = bot {
                    (
                        config::bot_skills_dir(bot_name)?,
                        config::bot_skills_manifest_path(bot_name)?,
                    )
                } else {
                    anyhow::bail!("specify --global or --bot <name>");
                };

                if skills::remove_skill(&skill_dir, &manifest_path, &name)? {
                    println!("Removed skill '{name}'.");
                } else {
                    println!("Skill '{name}' not found.");
                }
            }
        },

        Commands::Memory { bot, action } => {
            let mem_path = config::BotConfig::memory_path(&bot)?;
            let mut store = memory::MemoryStore::load(&mem_path)?;

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

/// Parse a skill identifier like "owner/repo/skill-name" into (source, skill_id).
///
/// Examples:
/// - "obra/superpowers/brainstorming" → ("obra/superpowers", "brainstorming")
/// - "user/repo/my-skill"            → ("user/repo", "my-skill")
fn parse_skill_identifier(id: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = id.splitn(3, '/').collect();
    if parts.len() != 3 {
        anyhow::bail!(
            "invalid skill identifier '{id}': expected format owner/repo/skill-name"
        );
    }
    let source = format!("{}/{}", parts[0], parts[1]);
    let skill_id = parts[2].to_string();
    Ok((source, skill_id))
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
