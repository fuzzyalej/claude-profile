# Command reference

This page documents the shipped command surface exactly as `claude-profile --help` and each
subcommand's `--help` report it.

```
Usage: claude-profile [OPTIONS] [PROFILE] [-- <EXTRA>...] [COMMAND]

Commands:
  list            List available profiles and their sources
  show            Show a profile's details and what it would install
  install         Install or refresh a profile repo (owner/repo[#ref]) without launching
  update          Git-pull profile repos and re-resolve floating marketplaces
  status          Show installed plugins/marketplaces and which profiles reference each
  remove          Delete a personal profile or cloned pack
  new             Scaffold a new profile in ~/.claude-profiles/
  test            Run `claude plugin eval` against a plugin/skill target
  find            Search a local index of plugins across marketplaces
  self-uninstall  Remove the claude-profile binary (and optionally profile data)
  help            Print this message or the help of the given subcommand(s)
```

## `claude-profile <profile>... [-- <extra>]`

```
Arguments:
  [PROFILES]...  Profile name(s) to launch (when no subcommand is given)
  [EXTRA]...     Extra args forwarded to claude after `--`

Options:
      --yes  Skip the provisioning confirmation prompt
```

Launches a session with only the named profile's plugins, skills, and MCP servers enabled.
Running `claude-profile` with no subcommand and no profile prints this help and exits.

Passing **several profiles** launches a combined session enabling the union of them all — e.g.
`claude-profile rust-dev frontend`. Each argument is resolved independently, so any of them may
be a profile name or a repo reference (`owner/repo`, URL). The profiles are merged:

- **plugins** and **pluginDirs**: union (deduped).
- **marketplaces** and **mcpServers**: merged key-by-key. If two profiles define the **same** key
  with **different** values, the launch **aborts** and names the conflicting key and profiles
  (`marketplace 'm' defined differently by a (o/a) and b (o/b); resolve before combining`);
  identical definitions merge cleanly.
- **bare**: must be the same across all selected profiles, else the launch aborts.

A combined launch pins marketplaces into its own lockfile at
`~/.claude-profiles/locks/<a+b>.lock` and sets `CLAUDE_PROFILE=<a+b>`; the individual profiles'
own lockfiles are untouched. A single profile behaves exactly as before.

- **Reads:** the profile JSON (see [profiles.md](profiles.md) for resolution order), its
  `<profile>.lock` file if present, and what's already vendored for this profile under
  `~/.claude-profiles/store/<profile>/vendor/`.
- **Writes:** on first launch (or whenever new marketplaces/plugins are referenced), clones any
  missing marketplaces into `~/.claude-profiles/store/marketplaces/`, copies each referenced
  plugin/skill out of the pinned checkout into this profile's own
  `~/.claude-profiles/store/<profile>/vendor/`, and writes/updates `<profile>.lock` with the
  resolved commit SHA of each marketplace used.
- **Safety behavior:** before vendoring anything new, prints a confirmation prompt showing each
  marketplace/plugin's source and pinned ref. `--yes` skips this prompt for scripted use.
  Anything after a literal `--` is forwarded to the underlying `claude` invocation unchanged
  (e.g. `claude-profile rust-developer -- --continue`).

## `claude-profile <owner/repo>`

Sugar for "install this profile repo, then launch its default profile." Equivalent to running
`claude-profile install <owner/repo>` (which keeps only the repo's profiles, and aborts if it
has none) followed by launching the repo's default profile. Detected by the presence of a `/`
in the argument (as opposed to a bare profile name), so a full `https://…` or `git@…` SSH URL
works here too.

## `show`

```
Usage: claude-profile show <TARGET>
```

Prints a profile's details and exactly what launching it would vendor, without launching or
vendoring anything.

- `<TARGET>` as a bare name: resolves the profile through the normal search path (see
  [profiles.md](profiles.md)), expanding `extends` so the plugin/marketplace lists reflect the
  full effective set.
- `<TARGET>` as a repo reference (`owner/repo[#ref]`, `https://…`, or `git@…` SSH): shallow-clones
  the repo into a **throwaway temp directory**, shows its default profile, then discards the
  clone. Nothing is written to `~/.claude-profiles/packs/`.
