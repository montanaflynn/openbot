# Memory Format

Memory is scoped per project workspace at `~/.openbot/bots/<name>/workspaces/<slug>/memory.json`. The slug is derived from the project directory name (e.g. `my-project`).

## Schema

```json
{
  "entries": {
    "key": "value"
  }
}
```

## Semantics

- `entries`
  - Arbitrary key/value store.
  - Managed by `openbot memory <bot> set/remove/clear` and by runtime injections such as `user_input`.
  - All entries are injected into the agent's prompt each iteration.

## Prompt Usage

During prompt assembly:

- All `entries` are injected as a key-value list.
- The last 5 session history summaries (from `history/` directory) are also included.

This gives the agent continuity across sessions while keeping context growth manageable.

## Operational Notes

- Memory file and parent directories are created on first save.
- Invalid JSON at the configured path will fail load.
- `openbot memory <bot> clear` removes all entries.
- Use `openbot memory <bot> --project <slug>` to manage memory for a specific workspace.
- The slug is derived from the project directory name (e.g. `/home/user/myapp` -> `myapp`).
- When running in a worktree, the workspace is resolved from the original repo root, so all worktrees of the same repo share one workspace.
- The `~/.openbot/` directory is fully portable across machines — just rsync/copy it.
