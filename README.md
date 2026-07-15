# claude-profile

**Give every task its own Claude Code.** Launch `claude` with *only* the plugins, skills,
marketplaces, and MCP servers a profile defines — everything else on your machine is never
loaded for that session.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)
![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange)

```sh
claude-profile rust-developer      # a lean Rust session — nothing else loaded
claude-profile rust-developer frontend  # combine profiles for a task that spans both
claude-profile fuzzyalej/security  # install a shared profile repo, then launch it
```

A standalone cross-platform CLI (Rust; macOS / Linux / Windows) — **not** a plugin, and it
never writes to your real `~/.claude` at all: every plugin/skill a profile uses is vendored
into that profile's own directory instead.

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
- 🧹 **Non-destructive** — every plugin/skill a profile uses is vendored into that profile's own
  directory; your real `~/.claude` is never written to. `remove` deletes a profile's vendor tree
  outright — a real uninstall, not a disable.
- 🔍 **Honest** — it tells you exactly what it can and can't isolate (see the limitation below).

## Quick start

```sh
claude-profile rust-developer          # launch a session with only this profile's plugins
claude-profile rust-dev frontend       # launch a combined session (union of several profiles)
claude-profile fuzzyalej/rust-profile  # install a profile repo if needed, then launch its default
claude-profile list                    # profiles and where they come from
claude-profile status                  # what's vendored per profile
claude-profile remove rust-developer   # delete a profile and its vendored plugins/skills
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

A profile is a small JSON file. When you launch it, `claude-profile` vendors exactly its
entries into that profile's own directory and loads only those — nothing else on the machine
is registered:

- **Plugins and skills** — every plugin/skill a profile references is vendored (cloned/copied)
  into that profile's own directory under `~/.claude-profiles/store/`, and loaded for the
  session via `--plugin-dir`. Nothing is installed or enabled globally; a plain `claude` session
  never sees any of it.
- **Loose skills** (`~/.claude/skills`, `.claude/skills`) — reference one as `<name>@skills-dir`
  in a profile's `plugins` list and claude-profile vendors a copy, generating a manifest on that
  copy if the original lacks one. The original folder is never touched.
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
client itself, not by the vendored plugin/skill directory a profile loads:

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

## Package-lifecycle model

Each profile is a fully isolated package: `install` = vendor into
`~/.claude-profiles/store/<profile>/`, `launch` = point `claude` at that
directory via `--plugin-dir`, `remove` = delete it. There's no shared global
install to reconcile, disable, or garbage-collect.

```sh
claude-profile status                       # what's vendored per profile
claude-profile remove <profile>              # delete a personal profile's JSON, lock, and vendor tree
claude-profile remove <owner/repo>          # delete a cloned pack directory
```

`remove` deletes a profile's **data and its vendored plugin/skill copies** in one step — a real
uninstall, not a disable:

- **`remove <name>`** (a bare name, no `/`): deletes that personal or project profile's JSON
  file, its `<name>.lock` file if present, and its entire
  `~/.claude-profiles/store/<name>/vendor/` directory.
- **`remove <owner/repo>`**: deletes the whole cloned pack directory under
  `~/.claude-profiles/packs/owner--repo/`.
- It refuses to remove the engine's bundled `profiles/` (e.g. `rust-developer`) — those
  ship with the binary and aren't your data.

Because each profile's vendored plugins/skills are its own private copies, removing one profile
never affects another's, even if both reference the same `plugin@marketplace` id — there's
nothing left to prune or garbage-collect afterward.

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
                                         # profiles, cloned packs, locks, and every
                                         # profile's vendored plugins/skills)
```

Or do it manually:

- Remove the binary you installed: delete it from `PATH`, or `cargo uninstall claude-profile`
  if you installed it with `cargo install`.
- Optionally remove profile data (personal profiles, packs, locks, and every profile's
  vendored plugins/skills):
  ```sh
  rm -rf ~/.claude-profiles
  ```

Either way, this never touches `~/.claude` — `claude-profile` never wrote anything there in
the first place.

## Learn more

- [Authoring profiles](docs/profiles.md) — the profile JSON format, marketplaces, pinning, `extends`.
- [Command reference](docs/commands.md) — every command, its flags, and behavior.
- [How it works](docs/how-it-works.md) — the isolation model, provisioning, pinning, and known limitations.
- [Statusline snippet](docs/statusline.md) — show the active profile in your Claude Code statusline.

### For contributors

Design specs and implementation plans behind notable features in `claude-profile` itself
(not needed to author or use a profile — only if you're changing the tool):

- [Vendored plugin/skill isolation](docs/superpowers/specs/2026-07-15-vendored-plugin-isolation-design.md) ([plan](docs/superpowers/plans/2026-07-15-vendored-plugin-isolation.md)) — why provisioning copies plugins into a private vendor tree instead of installing into `~/.claude`.
- [Cross-marketplace plugin finder](docs/superpowers/specs/2026-07-11-plugin-finder-design.md) ([plan](docs/superpowers/plans/2026-07-11-plugin-finder.md)) — the offline index behind `claude-profile find`.
- [Install/remove progress spinner](docs/superpowers/specs/2026-07-14-install-spinner-design.md) ([plan](docs/superpowers/plans/2026-07-14-install-spinner.md)) — the provisioning UX shown during vendoring.

## License

MIT — see [LICENSE](./LICENSE).
