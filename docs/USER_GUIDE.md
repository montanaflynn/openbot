# User Guide

This guide covers everything you need to get started with openbot and make the most of its features.

## Table of Contents

- [Overview](#overview)
- [Installation](#installation)
- [Creating Your First Bot](#creating-your-first-bot)
- [Running a Bot](#running-a-bot)
- [Bot Configuration](#bot-configuration)
- [Worktree Isolation](#worktree-isolation)
- [Skills](#skills)
- [Memory](#memory)
- [Session History](#session-history)
- [Project Workspaces](#project-workspaces)
- [Running Multiple Bots](#running-multiple-bots)
- [Resuming Sessions](#resuming-sessions)
- [Interrupting and Recovering](#interrupting-and-recovering)
- [Tips and Patterns](#tips-and-patterns)

## Overview

openbot is a CLI tool that runs autonomous AI agents in a loop. You create named bots with their own instructions, skills, and memory, then point them at a project. Each bot gets an isolated git worktree, executes commands, writes code, and persists what it learned for next time.

The runtime is powered by OpenAI's [Codex](https://github.com/openai/codex) engine. openbot handles the orchestration: prompt assembly, event streaming, session history, memory persistence, worktree lifecycle, and multi-bot concurrency.

## Installation

### Prerequisites

- **Rust** (edition 2024, rustc 1.85+)
- **Git** (for worktree isolation)
- **OpenAI API key** set as `OPENAI_API_KEY`, or authenticated via `codex login`

### Build from source

```sh
git clone https://github.com/montanaflynn/openbot
cd openbot
git submodule update --init --recursive

# Development build
cargo build

# Install to PATH
cargo install --path .
```

Verify with:

```sh
openbot --help
```

## Creating Your First Bot

Create a bot with `openbot bots create`:

```sh
openbot bots create mybot \
  --description "General-purpose coding assistant" \
  --prompt "You are a coding assistant. Fix bugs, add tests, and improve code quality."
```

This creates a directory at `~/.openbot/bots/mybot/` with a `config.md` file containing your description and instructions.

You can also create a minimal bot and edit the config file directly:

```sh
openbot bots create mybot
```

Then edit `~/.openbot/bots/mybot/config.md`:

```markdown
+++
description = "General-purpose coding assistant"
max_iterations = 5
sleep_secs = 10
model = "5.3-codex"
sandbox = "workspace-write"
+++

You are a coding assistant working on this project.

Your priorities:
1. Fix any failing tests
2. Address TODO comments in the code
3. Improve test coverage

Be thorough. Run tests after every change to confirm nothing is broken.
```

List your bots:

```sh
openbot bots list
```

Inspect a bot's config, skills, and memory stats:

```sh
openbot bots show mybot
```

## Running a Bot

Navigate to a git repository and run:

```sh
openbot run -b mybot
```

The bot will:

1. Create an isolated git worktree with a new branch
2. Load its config, skills, and memory
3. Build a prompt and submit it to the AI model
4. Stream output and execute commands autonomously
5. Repeat until it calls `session_complete` or hits the iteration limit
6. Save session history and clean up the worktree

### CLI options

```sh
openbot run -b mybot                     # Run with defaults
openbot run -b mybot -n 3                # Max 3 iterations
openbot run -b mybot -n 0                # Unlimited iterations
openbot run -b mybot -m 5.3-codex          # Use a specific model
openbot run -b mybot -s 60               # 60-second sleep between iterations
openbot run -b mybot -p "Fix the login bug"  # Override instructions
openbot run -b mybot --no-worktree       # Run in the current working tree
openbot run -b mybot --skip-git-check    # Run outside a git repo
openbot run -b mybot --project my-app    # Target a specific workspace
openbot run -b mybot --resume <ID>       # Resume a previous session
```

### What you see during a run

```
## Session 1

Model:     5.3-codex
Workspace: my-project
Branch:    openbot/mybot-1708800000
Skills:    3
Memory:    2 entries
History:   0 sessions

### Output

[streamed AI output and commands appear here]
  $ cargo test
  $ git add -A
  $ git commit -m "fix: resolve login bug"

### Summary

Result:    Fixed the login validation bug and added regression test
Action:    merged openbot/mybot-1708800000 into main
Duration:  47s
Tokens:    12345 input (8000 cached) / 3456 output (200 reasoning)
Resume:    openbot run --resume abc123
```

## Bot Configuration

Each bot's configuration lives in `~/.openbot/bots/<name>/config.md`. The file uses TOML frontmatter (delimited by `+++`) with a markdown body for instructions.

### Config fields

| Field | Default | Description |
|-------|---------|-------------|
| `description` | (empty) | Short description shown in `bots list` |
| `max_iterations` | `10` | Max iterations per run (`0` = unlimited) |
| `sleep_secs` | `30` | Seconds between iterations (`0` = no sleep) |
| `model` | (codex default) | Model override (e.g. `5.3-codex`, `o3`) |
| `sandbox` | `"workspace-write"` | Sandbox mode (see below) |
| `skip_git_check` | `false` | Allow running outside git repos |

### Sandbox modes

- **`read-only`** -- the bot can read files but not write or execute destructive commands.
- **`workspace-write`** -- the bot can read and write files within the project (default).
- **`danger-full-access`** -- no restrictions. Use with caution.

### Override precedence

1. Built-in defaults
2. Values from `config.md` frontmatter
3. CLI flags for the current run

CLI flags always win. For example, `-n 3` overrides whatever `max_iterations` is set in the config.

## Worktree Isolation

By default, every `openbot run` creates a temporary git worktree on a new branch (`openbot/<bot>-<timestamp>`). This means:

- The bot works on an isolated copy of the repo
- Your working tree is untouched
- Multiple bots can run on the same repo concurrently
- If the bot breaks something, your main branch is safe

### Session completion actions

When the bot finishes, it calls the `session_complete` tool with an action:

- **`merge`** -- fast-forward merges the bot's branch into the base branch
- **`review`** -- leaves the branch for you to inspect manually
- **`discard`** -- drops the changes (branch is still kept)

After the run, the worktree directory is removed but the branch is always preserved so no commits are lost.

### Reviewing a bot's work

If the bot chose `review`:

```sh
# See what the bot did
git log main..openbot/mybot-1708800000

# Diff against main
git diff main..openbot/mybot-1708800000

# Merge when satisfied
git merge openbot/mybot-1708800000
```

### Opting out

Use `--no-worktree` to run directly in the current working tree:

```sh
openbot run -b mybot --no-worktree
```

Use `--skip-git-check` to run outside a git repository entirely:

```sh
openbot run -b mybot --skip-git-check
```

## Skills

Skills are markdown files that get injected into the bot's prompt, giving it specialized knowledge, procedures, or constraints.

### Skill format

```markdown
---
name: code-review
description: Review code for bugs and style issues
---
When reviewing code, follow these steps:
1. Read each file thoroughly before commenting
2. Check for bugs, logic errors, and edge cases
3. Look for security issues
4. Provide specific, actionable feedback with file paths and line references
```

The YAML frontmatter (`name`, `description`) is optional. Without it, the filename is used as the skill name.

### Skill locations

- **Global skills** (`~/.openbot/skills/`) -- available to every bot
- **Bot-local skills** (`~/.openbot/bots/<name>/skills/`) -- available to one bot only

Bot-local skills take precedence when there's a name conflict.

### Installing from the registry

Search for published skills:

```sh
openbot skills search "code review"
```

Install a skill:

```sh
openbot skills install obra/superpowers/brainstorming --bot mybot
openbot skills install obra/superpowers/refactor --global
```

List installed skills:

```sh
openbot skills list mybot
```

Remove a skill:

```sh
openbot skills remove brainstorming --bot mybot
```

### Runtime skill creation

Bots can create their own skills during a session. The prompt tells the bot where its skill directory is, and any `.md` files it writes there will be loaded on the next iteration. This allows bots to accumulate reusable procedures over time.

### Writing effective skills

A good skill:

- Has a clear, focused purpose (one procedure per skill)
- Includes step-by-step instructions the agent can follow
- Specifies when the skill should be used
- Provides examples when helpful

## Memory

Each bot has a per-project key-value memory store that persists across runs. Memory entries are injected into the bot's prompt so it has context from previous sessions.

### CLI management

```sh
# View all memory entries
openbot memory mybot --project my-project show

# Set a value
openbot memory mybot --project my-project set project_goal "migrate to PostgreSQL"

# Remove a key
openbot memory mybot --project my-project remove project_goal

# Clear everything
openbot memory mybot --project my-project clear
```

### Seeding context before a run

You can pre-load memory entries to give the bot context:

```sh
openbot memory mybot --project my-project set constraints "must maintain backward compatibility"
openbot memory mybot --project my-project set priority "fix the payment processing bug first"
openbot run -b mybot
```

The bot sees these entries in its prompt and can act on them.

### Runtime memory updates

During the sleep window between iterations, you can type text into stdin. That text is saved as a `user_input` memory entry and injected into the next iteration's prompt. This lets you steer the bot without stopping it:

```
  Sleeping 30s (type to wake)...
focus on the authentication module next
User input received, injecting into next session.
```

### How memory is used in prompts

Each iteration's prompt includes:

- All current memory entries as a key-value list
- The last 5 session history summaries for continuity

This keeps the bot aware of what happened previously without overwhelming the context window.

## Session History

Every session is recorded to disk as events happen. If a session crashes mid-run, the events up to that point are preserved.

### Storage format

Each session is stored as a directory:

```
history/{session_id}/
  metadata.json    # Session-level summary (model, duration, tokens, etc.)
  events.jsonl     # Append-only event stream (messages, commands, token counts)
```

Events are flushed to disk immediately, so you never lose data on a crash.

### Browsing history from the CLI

List recent sessions:

```sh
openbot history mybot                    # Last 10 sessions
openbot history mybot --limit 50         # Last 50 sessions
openbot history mybot --project my-app   # Specific workspace
```

View a specific session:

```sh
openbot history mybot --session <SESSION_ID>
```

This prints the session metadata (model, duration, tokens, summary) followed by the commands executed and the full agent response reconstructed from the event stream.

### History in the agent's prompt

The bot can also access its own history during a session via the built-in `session_history` tool. It can:

- List all previous sessions with summaries
- View the full response and command log of any past session
- Page through long responses with offset/limit

This means bots can review their own past work and learn from it.

### Event types

The event stream (`events.jsonl`) contains three types of events:

- **`message`** -- chunks of the agent's text response, streamed as they arrive
- **`command`** -- a shell command that was executed, with exit code and duration
- **`token_count`** -- token usage snapshots (input, cached, output, reasoning)

## Project Workspaces

Memory and history are scoped per project. The project is identified by a slug derived from the directory name where you run the bot. For example, running in `/home/user/my-project` creates a workspace slug `my-project`.

All workspace data lives under:

```
~/.openbot/bots/<name>/workspaces/<slug>/
  memory.json
  history/
    <session_id>/
      metadata.json
      events.jsonl
```

### Specifying a workspace explicitly

Use `--project` to target a specific workspace:

```sh
openbot run -b mybot --project my-app
openbot history mybot --project my-app
openbot memory mybot --project my-app show
```

This is useful when you want to work on a project from a different directory or manage workspaces without being inside the project.

### Worktrees and workspace scoping

When running in a git worktree, the workspace is resolved from the original repo root (not the worktree path). This means all worktrees of the same repo share one workspace, so memory and history are consistent regardless of which worktree you're in.

## Running Multiple Bots

Because each bot gets its own worktree and branch, you can run multiple bots concurrently on the same project:

```sh
openbot run -b security-auditor &
openbot run -b test-writer &
openbot run -b refactorer &
```

Each bot operates on its own isolated branch. There are no file conflicts. When they finish, you can review and merge their branches independently.

### Different bots for different tasks

A common pattern is to create specialized bots:

```sh
openbot bots create security-bot -d "Security auditor" -p "Audit for OWASP top 10 vulnerabilities"
openbot bots create test-bot -d "Test writer" -p "Add missing unit tests for uncovered code"
openbot bots create docs-bot -d "Documentation" -p "Update docs to match current code"
```

Each bot accumulates its own skills and memory, becoming more effective over time at its specific task.

## Resuming Sessions

Every run prints a resume command at the end:

```
Resume:    openbot run --resume abc123def
```

Use this to continue a session where it left off:

```sh
openbot run -b mybot --resume abc123def
```

This reconnects to the same Codex session (if it's still available) so the agent retains full context from the previous run.

## Interrupting and Recovering

### Ctrl-C

Pressing Ctrl-C triggers a graceful shutdown:

1. The main loop stops
2. Any in-flight Codex operations are interrupted
3. Session history is finalized and saved
4. The worktree is cleaned up
5. A resume command is printed

Because events are streamed to disk as they happen, even if the shutdown isn't perfectly clean, the `events.jsonl` file contains everything up to the point of interruption.

### Crash recovery

If openbot crashes or is killed (e.g. `kill -9`):

- **Session events** are preserved in `events.jsonl` (flushed after each event)
- **The worktree branch** still exists in git (the branch is never deleted)
- **Memory** from previous sessions is intact (only the current session's memory updates may be lost)

To find and clean up orphaned worktrees:

```sh
git worktree list
git worktree remove .git/openbot-worktrees/<suffix>
```

## Tips and Patterns

### Start small

Begin with a low iteration count and review the bot's work before scaling up:

```sh
openbot run -b mybot -n 1
```

Once you're comfortable with what the bot does, increase iterations or set to unlimited.

### Use specific instructions

Vague instructions produce vague results. Be specific:

```markdown
# Bad
Fix bugs in the code.

# Good
Run the test suite with `cargo test`. Fix any failing tests. After each fix, run
the tests again to confirm the fix works and doesn't break other tests. Focus on
the authentication module first.
```

### Seed memory for context

Before a run, give the bot useful context:

```sh
openbot memory mybot --project my-app set recent_changes "migrated auth from JWT to sessions in v2.3"
openbot memory mybot --project my-app set known_issues "flaky test in test_concurrent_login"
```

### Review history to understand bot behavior

Use history to see what the bot did across sessions:

```sh
openbot history mybot
openbot history mybot --session <id>
```

This helps you refine instructions and skills based on actual behavior.

### Build skills incrementally

Start without skills. Watch what the bot does well and where it struggles. Then create skills to address the gaps:

```sh
# Bot keeps forgetting to run tests? Create a skill:
cat > ~/.openbot/bots/mybot/skills/always-test.md << 'EOF'
---
name: always-test
description: Always run tests after making changes
---
After every code change, run the project's test suite to verify nothing is broken.
If tests fail, fix the issue before moving on. Never commit code with failing tests.
EOF
```

### Use the sleep window for steering

During multi-iteration runs, the bot sleeps between iterations and listens for stdin input. Type a message to redirect the bot:

```
  Sleeping 30s (type to wake)...
stop working on auth, focus on the API rate limiting instead
```

This injects your message into the bot's memory for the next iteration.

## Debug Logging

Set the `RUST_LOG` environment variable to see internal activity:

```sh
RUST_LOG=info openbot run -b mybot -n 1    # Basic info
RUST_LOG=debug openbot run -b mybot -n 1   # Detailed debug output
```

## Data Portability

All openbot data lives under `~/.openbot/`. The directory contains no absolute paths, so you can copy it to another machine:

```sh
rsync -a ~/.openbot/ remote:~/.openbot/
```

This transfers all bots, skills, memory, and history.
