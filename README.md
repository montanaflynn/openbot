# openbot

An autonomous AI agent that runs in a loop, powered by OpenAI's [Codex](https://github.com/openai/codex) runtime. Create named bots with their own instructions, skills, and memory -- then run them on any project.

## Prerequisites

- **Rust** (edition 2024, rustc 1.85+)
- **OpenAI API key** set as `OPENAI_API_KEY`, or authenticated via `codex login`

## Install

```sh
git clone https://github.com/montanaflynn/openbot
cd openbot

# Local development (fast)
cargo build
./target/debug/openbot --help

# Release install
cargo install --path .
```

## Quick start

```sh
# Create a bot
openbot bots create secbot --description "Security auditor" --prompt "Audit this codebase for security issues"

# Run it (creates an isolated git worktree automatically)
openbot run -b secbot

# Run with overrides
openbot run -b secbot -n 5 --model o4-mini

# Run multiple bots concurrently -- no conflicts
openbot run -b secbot &
openbot run -b testbot &

# Run directly in the working tree (opt out of worktree isolation)
openbot run -b secbot --no-worktree

# Run outside a git repo
openbot run -b secbot --skip-git-check
```

## Usage

```
openbot <COMMAND>

Commands:
  run      Run a bot
  bots     Manage bots
  skills   Manage skills (list, search, install, remove)
  memory   Manage a bot's memory
  help     Print help
```

### `openbot run`

Start the agent loop for a named bot. The agent runs the bot's instructions, sleeps, then runs again -- accumulating memory across iterations.

Inside a git repo, each run automatically gets its own worktree and branch (`openbot/<bot>-<timestamp>`) so multiple bots can run concurrently without file conflicts. The worktree is cleaned up on exit; the branch is kept so no commits are lost.

```
openbot run [OPTIONS] --bot <BOT>

Options:
  -b, --bot <BOT>              Bot name
  -p, --prompt <PROMPT>        Override the bot's instructions
  -n, --max-iterations <N>     Max iterations, 0 for unlimited [default: 10]
  -m, --model <MODEL>          Model to use (e.g. o4-mini, gpt-4.1)
  -s, --sleep <SECONDS>        Sleep between iterations (overrides config)
      --skip-git-check         Allow running outside git repos
      --resume <SESSION_ID>    Resume a previous session
      --project <SLUG>         Use a specific project workspace by slug
      --no-worktree            Run directly in the working tree (skip worktree isolation)
```

During the sleep window, type anything into stdin to wake the agent immediately and inject that text into its memory for the next iteration.

When interrupted with ctrl-c, the agent shuts down gracefully and prints a resume command you can use to continue where you left off.

### `openbot bots`

Manage named bots.

```sh
openbot bots list                                    # List all bots
openbot bots create mybot                            # Create with defaults
openbot bots create mybot --prompt "Do X and Y"     # Create with custom instructions
openbot bots create mybot -d "My helper" -p "..."   # Create with description + instructions
openbot bots show mybot                              # Show config, skills, memory stats
```

### `openbot skills`

Manage skills -- list, search, install, and remove.

```sh
openbot skills list <BOT>                            # List loaded skills
openbot skills search "code review"                  # Search the skills.sh registry
openbot skills install owner/repo/skill --global     # Install globally
openbot skills install owner/repo/skill --bot mybot  # Install for a specific bot
openbot skills remove skill-name --global            # Remove a global skill
```

### `openbot memory`

Inspect and manage a bot's persistent memory. Memory is scoped per project workspace.

```sh
openbot memory mybot show              # Display all entries and history
openbot memory mybot set <KEY> <VALUE> # Store a key-value pair
openbot memory mybot remove <KEY>      # Remove a key
openbot memory mybot clear             # Wipe all memory
openbot memory mybot --project slug show  # Target a specific workspace
```

## Directory structure

All data lives under `~/.openbot/`:

```
~/.openbot/
├── skills/                    # Global skills (shared by all bots)
│   └── code-review.md
└── bots/
    └── secbot/
        ├── config.md          # Bot config (TOML frontmatter + markdown instructions)
        ├── skills/            # Bot-local skills
        │   └── custom.md
        ├── memory.json        # Global memory (fallback)
        └── workspaces/        # Per-project memory
            └── my-project/
                └── memory.json
```

- **Global skills** (`~/.openbot/skills/`) are available to every bot.
- **Bot-local skills** (`~/.openbot/bots/<name>/skills/`) are only available to that bot.
- Bots can create their own skills at runtime -- they're picked up on the next iteration.
- **Memory is per-project** -- each project directory gets its own workspace with separate memory. Use `--project <slug>` to target a specific workspace from anywhere.

## Bot configuration

