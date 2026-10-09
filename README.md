# claude-profile

**Give every task its own Claude Code.** Launch `claude` with *only* the plugins, skills,
marketplaces, and MCP servers a profile defines — everything else on your machine is never
loaded for that session.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)
![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange)

A standalone cross-platform CLI (Rust; macOS / Linux / Windows) — **not** a plugin, and it
never installs anything into your real `~/.claude`: every plugin/skill a profile uses is
vendored into that profile's own directory instead.

```sh
claude-profile rust-developer           # a lean Rust session — nothing else loaded
claude-profile rust-developer frontend  # combine profiles for a task that spans both
claude-profile fuzzyalej/security       # install a shared profile repo, then launch it
```

## Why

If you use Claude Code seriously, your `~/.claude` becomes a junk drawer. Every plugin, skill,
and MCP server you've ever installed loads into **every** session — the Rust linter you need
today sits in context next to the Kubernetes tools, the writing skills, and the three MCP
servers from that experiment last month. It all costs tokens, it all adds noise, and none of it
is scoped to what you're actually doing.

`claude-profile` fixes that with **profiles**: a tiny JSON file that says "for this kind of
work, load exactly these things." Launch a profile and you get a focused session; open a second
terminal with a different profile and the two don't interfere.

- 🎯 **Focused sessions** — only the tools relevant to the task are enabled.
- 🪟 **Many at once** — different terminals run different profiles simultaneously.
- 📦 **Shareable** — publish a profile repo; teammates `install` it and launch by name, backed
  by a lockfile so it resolves to the *same* plugin code across machines and over time.
- 🧹 **Non-destructive** — every plugin/skill a profile uses is vendored into that profile's own
  directory; `remove` deletes it outright — a real uninstall, not a disable.

## Installing

Pick whichever fits your platform — all install the same `claude-profile` binary.

**macOS / Linux — Homebrew:**

```sh
brew install fuzzyalej/tap/claude-profiles
```

**macOS / Linux — shell (no Homebrew, no Rust needed):**

```sh
curl -LsSf https://github.com/fuzzyalej/claude-profile/releases/latest/download/claude-profiles-installer.sh | sh
```

**Windows — PowerShell:**

```powershell
irm https://github.com/fuzzyalej/claude-profile/releases/latest/download/claude-profiles-installer.ps1 | iex
```

