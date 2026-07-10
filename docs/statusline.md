# Statusline: show the active profile

When `claude-profile` launches a session, it spawns `claude` with the environment variable
`CLAUDE_PROFILE` set to the profile name (see `src/launch.rs`). That variable is only present
in sessions started through `claude-profile`; a plain `claude` invocation never sets it. So it
tells you whether you're in a gated profile right now, and which one.

Claude Code's [statusline](https://docs.claude.com/en/docs/claude-code/statusline) feature runs
an arbitrary shell command on an interval and shows its stdout at the bottom of the terminal.
Point it at `$CLAUDE_PROFILE` to show the active profile in the terminal.

## Setup

Add a `statusLine` entry to your Claude Code `settings.json` (global `~/.claude/settings.json`
or a project's `.claude/settings.json`):

```json
{
  "statusLine": {
    "type": "command",
    "command": "echo \"${CLAUDE_PROFILE:-no-profile}\""
  }
}
```

- Inside a `claude-profile <name>` session, the statusline shows `<name>`.
- Inside a plain `claude` session (not launched via `claude-profile`), it falls back to
  `no-profile`.

## Combining with other statusline content

The command only needs to *include* the profile; it doesn't have to be the whole line. For
example, alongside the current directory:

```json
{
  "statusLine": {
    "type": "command",
    "command": "echo \"[${CLAUDE_PROFILE:-no-profile}] $(basename \"$PWD\")\""
  }
}
```

Claude Code also passes session context as JSON on stdin to the statusline command for more
advanced scripts (model, cwd, cost, etc.). See the
[statusline docs](https://docs.claude.com/en/docs/claude-code/statusline) for the full schema.
`$CLAUDE_PROFILE` is simply an extra environment variable available alongside that input.

## Notes

- `$CLAUDE_PROFILE` reflects the profile that spawned the *current* `claude` process. If you
  run `claude` directly (not through `claude-profile`) inside a shell that happens to have
  `CLAUDE_PROFILE` exported from a previous session, that stale value will show. Export it
  only in shells where you want it scoped, or rely on the fact that `claude-profile` always
  re-sets it correctly for the process it spawns.
- This is read-only observability. It does not change what's enabled. See
  [how it works](how-it-works.md) for the actual isolation model.
