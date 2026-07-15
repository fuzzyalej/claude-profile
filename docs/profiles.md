# Authoring profiles

A profile is a small JSON file that tells `claude-profile` exactly which plugins, skills,
marketplaces, and MCP servers should be active for a session. Each is vendored (cloned/copied)
into that profile's own private directory and loaded for the session via `--plugin-dir`;
nothing else on the machine is registered at all.

> **In a hurry?** `claude-profile new <name>` scaffolds an empty profile in
> `~/.claude-profiles/` with every field present and ready to fill in — see
> [`new`](commands.md#new).

To find candidate plugins to add, use [`find`](commands.md#find).

## Where profiles live

`claude-profile <name>` resolves `<name>.json` by searching, in order (first match wins):

1. `$CLAUDE_PROFILE_DIR/<name>.json`, if the env var is set.
2. `./profiles/<name>.json` (project-local).
3. `./.claude-profiles/<name>.json` (project-local).
4. `~/.claude-profiles/<name>.json` (personal profiles).
5. `~/.claude-profiles/packs/<owner--repo>/profiles/<name>.json`, one candidate per installed
   pack (see [`install`](commands.md#install)).
6. The engine's own bundled `profiles/<name>.json`: reference profiles shipped with
   `claude-profile` itself (e.g. `rust-developer`, `python-developer`, `react-developer`).
   This directory is resolved relative to the engine, not your current directory, so it is
   distinct from the project-local `./profiles/` in step 2.

`claude-profile list` shows every profile it can find across all of these locations and where
each one came from.

## A worked example

A minimal profile is just a name and one plugin:

```json
{
  "name": "rust-minimal",
  "description": "TDD-driven Rust development",
  "marketplaces": {
    "superpowers-marketplace": "obra/superpowers-marketplace"
  },
  "plugins": [
    "superpowers@superpowers-marketplace"
  ],
  "pluginDirs": [],
  "mcpServers": {}
}
```

Launching it (`claude-profile rust-minimal`) clones the `superpowers-marketplace` marketplace
if it isn't already cached, vendors a copy of the `superpowers` plugin into
`~/.claude-profiles/store/rust-minimal/vendor/` if it isn't already there, then launches
`claude --plugin-dir`-ed at only that vendor directory. Nothing else on the machine is loaded
into the session.

The engine ships richer reference profiles under `profiles/` for many stacks (Rust, Python,
Go, Java, .NET, Ruby, Rails, TypeScript, Angular, Vue, React, plus a Tauri-based
`rust-desktop-developer` and a `frontend` design-implementation profile). Most of these
`extend` a shared `dev-base` profile — the spec-driven `openpowers`/`superpowers` workflow,
live docs (`context7`), code review, and commit plugins — and layer a language server plus
backend/database/performance/testing plugins on top (see
[Inheriting from another profile](#inheriting-from-another-profile) below for how `extends`
works, using `rust-developer`/`dev-base` as a real example). Run `claude-profile list` to see
the full set, or `claude-profile show <name>` to inspect one (with `extends` expanded) before
launching. These were composed with [`find`](commands.md#find) against the cross-marketplace
plugin index.

## Fields

| Field | Type | Meaning |
|---|---|---|
| `name` | string, required | Profile name. Should match the filename (without `.json`). |
| `description` | string, optional | Human-readable summary shown by tooling. |
| `author` | string, optional | Profile author, shown by [`show`](commands.md#show). When absent, `show` falls back to the source repo owner (for pack/URL profiles), else `—`. |
| `marketplaces` | object, optional | Maps a local marketplace name to its source: `owner/repo` (floating, tracks the default branch), `owner/repo#ref` (pinned to a tag/branch/SHA, checked out at that ref and never silently moved), or a full `https://…` / `git@…` SSH git URL (also `#ref`-capable). |
| `plugins` | array of strings, optional | Plugins/skills to vendor, as `plugin@marketplace` ids (or `<name>@skills-dir` for a personal loose skill). Each is copied into this profile's own `~/.claude-profiles/store/<profile>/vendor/`; nothing is installed globally. |
| `pluginDirs` | array of strings, optional | Local paths passed to `claude --plugin-dir`, for plugins developed alongside the profile rather than published to a marketplace. |
| `mcpServers` | object, optional | MCP server definitions passed via `--mcp-config`. Because launches use `--strict-mcp-config`, **any MCP server bundled inside a vendored plugin is otherwise dropped**; redeclare it here if you need it. Empty (`{}`) means no MCP servers for the session. |
| `bare` | boolean, optional (default `false`) | API-key-only absolute isolation. Passes `--bare` to `claude`, which additionally suppresses global/project `CLAUDE.md` and auto-memory: see [how it works](how-it-works.md#what-is-not-gated). Requires `ANTHROPIC_API_KEY` authentication; it drops OAuth/keychain login. |
| `removePlugins` | array of strings, optional | Plugins (as `plugin@marketplace` ids) to drop from the set inherited when this profile `extends` another. Has no effect unless the profile sets `extends`. See [Inheriting from another profile](#inheriting-from-another-profile). |

## Version pinning and the lockfile

A `marketplaces` entry can float (`owner/repo`, tracks the default branch) or pin
(`owner/repo#ref`, checked out at a specific tag/branch/SHA).

The first time a profile is provisioned, `claude-profile` writes a `<profile>.lock` JSON file
(sibling to the profile for personal/project profiles; under `~/.claude-profiles/locks/` for
pack and example profiles) recording the resolved commit SHA of every marketplace it
installed. On every subsequent launch, even for floating marketplaces, `claude-profile`
provisions to the **locked SHA**, not whatever the branch currently points to. That is the
reproducibility guarantee: the same profile resolves to the same plugin code across machines
and over time, until you explicitly move it.

`claude-profile update` (no flag) git-pulls installed profile repos (packs) and, for every
discoverable profile, re-resolves each **floating** (unpinned) marketplace to its current HEAD
commit and rewrites that profile's `.lock` with the new SHA. Marketplaces pinned to an explicit
`#ref` in the profile JSON are never moved by `update`.

`update --frozen` moves nothing: it pulls no packs and re-resolves no pins. Instead it checks
whether any discoverable profile's lock is stale (references a marketplace that's missing from
its `.lock`) and **fails** (non-zero exit) if so, naming every stale profile. Use `--frozen` in
CI/scripts to assert "nothing needs updating" without mutating anything. See
[`update`](commands.md#update) for the full flag reference.

## Inheriting from another profile

A profile can set `"extends": "some-base-profile"` to build on another profile instead of
repeating its contents. `claude-profile` looks up the named base through the same search path
it uses for any profile, then merges the two:

- **`plugins`**: the base's plugins first, then the child's, with duplicates removed. Any id
  listed in the child's `removePlugins` is then dropped from the result.
- **`marketplaces`** and **`mcpServers`**: the base's entries, overlaid with the child's. If
  both define the same key, the child's value wins.
- **`pluginDirs`**: the base's, then the child's, with duplicates removed.
- **`description`**: the child's if set, otherwise the base's.
- **`bare`**: true if either profile sets it.

Inheritance is one level deep. If the base profile itself sets `extends`, or a profile extends
itself, `claude-profile` reports an error instead of following the chain.

For example, a team base profile with the shared toolchain, and a personal profile that adds
one plugin and drops another:

```json
{
  "name": "my-rust",
  "extends": "team-rust",
  "plugins": ["my-scratch-plugin@my-marketplace"],
  "removePlugins": ["pair-programming@team-marketplace"]
}
```

## Referencing a personal loose skill

A **loose skill** living directly under `~/.claude/skills/<name>` or
`.claude/skills/<name>` (project-local takes precedence) can be referenced
in a profile's `plugins` list as `<name>@skills-dir`. claude-profile vendors
a *copy* of that folder into the profile's own vendor directory, generating
a minimal `.claude-plugin/plugin.json` manifest on the copy if the original
doesn't have one. The original skill folder is never modified — only the
vendored copy gains a manifest.
