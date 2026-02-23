use crate::memory::MemoryStore;
use crate::skills::{Skill, format_skills_section};

/// Build the full prompt for an iteration, combining instructions, skills, memory, and context.
pub fn build_prompt(
    instructions: &str,
    skills: &[Skill],
    memory: &MemoryStore,
    iteration: u32,
    max_iterations: u32,
) -> String {
    let mut prompt = String::new();

    // Base instructions
    prompt.push_str(instructions);
    prompt.push_str("\n\n");

    // Iteration context
    if max_iterations > 0 {
        prompt.push_str(&format!(
            "## Iteration {iteration}/{max_iterations}\n\n"
        ));
    } else {
        prompt.push_str(&format!("## Iteration {iteration}\n\n"));
    }

    // Skills section
    let skills_section = format_skills_section(skills);
    if !skills_section.is_empty() {
        prompt.push_str(&skills_section);
        prompt.push('\n');
    }

    // Memory section
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
            // Show last 5 iterations to keep prompt manageable
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

    // Meta-instructions
    prompt.push_str("## Instructions\n");
    prompt.push_str("- Complete as much of the task as you can\n");
    prompt.push_str("- Report what you accomplished and what remains\n");
    prompt.push_str("- When fully done, say \"TASK COMPLETE\"\n");

    prompt
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