- **Output:** name, description, author (the profile's `author` field, falling back to the source
  repo owner, else `—`), source, then the marketplaces, plugins, MCP servers, and plugin dirs it
  declares. A marketplace is marked `(installed)` if its clone already exists under
  `~/.claude-profiles/store/marketplaces/`, else `+ … (new)`. A plugin/skill is marked
  `(installed)` if it's already vendored into this profile's own
  `~/.claude-profiles/store/<profile>/vendor/`, else `+ … (new)` if launching would vendor it.
  On a color terminal, installed entries are dimmed and new ones green; output is plain text
  when piped or when `NO_COLOR` is set.

## `install`

```
Usage: claude-profile install <SPEC>
```

Installs a profile repo without launching anything. `<SPEC>` is `owner/repo`,
`owner/repo#ref` (tag/branch/SHA), or a full `https://…` / `git@…` SSH git URL (also `#ref`-capable).

- **Behavior:** clones the repo into a temporary directory and keeps **only its profiles** —
  a root `profile.json` and everything under `profiles/*.json`. Everything else in the repo
  (source, READMEs, the `.git` history) is discarded.
- **Aborts** with a non-zero exit if the repo contains no profiles:
  `repo '<spec>' contains no profiles (expected a profile.json or profiles/*.json); nothing to install`.
- **Writes:** the profile files into `~/.claude-profiles/packs/owner--repo/` (the `owner`/`repo`
  are taken from the URL path tail), replacing any previous install of the same pack. Because a
  pack is stored profiles-only (no `.git`), re-running `install` re-clones fresh rather than
  pulling in place, and `update` does not refresh packs — re-run `install` to pick up changes.
- Does not provision any plugins/marketplaces referenced by the pack's profiles. That
  happens the first time one of its profiles is actually launched.

## `update`

```
Usage: claude-profile update [OPTIONS]

Options:
      --frozen  Fail if the lockfile is out of date instead of updating
```

Git-pulls every installed profile repo (pack) and re-resolves floating marketplaces.

- **Without `--frozen`:**
  1. Pulls every directory under `~/.claude-profiles/packs/` that is still a git checkout,
     printing `updated pack <name>` for each. Packs installed profiles-only (the current
     `install` behavior) have no `.git` and are skipped — re-run `install` to refresh them.
  2. For every discoverable profile (same search path as `list`), re-resolves each
     **floating** (unpinned/branch-tracking) marketplace to its current HEAD commit and
     rewrites that profile's `.lock` file with the new SHA. Marketplaces pinned to an
     explicit tag/SHA in the profile JSON are left alone; only floating refs move.
- **With `--frozen`:** does **not** pull packs and does **not** move any marketplace pin.
  Instead it re-resolves what each profile's lock *would* need and compares it against what's
  currently in `<profile>.lock`. If any profile's lock is stale (missing or out of date for a
  marketplace the profile references), the command **fails** and names every stale profile:
  `--frozen: lock out of date for profile(s): <name>, <name>, ...`. If every lock is current it
  prints `--frozen: all locks up to date` and exits successfully. Use `--frozen` in CI/scripts
  to assert "nothing needs updating" without mutating anything.

## `list`

```
Usage: claude-profile list
```

Lists every profile `claude-profile` can find across the full search path (env dir, project
dirs, personal profiles, installed packs, and the engine's own bundled profiles), deduplicated by name
with the highest-priority location winning, along with where each one resolved from.

## `status`

```
Usage: claude-profile status
```

Shows, per discoverable profile, what's vendored under
`~/.claude-profiles/store/<profile>/vendor/`: `(not yet provisioned)` if the profile has never
been launched/installed, otherwise the count and list of vendored plugin/skill ids. There is no
shared install to report on — each profile's vendor tree is independent, so nothing here is
ever "unreferenced" by another profile.

## `remove`

```
Usage: claude-profile remove <TARGET>
```

Deletes a profile's data **and** its vendored plugin/skill copies — a real uninstall, not a
disable.

- `<TARGET>` as a bare name (no `/`): deletes that personal/project profile's JSON file, its
  `.lock` file if any, and its entire `~/.claude-profiles/store/<name>/vendor/` directory.
- `<TARGET>` as `owner/repo`: deletes the entire cloned pack directory
  (`~/.claude-profiles/packs/owner--repo/`).
- **Safety behavior:** refuses to remove one of the engine's own bundled `profiles/` (e.g.
  `rust-developer`): those ship with the binary and aren't user data.
- Because each profile's vendored plugins/skills are its own private copies, removing one
  profile never affects another's vendor tree, even if both reference the same
  `plugin@marketplace` id. There's nothing to prune or garbage-collect afterward.

