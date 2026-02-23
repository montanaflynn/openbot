# Memory Format

Memory is scoped per project workspace at `~/.openbot/bots/<name>/workspaces/<slug>/memory.json`. A global fallback exists at `~/.openbot/bots/<name>/memory.json`.

## Schema

```json
{
  "entries": {
    "key": "value"
  },
  "history": [
    {
      "iteration": 1,
      "timestamp": "2026-02-23T12:34:56.123456Z",
      "prompt_summary": "short prompt summary",
      "response_summary": "short response summary"
    }
  ]
}
```

## Semantics

- `entries`
  - Arbitrary key/value store.
  - Managed by `openbot memory <bot> set/remove/clear` and by runtime injections such as `user_input`.

- `history`
  - Append-only sequence of summarized iteration records.
  - Each record includes:
    - `iteration` (u32)
    - `timestamp` (UTC)
    - `prompt_summary`
    - `response_summary`

## Prompt Usage

During prompt assembly:

- All `entries` are injected.
- Only the last 5 `history` records are included.

This keeps continuity while reducing context growth.

## Operational Notes

- Memory file and parent directories are created on first save.
- Invalid JSON at the configured path will fail load.
- `openbot memory <bot> clear` removes both entries and history.
- Use `openbot memory <bot> --project <slug>` to manage memory for a specific workspace.
- When running in a worktree, the workspace is resolved from the original repo root, so all worktrees of the same repo share one workspace.
