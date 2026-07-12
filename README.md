# claude-profile

**Give every task its own Claude Code.** Launch `claude` with *only* the plugins, skills,
marketplaces, and MCP servers a profile defines — everything else on your machine stays
disabled for that session.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)
![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange)

```sh
claude-profile rust-developer      # a lean Rust session — nothing else loaded
claude-profile frontend design     # combine profiles for a task that spans both
claude-profile fuzzyalej/security  # install a shared profile repo, then launch it
```

A standalone cross-platform CLI (Rust; macOS / Linux / Windows) — **not** a plugin, and it
never rewrites your real `~/.claude/settings.json` beyond ordinary plugin installs.

---

## Why

If you use Claude Code seriously, your `~/.claude` becomes a junk drawer. Every plugin, skill,
and MCP server you've ever installed loads into **every** session — the Rust linter you need
today sits in context next to the Kubernetes tools, the writing skills, and the three MCP
servers from that experiment last month. It all costs tokens, it all adds noise, and none of it
is scoped to what you're actually doing.

`claude-profile` fixes that with **profiles**: a tiny JSON file that says "for this kind of
work, load exactly these things." Launch a profile and you get a focused session; open a second
terminal with a different profile and the two don't interfere.

- 🎯 **Focused sessions** — only the tools relevant to the task are enabled, so less context is
  wasted and the model is less distracted.
- 🪟 **Many at once** — different terminals run different profiles simultaneously.
- 📦 **Shareable** — publish a profile repo; teammates `install` it and launch by name. Backed
  by a lockfile so it resolves to the *same* plugin code across machines and over time.
- 🧹 **Non-destructive** — profiles gate what's *enabled* at launch; your global settings and
  installs are left intact. `status` and `gc` keep the shared install set tidy.
- 🔍 **Honest** — it tells you exactly what it can and can't isolate (see the limitation below),
  and warns about anything that would leak through.

## Quick start

```sh
claude-profile rust-developer          # launch a session with only this profile's plugins
claude-profile rust-dev frontend       # launch a combined session (union of several profiles)
claude-profile fuzzyalej/rust-profile  # install a profile repo if needed, then launch its default
claude-profile list                    # profiles and where they come from
claude-profile status                  # what's installed globally and which profiles use it
claude-profile gc --dry-run            # preview cleanup of installs no profile references
claude-profile disable rust-developer  # stop its unshared plugins loading in plain claude
claude-profile find python             # discover plugins to add to a profile
```

> **Tip:** `claude-profile` is a mouthful to type all day. Add `alias cpf=claude-profile` to
> your shell. (We intentionally ship only the one binary name.)

## Discovering plugins

