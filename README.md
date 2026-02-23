# openbot

An autonomous AI agent that runs in a loop, powered by OpenAI's [Codex](https://github.com/openai/codex) runtime. Give it a task, and it will work through it iteratively -- executing commands, reading files, writing code -- sleeping between iterations but waking instantly when you type new input.

## Prerequisites

- **Rust** (edition 2024, rustc 1.85+)
- **OpenAI API key** set as `OPENAI_API_KEY`, or authenticated via `codex login`

## Install

```sh
git clone https://github.com/montanaflynn/openbot
cd openbot
cargo install --path .
```

The first build takes a few minutes since codex-core has a large dependency tree. For a faster debug build during development:

```sh
cargo build
./target/debug/openbot --help
```

## Quick start

```sh
# Run a one-shot task
openbot run --prompt "Find and fix any TODO comments in this project" -n 1

# Run a multi-step task with 5 iterations
openbot run --prompt "Add unit tests for the utils module" -n 5

# Run outside a git repo
openbot run --prompt "Organize these files" --skip-git-check
```

## Usage

```
openbot <COMMAND>

Commands:
  run      Run the agent loop
  skills   List available skills
  memory   Manage persistent memory
  help     Print help
```

### `openbot run`

Start the agent loop. The agent runs your prompt, sleeps, then runs again -- accumulating memory across iterations.

```
openbot run [OPTIONS]

Options:
  -p, --prompt <PROMPT>        Instructions for the agent
  -n, --max-iterations <N>     Max iterations, 0 for unlimited [default: 10]
  -m, --model <MODEL>          Model to use (e.g. o4-mini, gpt-4.1)
  -s, --sleep <SECONDS>        Sleep between iterations (overrides config)
      --skip-git-check         Allow running outside git repos
```

During the sleep window, type anything into stdin to wake the agent immediately and inject that text into its memory for the next iteration.

### `openbot skills`

List all loaded skills and where they came from.

```
$ openbot skills
Available skills (1):

  code-review - Review code for bugs and style issues
    source: skills/code-review.md
```

### `openbot memory`

Inspect and manage the persistent key-value store and iteration history.

```sh
openbot memory show              # Display all entries and history
openbot memory set <KEY> <VALUE> # Store a key-value pair
openbot memory remove <KEY>      # Remove a key
openbot memory clear             # Wipe all memory
```

## Configuration

Drop an `openbot.toml` in your working directory. All fields are optional -- anything omitted uses the default.

```toml
# Base instructions sent to the agent every iteration
instructions = "You are an autonomous AI agent."

# Max iterations per run (0 = unlimited)
max_iterations = 10

# Seconds to sleep between iterations (0 = only run on input)
sleep_secs = 30

# When the agent says this phrase, the loop stops
stop_phrase = "TASK COMPLETE"

# Model override (omit to use the codex default)
# model = "o4-mini"

# Sandbox mode: "read-only" | "workspace-write" | "danger-full-access"
sandbox = "workspace-write"

# Where to persist memory between runs
memory_path = ".openbot/memory.json"

# Directories to scan for skill files
skill_dirs = ["skills", "~/.codex/skills"]

# Allow running outside git repositories
skip_git_check = false
```

CLI flags override config file values. For example, `--prompt` overrides `instructions`, and `-n` overrides `max_iterations`.

## Skills

Skills are markdown files that get injected into the agent's prompt, giving it specialized knowledge or procedures. Place `.md` files in any directory listed in `skill_dirs`.

A skill file looks like this:

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

Skills are compatible with Codex's native `~/.codex/skills/` directory, so any skills you already have there will be picked up automatically if you include that path in `skill_dirs`.

## Memory

openbot maintains a JSON file (default `.openbot/memory.json`) that persists across runs. It contains:

- **Entries** -- a key-value store you can read/write from the CLI or that the agent loop populates
- **History** -- a record of each iteration (timestamp, prompt summary, response summary)

Memory is injected into the agent's prompt each iteration, so the agent is aware of what happened in previous iterations. The last 5 history entries are included to keep the context window manageable.

```sh
# Seed the agent with context before running
openbot memory set project_goal "migrate the database to PostgreSQL"
openbot memory set constraints "must maintain backward compatibility"

# Then run -- the agent sees these entries in its prompt
openbot run --prompt "Work on the project goal described in memory"
```

## How it works

```
                  ┌─────────────────────────┐
                  │      openbot run        │
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
           │                   │ no
           │   ┌───────────────▼───────────────┐
           │   │   Sleep (or wake on stdin)     │
           └───┤                               │
               └───────────────────────────────┘
```

Under the hood, openbot uses `codex-core` directly -- the same Rust library that powers the Codex CLI. It creates a `ThreadManager`, starts a `CodexThread`, and submits `Op::UserTurn` operations, processing the event stream for each iteration. Commands are auto-approved so the agent can work autonomously.

## Debug logging

Set `RUST_LOG` to see what's happening:

```sh
RUST_LOG=info openbot run --prompt "hello" -n 1
RUST_LOG=debug openbot run --prompt "hello" -n 1
```

## Reference Documentation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) - runtime architecture and data flow
- [`docs/CONFIG_REFERENCE.md`](docs/CONFIG_REFERENCE.md) - complete config key reference
- [`docs/MEMORY_FORMAT.md`](docs/MEMORY_FORMAT.md) - persisted memory schema and semantics
- [`docs/SKILLS_REFERENCE.md`](docs/SKILLS_REFERENCE.md) - built-in skill behavior and loading rules

## License

MIT
