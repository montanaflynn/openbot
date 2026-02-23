# Configuration Reference

Each bot has a `config.md` at `~/.openbot/bots/<name>/config.md`.
It uses TOML frontmatter (delimited by `+++`) with a markdown body for instructions.

## Format

```markdown
+++
description = "Short description of the bot"
max_iterations = 10
sleep_secs = 30
+++

Your instructions go here as markdown...
```

All frontmatter keys are optional; omitted values fall back to built-in defaults.
If no frontmatter is present, the entire file is treated as instructions.

## Resolution Order

1. Built-in defaults.
2. Frontmatter keys from `config.md`.
3. CLI overrides for the current invocation.

## Keys

- `description` (`string`)
  - Short description shown in `openbot bots list` and `openbot bots show`.
  - Default: empty.

- `max_iterations` (`integer`)
  - Maximum iterations per run.
  - `0` means unlimited.
  - Default: `10`.

- `sleep_secs` (`integer`)
  - Delay between iterations in seconds.
  - `0` disables sleep.
  - Default: `30`.

- `stop_phrase` (`string` or omitted)
  - If the final agent message contains this phrase, loop exits early.
  - Default: `"TASK COMPLETE"`.

- `model` (`string` or omitted)
  - Model override passed through Codex config.
  - If omitted, Codex default model resolution is used.

- `sandbox` (`string`)
  - One of:
    - `"read-only"`
    - `"workspace-write"`
    - `"danger-full-access"`
  - Unknown values fall back to `workspace-write`.

- `skip_git_check` (`boolean`)
  - If `true`, allows execution outside a git repo.
  - Default: `false`.

## Instructions (markdown body)

Everything after the closing `+++` is the bot's instructions, sent as the base prompt every iteration. This is plain markdown — write whatever you want the agent to do.

## CLI Overrides

For the `run` command:

- `--prompt` overrides instructions (the markdown body).
- `--max-iterations` overrides `max_iterations`.
- `--model` overrides `model`.
- `--sleep` overrides `sleep_secs`.
- `--skip-git-check` sets `skip_git_check = true`.
- `--resume` resumes a previous session by ID.

## Example

See `examples/config.md` in this repository.