Not sure what to put in a profile? `claude-profile find python` searches a local,
cross-marketplace index and prints copy-paste-ready `plugin@marketplace` ids plus each one's
source repo. Add your own marketplaces to the search by listing them (one `owner/repo` per
line) in `~/.claude-profiles/marketplaces.txt`. See [`find`](docs/commands.md#find) for the
full flag reference.

## What a profile controls

A profile is a small JSON file. When you launch it, `claude-profile` enables exactly its
entries and explicitly disables everything else:

- **Plugins** — every other installed plugin is set to `false` for the session.
- **Loose skills** (`~/.claude/skills`, `.claude/skills`) — gated the same way as plugins **if** the
  skill folder has a `.claude-plugin/plugin.json` manifest (it loads as `name@skills-dir`). A bare
  `SKILL.md` with no manifest auto-loads in every session and **cannot** be gated; `claude-profile`
  warns you about any such skills that will leak through. Add a manifest to make a personal skill
  containable.
- **MCP servers** — launched with `--strict-mcp-config`, so only the profile's servers load;
  your user/project MCP servers never appear. Empty means none.

Profiles can pin marketplace refs and are backed by a lockfile, so the same profile
resolves to the same plugin code across machines and over time.

### Launching several profiles at once

Pass more than one and `claude-profile` launches a **combined** session enabling the union of
them — handy when a task spans two profiles (e.g. `claude-profile mjolner frontend`):

```sh
claude-profile rust-dev frontend        # union of both profiles' plugins/skills/MCP
```

Plugins and pluginDirs union; marketplaces and MCP servers merge key-by-key. If two profiles
define the same marketplace or MCP key with *different* values, the launch aborts and tells you
which key and profiles conflict — it never silently picks one. Each argument can be a name or a
repo reference, and the combined session gets its own lockfile under `~/.claude-profiles/locks/`.

## Sharing profiles: repos and packs

A repo can hold many profiles. `install` clones the repo, keeps **only its profiles** (a root
`profile.json` and any `profiles/*.json`) under `~/.claude-profiles/packs/<owner--repo>/`, and
discards the rest; it doesn't launch anything, it just makes every profile in that repo
available. If the repo has no profiles, `install` aborts with an error. You then launch one by
name:

```sh
claude-profile install owner/repo   # fetch the pack's profiles (aborts if it has none)
claude-profile <profile-name>       # launch the one you want
claude-profile list                 # show every discoverable profile and which pack it's from
```

Launch resolution searches installed packs (`packs/*/profiles/<name>.json`), so it finds the
single profile you name out of the many in the repo.

The `claude-profile owner/repo` shortcut installs the pack **and** launches its *default*
profile — but that only works when the repo has a single profile or a root `profile.json`.
For a repo with a list of profiles it errors with `pack has multiple profiles; specify one by
name`, so the reliable path for a multi-profile repo is the two steps above.

Notes:

- `install` accepts `owner/repo[#ref]` (GitHub shorthand) or a full `https://…` / `git@…` SSH
  git URL (also `#ref`-capable). The same forms work with the `claude-profile <target>` shortcut
  and `claude-profile show <target>`.
- A pack stores profiles only (no `.git`), so re-run `install` to pick up upstream changes;
  `update` doesn't refresh packs. You still select a single profile at *launch* time by name.
- Use `claude-profile show <profile-or-repo>` to preview a profile's details and exactly what it
  would install before launching.

## Important limitation: your global `CLAUDE.md` and memory are NOT gated

This is the one thing a profile **cannot** isolate, and you should understand it before
relying on the tool for a "clean" environment.

Claude Code loads instruction and memory context from sources that are decided by the
client itself, not by the plugin/skill enablement machinery a profile controls:

- **`~/.claude/CLAUDE.md`** (your global instructions) — and project/local `CLAUDE.md` files
- **auto-loaded memory** (`~/.claude/.../memory`, `MEMORY.md`, etc.)

These load in **every** session regardless of profile. So if your global `CLAUDE.md`
imports other instruction files (RTK, lean-ctx directives, tool policies, personal
preferences…), a "minimal rust-developer" profile still carries all of that into context.
A profile trims *plugins/skills/MCP*, not your standing instructions.

### Why we don't "fix" it

The only switch that suppresses `CLAUDE.md` and memory is Claude Code's `--bare` mode, but
`--bare` **requires `ANTHROPIC_API_KEY` authentication and drops OAuth / keychain
login**. For the majority of users (OAuth), turning it on changes how you authenticate.

So the honest behavior is:

- **Default (OAuth or API key):** your global/project `CLAUDE.md` and memory always load.
  Profiles still fully control plugins, skills, and MCP servers.
- **Opt-in absolute isolation:** set `"bare": true` on a profile *if* you authenticate with
  `ANTHROPIC_API_KEY`. This suppresses `CLAUDE.md` and memory too, at the cost of OAuth.

### What to do about it

If you want a genuinely minimal profile without giving up OAuth, keep your global
`~/.claude/CLAUDE.md` lean and move heavyweight, context-specific instructions into
**plugins/skills** (which profiles *can* gate) rather than into global `CLAUDE.md`. That
way the instructions travel with the profiles that want them, instead of loading
everywhere.

## Isolation is runtime gating, not install isolation

Provisioning installs plugins into the shared user scope, so `~/.claude` accumulates the
union of everything any profile has used. Disabled plugins cost zero context, and one shared
install avoids re-downloading. A profile guarantees only that *at launch* nothing but its
entries is enabled; it is not an on-disk sandbox. Use `claude-profile status` /
`claude-profile gc` to keep the shared install set tidy.

## Managing profiles and installs

Four commands cover the lifecycle after a profile exists. They act at different levels — from
"just stop loading it" to "delete it entirely":

```sh
claude-profile disable <profile>            # stop this profile's plugins loading in plain claude
claude-profile disable <profile> --dry-run  # preview which plugins that would touch
claude-profile gc                           # uninstall plugins/marketplaces NO profile references
claude-profile remove <profile>             # delete a personal profile's JSON + lockfile
claude-profile remove <owner/repo>          # delete a cloned pack directory
claude-profile remove <profile> --prune     # delete the profile, then gc what's now unreferenced
```

### `disable` — save tokens between profile sessions

Every plugin you've ever provisioned stays enabled in `~/.claude/settings.json`, so a plain
`claude` session (no profile) loads *all* of them and pays their context cost. `disable <profile>`
sets `enabledPlugins["<id>"] = false` for the plugins that profile uses **and no other profile
uses** — the ones clearly specific to it. Plugins shared with another profile are left alone, so
disabling one profile never breaks another.

Nothing is uninstalled: launching `claude-profile <profile>` re-enables its plugins for that
session (the launch passes its own settings, overriding the global disabled state). So the loop
is: `disable` the heavy profiles you're not actively using, then just launch them when you need
them. Run with `--dry-run` first to see exactly what changes.

### `remove` — delete profile data

`remove` deletes profile **data**, never installed plugin code:

- **`remove <name>`** (a bare name, no `/`): deletes that personal or project profile's JSON file
  and its `<name>.lock` file, if present. The plugins it referenced stay installed — other
  profiles may still use them.
- **`remove <owner/repo>`**: deletes the whole cloned pack directory under
  `~/.claude-profiles/packs/owner--repo/`.
- **`--prune`**: after deleting, runs `gc` so any plugin/marketplace left referenced by no
  remaining profile is uninstalled too. This is the "remove it and clean up after it" option.
- It refuses to remove the engine's bundled `profiles/` (e.g. `rust-developer`) — those
  ship with the binary and aren't your data.

The difference at a glance: **`disable`** keeps the profile and its installs, just quiets them
globally; **`remove`** deletes the profile file; **`remove --prune`** / **`gc`** additionally
uninstall the underlying plugin code.

## Installing

Build from source (available today):

```sh
cargo install --path .
# or
cargo build --release
# then copy target/release/claude-profile onto your PATH
```

Prebuilt installers are configured via [cargo-dist](https://github.com/axodotdev/cargo-dist).
They become available from the first tagged release (`vX.Y.Z`) onward and are not published
yet:

```sh
# macOS / Linux, once a release exists:
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/fuzzyalej/claude-profile/releases/latest/download/claude-profile-installer.sh | sh

# Windows PowerShell, once a release exists:
irm https://github.com/fuzzyalej/claude-profile/releases/latest/download/claude-profile-installer.ps1 | iex

# Homebrew, once a release exists:
brew install fuzzyalej/tap/claude-profile

# crates.io, once published:
cargo install claude-profile
```

The generator config lives in `Cargo.toml` under `[workspace.metadata.dist]`.

## Uninstalling

The built-in command:

```sh
claude-profile self-uninstall           # removes the claude-profile binary
claude-profile self-uninstall --purge   # also removes ~/.claude-profiles (personal
                                         # profiles, cloned packs, and all locks)
```

Or do it manually:

- Remove the binary you installed: delete it from `PATH`, or `cargo uninstall claude-profile`
  if you installed it with `cargo install`.
- Optionally remove profile data (personal profiles, packs, locks):
  ```sh
  rm -rf ~/.claude-profiles
  ```

Either way, **this does NOT remove plugins provisioned into `~/.claude`**. Run
`claude-profile gc` first if you want those gone; they belong to Claude Code, not
`claude-profile`.

## Learn more

- [Authoring profiles](docs/profiles.md) — the profile JSON format, marketplaces, pinning, `extends`.
- [Command reference](docs/commands.md) — every command, its flags, and behavior.
- [How it works](docs/how-it-works.md) — the isolation model, provisioning, pinning, and known limitations.
- [Statusline snippet](docs/statusline.md) — show the active profile in your Claude Code statusline.

## License

MIT — see [LICENSE](./LICENSE).