See [Before you start on Windows](#before-you-start-on-windows) for prerequisites.

**Any platform with Rust — Cargo:**

```sh
cargo install claude-profiles
```

Then verify:

```sh
claude-profile --version
```

### Before you start on Windows

`claude-profile` runs in PowerShell, Command Prompt, Windows Terminal, and Git Bash. Before you
launch a profile, make sure you have the following:

- **Git for Windows.** `claude-profile` uses `git` to fetch marketplaces and profile repos. Git
  for Windows also includes `sh.exe`, which runs an existing statusline command. To install it,
  run `winget install --id Git.Git -e`.
- **Claude Code, with `claude` on your `PATH`.** Both the native installer (`claude.exe`) and an
  npm install (`claude.cmd`) work.

To check both, open a new terminal and run:

```powershell
git --version
claude --version
```

If either command isn't found, install the missing tool, and then open a new terminal so it
picks up the updated `PATH`.

Good to know:

- Profiles and vendored plugins are stored in `%USERPROFILE%\.claude-profiles`. If `HOME` is
  set, as it is in Git Bash, `claude-profile` uses `%HOME%\.claude-profiles` instead. Use the
  same shell type each time, so all your sessions see the same profiles.
- If you installed Git in a custom location, set `CLAUDE_CODE_GIT_BASH_PATH` to the full path of
  `bash.exe` or `sh.exe`. Claude Code reads the same variable. If no shell is found, the statusline
  command runs through `cmd /C` instead.
- `claude-profile completions powershell --install` adds completion to the `$PROFILE` of every
  PowerShell edition you have installed, PowerShell 7 and Windows PowerShell 5.1 alike.

## Updating

Update the same way you installed:

```sh
brew upgrade claude-profiles                # Homebrew

curl -LsSf https://github.com/fuzzyalej/claude-profile/releases/latest/download/claude-profiles-installer.sh | sh   # shell (macOS/Linux) — re-run to get latest

irm https://github.com/fuzzyalej/claude-profile/releases/latest/download/claude-profiles-installer.ps1 | iex        # Windows PowerShell — re-run to get latest

cargo install claude-profiles              # Cargo — reinstalls the latest published version
```

<details>
<summary>Build from source instead</summary>

```sh
cargo install --path .
# or
cargo build --release   # then copy target/release/claude-profile onto your PATH
```
</details>

> **Tip:** `claude-profile` is a mouthful to type all day. Add `alias cpf=claude-profile` to
> your shell. (We intentionally ship only the one binary name.)

Shell completion is built in:

```sh
claude-profile completions zsh --install   # bash, fish, powershell also supported
```

See [`completions`](docs/commands.md#completions) for install paths per shell and OS.

## Quick start

```sh
claude-profile rust-developer          # launch a session with only this profile's plugins
claude-profile rust-dev frontend       # launch a combined session (union of several profiles)
claude-profile fuzzyalej/rust-profile  # install a profile repo if needed, then launch its default
claude-profile list                    # profiles and where they come from (bundled ones included)
claude-profile status                  # what's vendored per profile
claude-profile remove rust-developer   # delete a profile and its vendored plugins/skills
claude-profile find python             # discover plugins to add to a profile
```

Not sure what to put in a profile? `claude-profile find python` searches a local,
cross-marketplace index and prints copy-paste-ready `plugin@marketplace` ids. See
[Authoring profiles](docs/profiles.md) for the full JSON format and [`find`](docs/commands.md#find)
for the flag reference.

## Bundled profiles

`claude-profile` ships ready-made profiles for Rust, Python, Go, Java, .NET, Ruby, Rails,
TypeScript, Angular, Vue, React, and Tauri desktop work, plus `frontend`, `rails-hyperdrive`,
and `clean` (plain Claude Code, no extras). They are built into the binary and available as
soon as you install it. Run `claude-profile list` to see them, or `claude-profile show <name>`
to inspect one. See [Authoring profiles](docs/profiles.md#bundled-profiles).

## What a profile controls

A profile is a small JSON file naming plugins, skills, marketplaces, and MCP servers. Launching
it vendors exactly those entries into the profile's own directory and loads only those — nothing
else on the machine is registered. Passing several profiles at once launches a combined session
with the union of all of them.

Full details, including the vendoring/pinning model and how multi-profile launches merge and
conflict-check: [How it works](docs/how-it-works.md).

## Important limitation: your global `CLAUDE.md` and memory are NOT gated

Claude Code loads your global/project `CLAUDE.md` and auto-loaded memory from sources a profile
cannot isolate — they load in **every** session regardless of profile. See
[How it works § What is NOT gated](docs/how-it-works.md#what-is-not-gated) for why this isn't
"fixed" and what to do about it.

## Sharing profiles

A repo can hold many profiles. `install` clones it and makes every profile in it available by
name, without launching anything:

```sh
claude-profile install owner/repo   # fetch the pack's profiles
claude-profile <profile-name>       # launch the one you want
```

See [Command reference](docs/commands.md) for `install`, `show`, and pack update semantics.

## Coordinate many sessions

`coordinate` launches a session that delegates tasks to worker sessions. Each worker runs with
its own profile, in its own git worktree and branch:

```sh
claude-profile coordinate rust-developer -- --permission-mode acceptEdits   # ask it to split work
claude-profile runs --clean                                                 # remove finished workers' worktrees
```

Headless workers can't ask you to approve edits, so pass a permission mode that allows them.

Inside [Herdr](https://herdr.dev), workers open in their own background tabs in the
coordinator's workspace. Elsewhere they run headless. See
[Coordinate many sessions](docs/coordinate.md).

## Uninstalling

```sh
claude-profile self-uninstall           # removes the claude-profile binary
claude-profile self-uninstall --purge   # also removes ~/.claude-profiles (personal
                                         # profiles, cloned packs, locks, and every
                                         # profile's vendored plugins/skills)
```

`claude-profile remove <profile>` removes a single profile's data and vendored plugins/skills —
see [Command reference](docs/commands.md#remove).

## Learn more

- [Authoring profiles](docs/profiles.md) — the profile JSON format, marketplaces, pinning, `extends`.
- [Command reference](docs/commands.md) — every command, its flags, and behavior.
- [Coordinate many sessions](docs/coordinate.md) — run workers from a coordinator session.
- [How it works](docs/how-it-works.md) — the isolation model, provisioning, pinning, and known limitations.
- [Statusline snippet](docs/statusline.md) — show the active profile in your Claude Code statusline.

## License

MIT — see [LICENSE](./LICENSE).
