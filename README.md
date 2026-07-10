# claude-profile

Launch `claude` with **only** the plugins, skills, marketplaces, and MCP servers a
profile defines. Everything else installed on your machine stays disabled for that
session. A standalone cross-platform CLI (Rust; macOS / Linux / Windows), not a plugin.

Different terminals can run different profiles at the same time, and none of it touches
your real `~/.claude/settings.json` beyond ordinary plugin installs.

See [`docs/`](./docs/README.md) for user documentation (profile authoring, full command
reference, how it works).

## Quick start

```sh
claude-profile rust-developer          # launch a session with only this profile's plugins
claude-profile fuzzyalej/rust-profile  # install a profile repo if needed, then launch its default
claude-profile list                    # profiles and where they come from
claude-profile status                  # what's installed globally and which profiles use it
claude-profile gc --dry-run            # preview cleanup of installs no profile references
```

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

See the shell alias tip below for a shorter name to type.

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

## Note on the name

The binary is `claude-profile` everywhere (installers included). It's a bit long to type,
so a shell alias helps:

```sh
alias cpf=claude-profile
```

We intentionally don't ship a second binary name.
