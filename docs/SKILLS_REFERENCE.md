# Skills Reference

Skills are markdown files injected into the agent's prompt. They're loaded from two locations:

- **Global**: `~/.openbot/skills/` (shared by all bots)
- **Bot-local**: `~/.openbot/bots/<name>/skills/` (specific to one bot)

## Skill Format

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

Frontmatter is optional. If missing:
- Skill name falls back to filename stem
- Description defaults to empty
- File content becomes skill body

## Example Skills

See `examples/skills/` in this repository for sample skill files:

- `code-review.md` - Bug- and risk-focused code review process
- `refactor.md` - Safe incremental refactoring workflow

To install an example skill globally:

```sh
cp examples/skills/code-review.md ~/.openbot/skills/
```

Or for a specific bot:

```sh
cp examples/skills/code-review.md ~/.openbot/bots/mybot/skills/
```

## Loading Rules

- Only `*.md` files are loaded.
- Skills are reloaded at the start of each iteration, so bots can create new skills at runtime.
- Bot-local skills take precedence if there's a name conflict with global skills.
- The agent is told where its bot-local skill directory is and encouraged to create skills for reusable procedures.

## Runtime Skill Creation

Bots can create their own skills during execution. The prompt tells the agent:

> If you develop a reusable procedure, save it as a skill in `~/.openbot/bots/<name>/skills/` (markdown with `name:` and `description:` frontmatter). It will be available next iteration.

This means bots can learn and accumulate specialized knowledge over time.
