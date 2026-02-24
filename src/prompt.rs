//! Prompt construction utilities used to build each autonomous session input.

use std::path::Path;

use crate::history::SessionRecord;
use crate::memory::MemoryStore;
use crate::skills::{Skill, format_skills_section};

/// Build the full prompt for one session.
///
/// `worktree_info` is `Some((branch, base_branch))` when the bot is running
/// in an isolated git worktree.
///
/// `user_input` is text the user typed between sessions (during the sleep
/// phase) that should be addressed directly this session.
#[allow(clippy::too_many_arguments)]
pub fn build_prompt(
    instructions: &str,
    skills: &[Skill],
    memory: &MemoryStore,
    recent_history: &[SessionRecord],
    session_num: usize,
    bot_skill_dir: &Path,
    project_context: Option<&str>,
    worktree_info: Option<(&str, &str)>,
    user_input: Option<&str>,
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
            "- Branch: `{branch}` (based on `{base_branch}`)\n\
             - You are working in an isolated git worktree. Commit your changes on this branch.\n\
             - When you call `session_complete`, choose an action for your commits:\n\
             - `merge` — your branch gets merged into `{base_branch}`\n\
             - `review` — leave the branch for the user to review\n\
             - `discard` — drop the changes\n"
        ));
    }
    prompt.push('\n');

    // Skills section.
    let skills_section = format_skills_section(skills);
    if !skills_section.is_empty() {
        prompt.push_str(&skills_section);
        prompt.push('\n');
    }

    // Memory section (agent's own key-value store).
    if !memory.memory.entries.is_empty() {
        prompt.push_str("## Memory (from previous sessions)\n\n");
        for (k, v) in &memory.memory.entries {
            prompt.push_str(&format!("- **{k}**: {v}\n"));
        }
        prompt.push('\n');
    }

    // User input — the user typed this between sessions and it should be
    // treated as a direct instruction to address in this session.
    if let Some(input) = user_input {
        prompt.push_str("## User Input\n\n");
        prompt.push_str(
            "The user provided the following input. Address this directly in your response:\n\n",
        );
        prompt.push_str(&format!("> {input}\n\n"));
    }

    // Recent history section.
    if !recent_history.is_empty() {
        prompt.push_str("### Recent History\n");
        for record in recent_history {
            prompt.push_str(&format!(
                "- Session {}: {}\n",
                record.session_number,
                truncate(&record.response_summary, 200),
            ));
        }
        prompt.push('\n');
    }

    // Instructions.
    prompt.push_str("## Instructions\n");
    prompt.push_str(
        "You are a fully autonomous agent. Do not ask for human input — make decisions and act.\n",
    );
    prompt.push_str("Your goal is to ship working code: make changes, test them, and commit.\n\n");
    prompt.push_str("- Work through the task independently and make as much progress as you can\n");
    prompt.push_str("- When you are done, call the `session_complete` tool with a summary of what you accomplished\n");
    prompt.push_str(
        "- You can call the `session_history` tool to browse previous sessions in detail. \
         Use action='list' for an overview or action='view' with session_number to read \
         the full transcript and commands (shows the end first; increase offset to page backward).\n",
    );
    prompt.push_str(
        "- Do not stop and ask for clarification — use your best judgment and keep moving\n",
    );
    // Skills documentation.
    prompt.push_str(&format!(
        "\n## Skills System\n\n\
         Skills are reusable markdown workflows loaded into your prompt each session.\n\
         You currently have {} skill(s) loaded (listed above under \"Available Skills\" if any).\n\n\
         **Creating skills:** Write a markdown file to `{}/` with YAML frontmatter:\n\
         ```\n\
         ---\n\
         name: skill-name\n\
         description: What this skill does\n\
         ---\n\
         Step-by-step instructions, examples, and guidelines here.\n\
         ```\n\
         The skill will be loaded automatically in your next session.\n\n\
         **When to create a skill:** If you develop a reusable procedure, debugging technique,\n\
         or workflow pattern that would be useful across sessions, save it as a skill.\n",
        skills.len(),
        bot_skill_dir.display()
    ));

    prompt
}

/// Return a borrowed slice capped at max bytes for prompt summaries.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}