## `new`

```
Usage: claude-profile new <NAME>
```

Scaffolds a new personal profile.

- **Writes:** creates `~/.claude-profiles/<name>.json` with an empty template (`name`,
  `description`, `marketplaces`, `plugins`, `pluginDirs`, `mcpServers` all present but empty)
  and prints the path plus a reminder to launch it with `claude-profile <name>` once edited.
- **Safety behavior:** refuses to overwrite an existing file. Fails with `profile '<name>'
  already exists at <path>` if one is already there.
- See [profiles.md](profiles.md) for the JSON format to fill in.

## `test`

```
Usage: claude-profile test [OPTIONS] <TARGET> [-- <EXTRA>...]

Options:
      --json
```

Runs `claude plugin eval <TARGET>`, a thin wrapper that forwards to Claude Code's own plugin
evaluator so you can test a plugin/skill without hand-writing the `claude` invocation.

- **Behavior:** builds the argv `plugin eval <TARGET> [--json] [EXTRA...]` and executes
  `claude` with it, returning `claude`'s own exit code (falls back to `1` if the process didn't
  report one).
- `<TARGET>`: the plugin or skill identifier to evaluate, exactly as `claude plugin eval`
  expects it.
- `--json`: forwarded to `claude plugin eval --json` for machine-readable output.
- Anything after a literal `--` is forwarded to `claude plugin eval` unchanged (e.g. `claude-profile
  test my-skill -- --case "smoke*"`).

## `find`

```
Usage: claude-profile find [OPTIONS] [QUERY]...

Options:
      --sync                    Rebuild the index from seeds (fetches marketplace manifests)
      --refresh-seeds           Harvest new marketplace seeds before syncing (not yet implemented)
      --json                    Machine-readable output
      --limit <N>           Maximum number of results [default: 20]
      --marketplace <NAME>  Filter results to a single marketplace
```

Searches a local, offline index of plugins across many marketplaces and prints results as
profile-ready `plugin@marketplace` ids, each with its marketplace's source repo
(`owner/repo`) — copy-paste-ready for a profile's `plugins` and `marketplaces` fields.

- **Behavior:** `<QUERY>` words are joined and matched against each indexed plugin's name,
  description, and category (metadata only — it does not search skill file bodies). If no
  index exists yet, the first run syncs automatically before searching; every later run
  searches the cached index offline unless `--sync` is given. Running `find` with `--sync` or
  `--refresh-seeds` and no query just rebuilds the index without searching.
- `--sync`: rebuilds the index now, fetching each seed marketplace's manifest over the
  network.
- `--refresh-seeds`: intended to harvest new marketplace seeds before syncing; not yet
  implemented — prints a notice and falls back to the existing seed list.
- `--json`: prints the matching entries as a JSON array instead of the human-readable listing.
- `--limit <N>`: caps the number of results (default `20`).
- `--marketplace <NAME>`: restricts results to entries from one marketplace.
- **Seeds:** the index is built from a seed list of marketplace repos — the ~59 marketplaces
  bundled with `claude-profile`, plus every marketplace you currently have installed, plus
  any repos you add to `~/.claude-profiles/marketplaces.txt` (one `owner/repo` per line, `#`
  starts a comment).
- **Reads/writes:** the index is cached at `~/.claude-profiles/.index-cache/index.json`. On a
  no-match search, the message includes the index's `generated_at` timestamp so you know how
  stale it might be; re-run with `--sync` to refresh it.

## `self-uninstall`

```
Usage: claude-profile self-uninstall [OPTIONS]

Options:
      --purge  Also remove ~/.claude-profiles (all personal profiles, packs, locks)
```

Removes the `claude-profile` binary itself.

- **Writes:** deletes the currently running executable (`std::env::current_exe()`), i.e.
  whatever binary you actually invoked.
- `--purge`: additionally removes the entire `~/.claude-profiles` directory — personal
  profiles, cloned packs, all `.lock` files, and every profile's vendored plugins/skills under
  `store/`. Without `--purge`, profile data (and vendored plugin/skill copies) is left in place.
- **Safety behavior:** never touches `~/.claude`. Since every plugin/skill claude-profile uses
  lives in its own vendor tree under `~/.claude-profiles/store/`, `--purge` alone is a complete
  uninstall — there is no separate shared-scope cleanup step needed.

See the [top-level README](../README.md#uninstalling) for the full uninstall walkthrough
(including the manual, no-binary-needed path).
