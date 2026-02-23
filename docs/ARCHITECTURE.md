# Architecture

This document explains the runtime architecture of `openbot` and how modules interact.

## Overview

`openbot` is a thin CLI wrapper over the Codex Rust runtime. It manages named bots, each with their own configuration, skills, and persistent memory. When you run a bot, it enters an iterative loop that:

1. Loads the bot's configuration, skills (global + local), and memory.
2. Builds a prompt for the current iteration.
3. Submits a `UserTurn` to Codex.
4. Streams events/results.
5. Persists a summarized history record.
6. Repeats until completion criteria are met.

## Directory Layout

```
~/.openbot/
├── skills/                    # Global skills (all bots)
└── bots/
    └── <name>/
        ├── config.md          # Bot config (TOML frontmatter + markdown body)
        ├── skills/            # Bot-local skills
        ├── memory.json        # Global memory (fallback)
        └── workspaces/        # Per-project memory
            └── <slug>/
                └── memory.json
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
  - Defines persistent memory model (`entries`, `history`).
  - Handles JSON load/save (per-workspace at `~/.openbot/bots/<name>/workspaces/<slug>/memory.json`).
  - Provides CLI-friendly rendering for `openbot memory <bot> show`.

- `src/prompt.rs`
  - Assembles iteration prompt from instructions, skills, memory, and meta instructions.
  - Includes iteration count, urgency warnings, and worktree branch context.
  - Tells the agent where to save new skills.

- `src/runner.rs`
  - Orchestrates the main agent loop.
  - Creates a git worktree for isolation (default) or runs in the working tree (`--no-worktree`).
  - Starts or resumes a Codex session/thread.
  - Submits turns, consumes event stream, persists iteration summaries, and handles sleep/wake behavior.
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
   - Prompt is submitted as `Op::UserTurn`.
   - Event stream is consumed until `TurnComplete` or `TurnAborted`.
   - Last response is summarized and saved via `MemoryStore`.
5. Loop exits on stop phrase, iteration limit, or ctrl-c.
6. Runner sends `Op::Shutdown` with a 5-second timeout.
7. Worktree directory is removed (branch is kept).
8. Resume hint is printed with the session ID.

## Event Handling

`runner` handles these event types:

- `AgentMessage`: full message snapshots
- `AgentMessageDelta`: streaming partial output
- `ExecCommandBegin` / `ExecCommandEnd`: command lifecycle logging
- `ExecApprovalRequest`: auto-approved in autonomous mode
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
- Current iteration marker with remaining count.
- Worktree branch name and integration instructions (when running in a worktree).
- Skills section (if any).
- Memory entries and last 5 history items.
- Standard completion instructions including the `TASK COMPLETE` convention.
- Skill creation hint pointing to the bot's skill directory.
