use std::path::Path;

use crate::memory::MemoryStore;
use crate::skills::{Skill, format_skills_section};

/// Build the full prompt for one session.
///
/// `worktree_info` is `Some((branch, base_branch))` when the bot is running
/// in an isolated git worktree.
pub fn build_prompt(
    instructions: &str,
    skills: &[Skill],
    memory: &MemoryStore,
    session_num: u32,
    bot_skill_dir: &Path,
    project_context: Option<&str>,
    worktree_info: Option<(&str, &str)>,
) -> String {
    let mut prompt = String::new();

    // Base task instructions.
    prompt.push_str(instructions);
    prompt.push_str("\n\n");

    // Session context.
    prompt.push_str("## Status\n");
    if let Some(project) = project_context {
        prompt.push_str(&format!("- Project: {project}\n"));
    }
    prompt.push_str(&format!("- Session: {session_num}\n"));
    if let Some((branch, base_branch)) = worktree_info {
        prompt.push_str(&format!(
            "- Branch: {branch} (worktree, based on {base_branch})\n\
             - You are working in an isolated git worktree. \
             Commit your changes and merge/push/PR as appropriate.\n"
        ));
    }
    prompt.push('\n');

    // Skills section.
    let skills_section = format_skills_section(skills);
    if !skills_section.is_empty() {
        prompt.push_str(&skills_section);
        prompt.push('\n');
    }

    // Memory section.
    if !memory.memory.entries.is_empty() || !memory.memory.history.is_empty() {
        prompt.push_str("## Memory (from previous sessions)\n\n");

        if !memory.memory.entries.is_empty() {
            for (k, v) in &memory.memory.entries {
                prompt.push_str(&format!("- **{k}**: {v}\n"));
            }
            prompt.push('\n');
        }

        if !memory.memory.history.is_empty() {
            prompt.push_str("### Recent History\n");
            let recent: Vec<_> = memory
                .memory
                .history
                .iter()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            for record in recent {
                prompt.push_str(&format!(
                    "- Session {}: {}\n",
                    record.iteration,
                    truncate(&record.response_summary, 200),
                ));
            }
            prompt.push('\n');
        }
    }

    // Instructions.
    prompt.push_str("## Instructions\n");
    prompt.push_str("You are a fully autonomous agent. Do not ask for human input — make decisions and act.\n");
    prompt.push_str("Your goal is to ship working code: make changes, test them, and commit.\n\n");
    prompt.push_str("- Work through the task independently and make as much progress as you can\n");
    prompt.push_str("- When you are done, call the `session_complete` tool with a summary of what you accomplished\n");
    prompt.push_str("- Do not stop and ask for clarification — use your best judgment and keep moving\n");
    prompt.push_str(&format!(
        "- If you develop a reusable procedure, save it as a skill in `{}/` \
         (markdown with `name:` and `description:` frontmatter). \
         It will be available in the next session.\n",
        bot_skill_dir.display()
    ));

    prompt
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
