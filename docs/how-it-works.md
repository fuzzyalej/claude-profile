# How it works

`claude-profile` is a launcher, not a plugin: what loads into a Claude Code session is decided
at `claude` startup from settings, so the control point has to sit outside `claude` itself.
This page condenses the parts of the internal design that matter for using the tool.

## The launch flow

Running `claude-profile <profile> [-- extra claude args]` does, in order:

1. **Resolve.** Find `<profile>.json` via the search path described in
   [profiles.md](profiles.md#where-profiles-live).
2. **Provision.** Diff the profile's `marketplaces`/`plugins` against what's currently
   installed. For anything missing, show a confirmation prompt naming the source and pinned
   ref before installing (`--yes` skips this prompt). Nothing new is installed silently.
3. **Build the `enabledPlugins` override.** Enumerate every installed plugin plus any
   manifest-bearing loose skills, and emit an explicit `true`/`false` for every single one:
   `true` for the profile's own entries, `false` for everything else. This "explicit false
   everywhere" approach means the override can't be defeated by Claude Code's own settings
   merge behavior: there's no ambient state left unset that Claude Code could later enable.
4. **Pin marketplaces.** Resolve each marketplace to its locked commit SHA (writing the lock
   on first use; see [profiles.md](profiles.md#version-pinning-and-the-lockfile)).
5. **Spawn** `claude --settings <override> --strict-mcp-config --mcp-config <profile.mcpServers>
   [--plugin-dir ...] [--bare]`, forwarding anything after `--` and proxying the child's exit
   code back to the caller.

Nothing is written to your real `~/.claude/settings.json` beyond the ordinary installs that
provisioning performs; the enablement override is passed in per-launch via `--settings`.

## Isolation is runtime gating, not install isolation

Provisioning installs plugins into the shared user scope (the same place `claude plugin
install` would put them normally), so `~/.claude` accumulates the union of everything any
profile has ever used. Disabled plugins cost zero context at launch, and a shared install
avoids re-downloading the same plugin for every profile that uses it.

A profile guarantees only that, at launch, nothing but its own entries is enabled for that
session. It is not an on-disk sandbox: other profiles' plugins stay installed but unused. Use
`claude-profile status` to see what's installed and which profiles reference it, and
`claude-profile gc` to uninstall anything no profile references anymore.

## What is NOT gated

Two things load in every Claude Code session regardless of which profile launched it, and a
profile cannot isolate them:

- **Global and project `CLAUDE.md`, plus auto-loaded memory.** These are read by the `claude`
  client itself as part of its normal startup, independent of the plugin/skill enablement a
  profile controls. The only thing that suppresses them is Claude Code's `--bare` mode, which
  requires `ANTHROPIC_API_KEY` authentication and drops OAuth/keychain login, which changes
  how most users authenticate. Set `"bare": true` on a profile if you authenticate with an API
  key and want this stronger isolation.
- **Manifest-less loose skills.** A bare `SKILL.md` with no `.claude-plugin/plugin.json`
  manifest under `~/.claude/skills` or `.claude/skills` auto-loads in every session and can't
  be gated by `enabledPlugins`. `claude-profile` warns loudly at launch about any such skills
  it detects, so you know they loaded anyway.

See the top-level README's ["Important limitation" section](../README.md#important-limitation-your-global-claudemd-and-memory-are-not-gated)
for more on why this isn't "fixed" and what to do about it.

## Pinning and reproducibility

A marketplace reference in a profile is either floating (`owner/repo`, tracks the default
branch) or pinned (`owner/repo#ref`, a specific tag/branch/SHA). Regardless of which, the
*first* successful provision writes a `<profile>.lock` file recording the exact commit SHA
resolved for each marketplace: a git-level pin at the marketplace's install location.

Every subsequent launch provisions to that locked SHA, not whatever a floating branch has
since moved to, until you explicitly move it. `claude-profile update` (no flag) git-pulls the
installed profile repos (packs) under `~/.claude-profiles/packs/` and, for every discoverable
profile, re-resolves each **floating** (unpinned) marketplace to its current HEAD commit and
rewrites that profile's `.lock`. Marketplaces pinned to an explicit `#ref` never move.
`update --frozen` moves nothing and instead **fails** (non-zero exit) if any discoverable
profile's lock is stale, naming the stale profiles. Useful in CI/scripts to assert "nothing
needs updating" without mutating anything.

In practice, a profile installed today and a profile installed from the same JSON six
months from now resolve to identical plugin code, until someone runs `claude-profile update` to
advance the floating pins.

## Further reading

- [Authoring profiles](profiles.md): the full field reference and worked example.
- [Command reference](commands.md): every command's exact flags and behavior.
