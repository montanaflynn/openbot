# Architecture

This document explains the runtime architecture of `openbot` and how modules interact.

## Overview

`openbot` is a thin CLI wrapper over the Codex Rust runtime. It manages named bots, each with their own configuration, skills, and persistent memory. When you run a bot, it enters an iterative loop that:

1. Loads the bot's configuration, skills (global + local), and memory.
2. Builds a prompt for the current iteration.
3. Submits a `UserTurn` to Codex.
4. Streams events to disk (`events.jsonl`) as they arrive.
5. Finalizes session metadata (`metadata.json`) on completion.
6. Repeats until completion criteria are met.

## Directory Layout

```
~/.openbot/
├── skills/                    # Global skills (all bots)
└── bots/
    └── <name>/
        ├── config.md          # Bot config (TOML frontmatter + markdown body)
        ├── skills/            # Bot-local skills
        └── workspaces/        # Per-project data
            └── <slug>/        # Slug derived from directory name
                ├── memory.json
                └── history/
                    └── <session_id>/
                        ├── metadata.json   # Session-level summary
                        └── events.jsonl    # Append-only event stream
```

## Module Map

- `src/main.rs`
  - CLI entry point (clap).
  - Parses arguments and dispatches to subcommands (`run`, `bots`, `skills`, `memory`).

- `src/config.rs`
  - Defines `BotConfig` and path helpers for `~/.openbot/`.
  - Loads bot config from `~/.openbot/bots/<name>/config.md` (TOML frontmatter + markdown body).
  - Applies CLI overrides.
  - Resolves sandbox mode and skill directories (global + bot-local).

- `src/git.rs`
  - Git worktree lifecycle: create, remove, resolve repo root.
  - `create_worktree()` creates an isolated checkout on branch `openbot/<bot>-<ts>`.
  - `WorktreeGuard` (Drop-based) ensures cleanup on any exit path.
  - `resolve_repo_root()` uses `git rev-parse --show-toplevel` so worktrees of the same repo share one root.

- `src/skills.rs`
  - Loads `.md` skill files from configured directories.
  - Parses optional frontmatter (`name`, `description`).
  - Formats a prompt section containing loaded skills.

- `src/memory.rs`
  - Defines persistent memory model (key-value `entries`).
  - Handles JSON load/save (per-workspace at `~/.openbot/bots/<name>/workspaces/<slug>/memory.json`).
  - Provides CLI-friendly rendering for `openbot memory <bot> show`.

- `src/prompt.rs`
  - Assembles iteration prompt from instructions, skills, memory, and recent session history.
  - Includes session count, worktree branch context, and tool usage instructions.
  - Tells the agent where to save new skills.

- `src/history.rs`
  - Defines `SessionRecord` (metadata), `SessionEvent` (event stream), and `SessionWriter`.
  - `SessionWriter` creates a directory per session, writes `metadata.json` and streams events to `events.jsonl`.
  - Reader functions support both new directory format and legacy `.json` files.
  - Helpers: `load_events()`, `reconstruct_response()`, `extract_commands()`.

- `src/runner.rs`
  - Orchestrates the main agent loop.
  - Creates a git worktree for isolation (default) or runs in the working tree (`--no-worktree`).
  - Starts or resumes a Codex session/thread.
  - Creates a `SessionWriter` at session start and streams events to disk as they happen.
  - Submits turns, consumes event stream, and handles sleep/wake behavior.
  - Handles graceful ctrl-c shutdown and prints resume hint.

- `src/workspace.rs`
  - Project root detection and slug derivation.
  - Scopes memory per-project by deriving a slug from the directory name.

## Runtime Data Flow

1. `main` parses CLI and builds `BotConfig` with overrides.
2. `runner::run()` resolves the git repo root and creates a worktree (unless `--no-worktree`).
3. Codex config is built (with cwd pointed at the worktree) and a thread is started (or resumed).
4. For each iteration:
   - Skills are reloaded (picks up newly created skills).
   - `prompt::build_prompt` returns the full prompt.
   - A `SessionWriter` is created, writing initial `metadata.json` and opening `events.jsonl`.
   - Prompt is submitted as `Op::UserTurn`.
   - Event stream is consumed until `TurnComplete` or `TurnAborted`; events are streamed to `events.jsonl` as they arrive.
   - On completion, `SessionWriter::finalize()` overwrites `metadata.json` with final summary.
5. Loop exits on `session_complete` tool call, iteration limit, or ctrl-c.
6. Runner sends `Op::Shutdown` with a 5-second timeout.
7. Worktree directory is removed (branch is kept).
8. Resume hint is printed with the session ID.

## Event Handling

`runner` handles these event types from Codex and streams them to `events.jsonl`:

- `AgentMessage`: full message snapshots (fallback when no deltas received)
- `AgentMessageDelta`: streaming partial output → `SessionEvent::Message`
- `ExecCommandBegin` / `ExecCommandEnd`: command lifecycle → `SessionEvent::Command`
- `TokenCount`: token usage snapshots → `SessionEvent::TokenCount`
- `ExecApprovalRequest`: auto-approved in autonomous mode
- `DynamicToolCallRequest`: handles `session_complete` and `session_history` tools
- `TurnComplete`: marks end of a turn
- `TurnAborted`: turn interrupted (e.g. ctrl-c)
- `Error`: logs and ends current turn processing

Other events are ignored.

## Execution Constraints

- By default, `openbot` expects to run inside a git repository.
- `--skip-git-check` disables that requirement.
- Inside a git repo, each run gets its own worktree and branch for isolation. `--no-worktree` opts out.
- Sandbox mode is controlled by bot config (`read-only`, `workspace-write`, `danger-full-access`).

## Prompt Composition

Each prompt includes:

- Base instructions from bot config.
- Status block: project name, session number, worktree branch context.
- Skills section (if any loaded from global + bot-local directories).
- Memory entries (agent's key-value store).
- Last 5 session history summaries for continuity.
- Instructions for using the `session_complete` and `session_history` tools.
- Skill creation hint pointing to the bot's skill directory.
