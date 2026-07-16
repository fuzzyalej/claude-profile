# Statusline: show the active profile

`claude-profile statusline install` adds a `statusLine` entry to Claude Code's
`settings.json` that shows the active profile — colored per profile name — whenever
a session was launched through `claude-profile`. Outside a `claude-profile` session
(a plain `claude` invocation), it shows nothing.

It composes with whatever `statusLine` command you already had configured: your
existing command still runs, with the profile tag prefixed in front of its output.

## Install

```bash
claude-profile statusline install            # global: ~/.claude/settings.json
claude-profile statusline install --project  # this repo only: ./.claude/settings.json
```

This backs up whatever `statusLine` config was already there (or records that there
wasn't one) before making the change, so `uninstall` can put it back exactly as it was.

Running `install` again when it's already installed is a no-op.

## Uninstall

```bash
claude-profile statusline uninstall
claude-profile statusline uninstall --project
```

Restores the prior `statusLine` config (or removes the key entirely if there wasn't
one), and deletes the backup. If `settings.json`'s `statusLine` was hand-edited since
install, `uninstall` warns and leaves it alone rather than overwriting your changes —
the backup is kept in that case, so nothing is lost.

## How it works

When `claude-profile` launches a session, it spawns `claude` with the environment
variable `CLAUDE_PROFILE` set to the profile name (see `src/launch.rs`) — combined
launches get the joined name, e.g. `web+infra`. The installed statusline reads that
variable on every render tick; when it's unset (a plain `claude` session), the
profile tag is simply omitted.

## Notes

- Respects `NO_COLOR`: set it to get a plain `[profile-name]` tag with no ANSI color
  codes.
- This is read-only observability layered on top of your existing statusline. It
  does not change what's enabled. See [how it works](how-it-works.md) for the actual
  plugin/skill isolation model.
- Prefer to wire this up by hand instead? The statusline command is just
  `claude-profile statusline-render` — point your own `settings.json`'s `statusLine.command`
  at it directly, or read `$CLAUDE_PROFILE` yourself in a custom script.
