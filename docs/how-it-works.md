# How it works

`claude-profile` is a launcher, not a plugin: what loads into a Claude Code session is decided
at `claude` startup from the `--plugin-dir` flags it's invoked with, so the control point has
to sit outside `claude` itself. This page condenses the parts of the internal design that
matter for using the tool.

## The launch flow

Running `claude-profile <profile> [-- extra claude args]` does, in order:

1. **Resolve.** Find `<profile>.json` via the search path described in
   [profiles.md](profiles.md#where-profiles-live).
2. **Vendor.** For each marketplace/plugin/skill the profile references that
   isn't already vendored for this profile, show a confirmation prompt
   naming what will be cloned/copied (`--yes` skips this prompt). Nothing
   new is fetched silently, and nothing is ever written to your real
   `~/.claude`.
3. **Pin marketplaces.** Resolve each marketplace clone to its locked commit
   SHA (writing the lock on first use; see
   [profiles.md](profiles.md#version-pinning-and-the-lockfile)), then copy
   each referenced plugin/skill out of that pinned checkout into
   `~/.claude-profiles/store/<profile>/vendor/`.
4. **Spawn** `claude --strict-mcp-config --mcp-config <profile.mcpServers>
   --plugin-dir <each vendored entry> [--plugin-dir ...profile.pluginDirs]
   [--bare]`, forwarding anything after `--` and proxying the child's exit
   code back to the caller.

Nothing is ever written to your real `~/.claude/settings.json`, `~/.claude/plugins`,
or `~/.claude/skills`. Every plugin and skill a profile uses lives under
`~/.claude-profiles/store/<profile>/vendor/`, a directory claude-profile fully
owns.

## Isolation is install isolation

Unlike gating what's *enabled* in a shared install, each profile is a fully
self-contained package: `~/.claude-profiles/store/<profile>/vendor/` holds
copies of exactly that profile's plugins and skills, vendored out of a
pinned marketplace checkout. Plain `claude`, run without `claude-profile`,
never sees any of it — there is nothing registered anywhere `claude` looks
by default. `claude-profile status` lists what's vendored per profile, and
`claude-profile remove <profile>` deletes a profile's vendor tree along with
its profile file — a real uninstall, not just a disable.

Marketplace *clones* (the repos claude-profile copies plugin code out of)
are cached once per marketplace name under `~/.claude-profiles/store/marketplaces/`
and reused across profiles that reference the same marketplace, to avoid
re-cloning — but each profile's *vendored copy* of a plugin is independent,
so profiles never share mutable state.

## What is NOT gated

Two things load in every Claude Code session regardless of which profile launched it, and a
profile cannot isolate them:

- **Global and project `CLAUDE.md`, plus auto-loaded memory.** These are read by the `claude`
  client itself as part of its normal startup, independent of which vendored plugin/skill
  directory a profile loads. The only thing that suppresses them is Claude Code's `--bare` mode, which
  requires `ANTHROPIC_API_KEY` authentication and drops OAuth/keychain login, which changes
  how most users authenticate. Set `"bare": true` on a profile if you authenticate with an API
  key and want this stronger isolation.
- **Manifest-less loose skills.** A bare `SKILL.md` with no
  `.claude-plugin/plugin.json` manifest under `~/.claude/skills` or
  `.claude/skills` still can't be *referenced by Claude Code's own*
  enablement system — but claude-profile no longer needs that system.
  Reference it in a profile's `plugins` list as `<name>@skills-dir` and
  claude-profile vendors a **copy** of it, generating a minimal manifest on
  that copy if one is missing. The original skill folder is never modified.

See the top-level README's ["Important limitation" section](../README.md#important-limitation-your-global-claudemd-and-memory-are-not-gated)
for more on why this isn't "fixed" and what to do about it.

## Pinning and reproducibility

A marketplace reference in a profile is either floating (`owner/repo`, tracks the default
branch) or pinned (`owner/repo#ref`, a specific tag/branch/SHA). Regardless of which, the
*first* successful provision writes a `<profile>.lock` file recording the exact commit SHA
resolved for each marketplace: a git-level pin at the marketplace's install location.

Every subsequent launch provisions to that locked SHA, not whatever a floating branch has
since moved to, until you explicitly move it. `claude-profile update profiles` git-pulls the
installed profile repos (packs) under `~/.claude-profiles/packs/` and, for every discoverable
profile, re-resolves each **floating** (unpinned) marketplace to its current HEAD commit and
rewrites that profile's `.lock`. Marketplaces pinned to an explicit `#ref` never move.
`update profiles --frozen` moves nothing and instead **fails** (non-zero exit) if any
discoverable profile's lock is stale, naming the stale profiles. Useful in CI/scripts to assert
"nothing needs updating" without mutating anything.

(Plain `claude-profile update`, with no subcommand, instead checks whether a newer
`claude-profile` release exists — see [commands.md](commands.md#update).)

In practice, a profile installed today and a profile installed from the same JSON six
months from now resolve to identical plugin code, until someone runs
`claude-profile update profiles` to advance the floating pins.

## Further reading

- [Authoring profiles](profiles.md): the full field reference and worked example.
- [Command reference](commands.md): every command's exact flags and behavior.
