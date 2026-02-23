use std::path::Path;

use crate::memory::MemoryStore;
use crate::skills::{Skill, format_skills_section};

/// Build the full prompt for one iteration.
///
/// `worktree_info` is `Some((branch, base_branch))` when the bot is running
/// in an isolated git worktree.
pub fn build_prompt(
    instructions: &str,
    skills: &[Skill],
    memory: &MemoryStore,
    iteration: u32,
    max_iterations: u32,
    bot_skill_dir: &Path,
    project_context: Option<&str>,
    worktree_info: Option<(&str, &str)>,
) -> String {
    let mut prompt = String::new();

    // Base task instructions.
    prompt.push_str(instructions);
    prompt.push_str("\n\n");

    // Iteration context.
    prompt.push_str("## Status\n");
    if let Some(project) = project_context {
        prompt.push_str(&format!("- Project: {project}\n"));
    }
    if max_iterations > 0 {
        let remaining = max_iterations.saturating_sub(iteration);
        prompt.push_str(&format!(
            "- Iteration: {iteration} of {max_iterations} ({remaining} remaining)\n"
        ));
        if remaining <= 2 && remaining > 0 {
            prompt.push_str("- **Running low on iterations** -- prioritize finishing or say TASK COMPLETE\n");
        } else if remaining == 0 {
            prompt.push_str("- **This is your last iteration** -- wrap up and report final status\n");
        }
    } else {
        prompt.push_str(&format!("- Iteration: {iteration} (unlimited)\n"));
    }
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
        prompt.push_str("## Memory (from previous iterations)\n\n");

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
                    "- Iteration {}: {}\n",
                    record.iteration,
                    truncate(&record.response_summary, 200),
                ));
            }
            prompt.push('\n');
        }
    }

    // Instructions.
    prompt.push_str("## Instructions\n");
    prompt.push_str("- Complete as much of the task as you can\n");
    prompt.push_str("- Report what you accomplished and what remains\n");
    prompt.push_str("- When fully done, say \"TASK COMPLETE\"\n");
    prompt.push_str(&format!(
        "- If you develop a reusable procedure, save it as a skill in `{}/` \
         (markdown with `name:` and `description:` frontmatter). \
         It will be available next iteration.\n",
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