Each bot has a `config.md` — a markdown file with TOML frontmatter. The frontmatter holds settings, and the markdown body is the bot's instructions.

```markdown
+++
description = "Security auditor for OWASP top 10"
max_iterations = 5
sleep_secs = 10
sandbox = "workspace-write"
+++

You are a security auditor. Scan this codebase for vulnerabilities...
```

All frontmatter fields are optional:

| Field | Default | Description |
|-------|---------|-------------|
| `description` | (empty) | Short description shown in `bots list` |
| `max_iterations` | `10` | Max iterations per run, `0` = unlimited |
| `sleep_secs` | `30` | Seconds between iterations, `0` = no sleep |
| `stop_phrase` | `"TASK COMPLETE"` | Phrase that ends the loop |
| `model` | (codex default) | Model override (e.g. `o4-mini`) |
| `sandbox` | `"workspace-write"` | `read-only`, `workspace-write`, or `danger-full-access` |
| `skip_git_check` | `false` | Allow running outside git repos |

CLI flags override config values. See `examples/config.md` for a full example.

## Skills

Skills are markdown files that get injected into the agent's prompt, giving it specialized knowledge or procedures.

```markdown
---
name: code-review
description: Review code for bugs and style issues
---
When asked to review code, follow these steps:
1. Read the file thoroughly
2. Check for bugs, security issues, and style problems
3. Provide actionable feedback with specific line references
```

The YAML frontmatter (`name`, `description`) is optional. Without it, the filename is used as the skill name.

Place skills in `~/.openbot/skills/` for global availability, or in `~/.openbot/bots/<name>/skills/` for a specific bot. See `examples/skills/` for sample skill files.

## Memory

Each bot maintains per-project memory that persists across runs. It contains:

- **Entries** -- a key-value store you can read/write from the CLI or that the agent populates
- **History** -- a record of each iteration (timestamp, prompt summary, response summary)

Memory is injected into the agent's prompt each iteration, so the agent is aware of what happened previously. The last 5 history entries are included to keep the context window manageable.

```sh
# Seed the agent with context before running
openbot memory secbot set project_goal "migrate the database to PostgreSQL"
openbot memory secbot set constraints "must maintain backward compatibility"

# Then run -- the agent sees these entries in its prompt
openbot run -b secbot

# Manage memory for a specific project workspace
openbot memory secbot --project my-project show
```

## How it works

```
                  ┌─────────────────────────┐
                  │    openbot run -b <bot>  │
                  └────────────┬────────────┘
                               │
                  ┌────────────▼────────────┐
                  │  Create git worktree    │
                  │  (isolated branch)      │
                  └────────────┬────────────┘
                               │
                  ┌────────────▼────────────┐
                  │   Load config, skills,  │
                  │   memory, start codex   │
                  └────────────┬────────────┘
                               │
               ┌───────────────▼───────────────┐
               │   Build prompt:               │
           ┌──►│   instructions + skills +     │
           │   │   memory + iteration context  │
           │   └───────────────┬───────────────┘
           │                   │
           │   ┌───────────────▼───────────────┐
           │   │   Submit to codex-core        │
           │   │   (model runs, executes       │
           │   │    commands, writes files)     │
           │   └───────────────┬───────────────┘
           │                   │
           │   ┌───────────────▼───────────────┐
           │   │   Save results to memory      │
           │   └───────────────┬───────────────┘
           │                   │
           │              stop phrase?──── yes ──► done
           │                   │ no                  │
           │   ┌───────────────▼───────────────┐     │
           │   │   Sleep (or wake on stdin)     │     │
           └───┤                               │     │
               └───────────────────────────────┘     │
                  ┌──────────────────────────────────┘
                  │  Remove worktree (keep branch)
                  └──────────────────────────────
```

Under the hood, openbot uses `codex-core` directly -- the same Rust library that powers the Codex CLI. It creates a `ThreadManager`, starts a `CodexThread`, and submits `Op::UserTurn` operations, processing the event stream for each iteration. Commands are auto-approved so the agent can work autonomously.

## Debug logging

Set `RUST_LOG` to see what's happening:

```sh
RUST_LOG=info openbot run -b secbot -n 1
RUST_LOG=debug openbot run -b secbot -n 1
```

## Reference Documentation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) - runtime architecture and data flow
- [`docs/CONFIG_REFERENCE.md`](docs/CONFIG_REFERENCE.md) - complete config key reference
- [`docs/MEMORY_FORMAT.md`](docs/MEMORY_FORMAT.md) - persisted memory schema and semantics
- [`docs/SKILLS_REFERENCE.md`](docs/SKILLS_REFERENCE.md) - skill loading rules and examples

## License

MIT
