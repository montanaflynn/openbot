# openbot

Autonomous AI agents that run in a loop, ship code, and learn across sessions.

Create named bots with their own instructions, skills, and persistent memory. Point them at any git repo and they work autonomously — in isolated branches, executing commands, writing code, and committing changes. When they're done, merge the branch or review it yourself.

Built on OpenAI's [Codex](https://github.com/openai/codex) runtime in Rust.

```sh
# Create a bot
openbot bots create security-bot \
  --prompt "Audit this codebase for OWASP top 10 vulnerabilities. Fix what you find."

# Run it — gets its own worktree and branch automatically
openbot run -b security-bot

# That's it. It creates a branch, reads the code, runs commands,
# fixes issues, commits, and merges — all autonomously.
```

<details>
<summary>See what a session looks like</summary>

```
## Session 1

Model:     5.3-codex
Workspace: my-project
Branch:    openbot/security-bot-1740000000
Skills:    2
Memory:    0 entries
History:   0 sessions

### Output

Scanning project structure and dependencies...
  $ cargo audit
  $ grep -rn "unsafe" src/
Found 3 potential vulnerabilities.
  $ cargo test
  $ git add -A && git commit -m "fix: sanitize user input in API handlers"

### Summary

Result:    Fixed 3 security issues: SQL injection in query builder, XSS in template
           rendering, missing auth check on /admin endpoint
Action:    merged into main
Duration:  34s
Tokens:    12,480 input (8,200 cached) / 3,456 output (200 reasoning)
Resume:    openbot run --resume abc123
```
</details>

## Install

Requires **Rust 1.85+** and an **OpenAI API key** (`OPENAI_API_KEY` env var or `codex login`).

```sh
git clone https://github.com/montanaflynn/openbot
cd openbot
cargo install --path .
```

## Why openbot

**Autonomous, not interactive.** Most AI coding tools are chat interfaces. openbot runs unattended — create a bot, give it a task, come back to merged code.

**Isolated by default.** Every run gets its own git worktree and branch. Your working tree is never touched. If the bot breaks something, your main branch is safe. The bot decides to merge, leave for review, or discard.

**Bots that learn.** Skills (markdown procedures) and memory (key-value store) persist across sessions. Bots improve at their specific task over time. They can even write their own skills at runtime.

**Run them in parallel.** Each bot gets its own branch — no conflicts. Run a security auditor, a test writer, and a refactoring bot simultaneously on the same repo.

**Crash-safe history.** Every event (messages, commands, token usage) is streamed to disk as it happens. If a session crashes, everything up to that point is preserved.

## Quick start

```sh
# Create a bot with specific instructions
openbot bots create test-bot \
  --prompt "Find untested code paths and add comprehensive unit tests. Run tests after every change."

# Run it on your project
cd ~/my-project
openbot run -b test-bot

# Run with options
openbot run -b test-bot -n 5 --model 5.3-codex    # 5 iterations, specific model
openbot run -b test-bot --no-worktree            # skip worktree isolation
openbot run -b test-bot --resume <SESSION_ID>    # continue where you left off
```

### Give bots context before running

```sh
# Seed memory with project-specific knowledge
openbot memory test-bot --project my-project set priority "focus on the auth module"
openbot memory test-bot --project my-project set known_issues "flaky test in test_concurrent_login"

# Bot sees these in its prompt
openbot run -b test-bot
```

### Run multiple bots at once

```sh
openbot run -b security-bot &
openbot run -b test-bot &
openbot run -b docs-bot &
# Each gets its own branch — no conflicts
```

### Review what a bot did

```sh
openbot history test-bot                          # list recent sessions
openbot history test-bot --session <SESSION_ID>   # full session detail

# Or inspect the git branch directly
git log main..openbot/test-bot-1740000000
git diff main..openbot/test-bot-1740000000
```

## Key concepts

### Bot configuration

Each bot has a `config.md` at `~/.openbot/bots/<name>/config.md` — TOML frontmatter for settings, markdown body for instructions:

```markdown
+++
description = "Security auditor"
max_iterations = 5
sleep_secs = 10
model = "5.3-codex"
sandbox = "workspace-write"
+++

You are a security auditor. Scan this codebase for vulnerabilities.
For each finding, report the file, line, severity, and a suggested fix.
Run tests to verify your fixes don't break anything.
```

All fields are optional. CLI flags override config values.

### Skills

Skills are markdown files injected into the bot's prompt. Install from a registry, write your own, or let bots create them at runtime:

```sh
openbot skills search "code review"                         # find skills
openbot skills install obra/superpowers/brainstorming --bot mybot  # install one
openbot skills list mybot                                   # see what's loaded
```

```markdown
---
name: always-test
description: Run tests after every change
---
After every code change, run the project's test suite.
If tests fail, fix the issue before moving on.
Never commit code with failing tests.
```

Skills in `~/.openbot/skills/` are global. Skills in `~/.openbot/bots/<name>/skills/` are bot-specific.

### Session history

Each session is stored as a directory with metadata and an append-only event stream:

```
history/{session_id}/
  metadata.json    # session summary (model, duration, tokens, result)
  events.jsonl     # every message, command, and token count as it happened
```

Events are flushed to disk immediately — nothing is lost on crashes. Bots can also browse their own history during a session to learn from past runs.

### Memory

Per-project key-value store that persists across sessions. Memory entries plus the last 5 session summaries are injected into every prompt:

```sh
openbot memory mybot --project my-app show                # view entries
openbot memory mybot --project my-app set key "value"     # set a value
openbot memory mybot --project my-app remove key          # remove
openbot memory mybot --project my-app clear               # wipe
```

During multi-iteration runs, type into stdin during the sleep window to inject context into the next iteration.

### Data layout

```
~/.openbot/
├── skills/                    # global skills (all bots)
└── bots/
    └── mybot/
        ├── config.md          # bot config + instructions
        ├── skills/            # bot-specific skills
        └── workspaces/
            └── my-project/
                ├── memory.json
                └── history/
                    └── {session_id}/
                        ├── metadata.json
                        └── events.jsonl
```

Fully portable — no absolute paths. Copy `~/.openbot/` to another machine and everything works.

## CLI reference

```
openbot run      Run a bot
openbot bots     Manage bots (list, create, show)
openbot skills   Manage skills (list, search, install, remove)
openbot history  View session history
openbot memory   Manage bot memory (show, set, remove, clear)
```

<details>
<summary><code>openbot run</code> options</summary>

```
-b, --bot <BOT>              Bot name (required)
-p, --prompt <PROMPT>        Override instructions
-n, --max-iterations <N>     Max iterations, 0 = unlimited [default: 10]
-m, --model <MODEL>          Model (e.g. 5.3-codex, o3)
-s, --sleep <SECONDS>        Sleep between iterations
    --skip-git-check         Run outside git repos
    --resume <SESSION_ID>    Resume a previous session
    --project <SLUG>         Target a specific workspace
    --no-worktree            Skip worktree isolation
```
</details>

## Documentation

- **[User Guide](docs/USER_GUIDE.md)** — comprehensive walkthrough of all features
- **[Architecture](docs/ARCHITECTURE.md)** — runtime design, module map, data flow
- **[Config Reference](docs/CONFIG_REFERENCE.md)** — all configuration options
- **[Skills Reference](docs/SKILLS_REFERENCE.md)** — skill format, loading rules, examples
- **[Memory Format](docs/MEMORY_FORMAT.md)** — persistence schema and semantics

## License

MIT
