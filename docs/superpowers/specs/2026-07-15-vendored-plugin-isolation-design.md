# Vendored Plugin/Skill Isolation

## Problem

Today, `claude-profile provision` (`src/provision.rs`) installs a profile's
plugins/marketplaces by shelling out to `claude plugin marketplace add` and
`claude plugin install --scope user`. Both write into the user's real
`~/.claude` (settings.json, `~/.claude/plugins/`), and installs accumulate
there for the lifetime of the machine regardless of which profile is
currently active. `claude-profile <profile>` gates what's *enabled* at
launch by passing an explicit `--settings '{"enabledPlugins": {...}}'`
override, but that gate only applies to sessions launched through
`claude-profile` itself.

This creates a real gap: anyone who runs plain `claude` (forgetting the
launcher, or in a context where it isn't set up) gets the union of every
plugin any profile has ever provisioned, because nothing was ever
uninstalled — only toggled off for one session. Personal skills without a
`.claude-plugin/plugin.json` manifest are worse: Claude Code has no
enablement mechanism for them at all, so bare `SKILL.md` folders under
`~/.claude/skills` load in *every* session, profile-gated or not
(`enablement.rs`'s `leaking_skills`, currently just a warning).

The user's requirements for closing this gap:
- Plain `claude` must never load a profile's plugins/skills.
- `claude-profile` must never write into the user's real `~/.claude`.
- A profile must be manageable as a fully isolated package: install, use,
  uninstall, with no shared global state to reconcile.
- The mechanism must be transparent (inspectable on disk, no symlink tricks
  or generated files grafted onto the user's own directories).

## Goals

- Replace `claude plugin install`/`marketplace add`/`uninstall`/
  `marketplace remove` with claude-profile-owned vendoring: each profile's
  plugins and skills are copied into a private, per-profile directory tree
  claude-profile fully owns.
- Replace the `--settings enabledPlugins` launch-time override with
  `--plugin-dir` flags pointing at that profile's vendored directories —
  a mechanism `launch.rs` already uses for `profile.plugin_dirs`.
- Zero writes to `~/.claude` (settings.json, `~/.claude/plugins`,
  `~/.claude/skills`) by any claude-profile command.
- A profile's on-disk footprint is fully self-contained under
  `~/.claude-profiles/store/<profile-key>/`; deleting that directory is a
  complete uninstall.
- Personal bare-skill folders can be vendored with a claude-profile-generated
  manifest, as a *copy* — the user's original skill folder is never touched.
- Keep marketplace SHA-pinning and the lockfile format/semantics unchanged.

## Non-goals

- No cross-profile de-duplication of vendored plugin code. Two profiles
  using the same plugin each get their own copy. Revisit only if disk usage
  becomes a real complaint.
- No auto-update of vendored code on every launch — vendored copies only
  change via the existing `update` flow (advancing a floating marketplace
  pin and re-vendoring against the new SHA).
- No change to combined-profile launches' merge semantics (union of
  plugins/marketplaces/MCP servers, conflict-on-mismatch) — only how the
  *result* gets provisioned and launched.
- No change to how profile *packs* (repo-hosted profile JSON) are fetched
  (`pack.rs` is unaffected; it's a different kind of repo).

## Design

### Storage layout

```
~/.claude-profiles/
  store/
    marketplaces/<mkt-name>/            # marketplace repo, cloned+pinned by claude-profile
    marketplaces/_external/<owner>--<repo>/  # externally-sourced individual plugins
    <profile-key>/
      vendor/<plugin-id>/                # copy of a plugin's directory, at the pinned SHA
      vendor/<skill-name>/               # a personal skill, copied + manifest-wrapped
      profile.lock                        # unchanged format (LockedMarketplace{source, sha})
```

`<profile-key>` matches how locks are already keyed today (single profile
name, or a stable combined-profile key for multi-profile launches).

### Provisioning (`provision.rs` rewrite)

Replaces `ClaudeCli::marketplace_add`/`install_plugin` calls with:

1. **Marketplace clone.** `GitCli::clone` the marketplace source into
   `store/marketplaces/<name>/` if not already present (this is the same
   `GitCli` trait already used for profile packs — no new abstraction).
   `pin_marketplaces` keeps its current signature and logic; the
   `mkt_install_dir` closure now points at this claude-profile-owned path
   instead of asking `claude plugin marketplace list --json` for
   `installLocation`.
2. **Plugin resolution.** Read `.claude-plugin/marketplace.json` from the
   cloned marketplace. Each listed plugin's `source` is either:
   - a relative in-repo path (`"./skills/foo"`) — copy that subdirectory
     straight out of the marketplace clone; or
   - an external repo (`{"source": "github", "repo": "owner/x"}`) — clone
     it into `store/marketplaces/_external/<owner>--<x>/` and copy from
     there.
   Any other `source` shape is a hard error naming the plugin id and the
   raw JSON — no silent fallback for formats we haven't seen.
3. **Copy into vendor dir.** `store/<profile-key>/vendor/<plugin-id>/` is
   populated as a full copy (not a symlink — a moving marketplace clone
   must not retroactively change an already-launched profile's vendored
   code). `update` re-copies wholesale against the newly pinned SHA rather
   than patching in place.
4. **Skill vendoring.** For a profile's skill references, copy the source
   skill folder into `store/<profile-key>/vendor/<skill-name>/`. If the
   copy lacks `.claude-plugin/plugin.json`, generate a minimal one so the
   skill is a normal `--plugin-dir`-loadable unit; the original folder
   (wherever the user keeps it) is never modified. A name collision with an
   existing vendor entry is a hard error, not a silent overwrite.

Confirmation prompt (`confirm()`) keeps its current shape but names
vendor-copy operations instead of "install into user scope."

### Launch (`launch.rs` rewrite)

- Drop `--settings '{"enabledPlugins": ...}'` entirely — there is no global
  enablement state left to override.
- For every entry under `store/<profile-key>/vendor/*`, add a
  `--plugin-dir <path>` flag (this already happens for `profile.plugin_dirs`
  today; vendored entries just extend the same list).
- `--strict-mcp-config`/`--mcp-config`, `--bare`, and forwarded extra args
  are unchanged.

### `enablement.rs`

The module's purpose (compute an `enabledPlugins` map, track
`leaking_skills`) goes away — there is nothing left to enable/disable
globally, and manifest-less skills are closed by vendor-time wrapping
instead of warned about. Its `scan_skills_dir` logic is repurposed for
provisioning: finding a personal skill's manifest (or absence of one) at
vendor time.

### Commands affected

- **`disable`, `gc`**: removed. Both exist solely to manage the shared
  global install that no longer exists; there's nothing to disable or
  garbage-collect once each profile owns its own vendor tree.
- **`status`**: rewritten to report `store/<profile-key>/` directories,
  their vendored contents, and disk size — a local filesystem listing
  instead of cross-referencing `claude plugin list` against profiles.
- **`remove`**: gains real teeth — deleting a profile now also deletes its
  `store/<profile-key>/` vendor tree, so plugin code is actually reclaimed
  (today's `remove` explicitly does *not* uninstall plugin code; that
  limitation is gone). `--prune` becomes a no-op / is removed, since there's
  no shared install left to prune.
- **`claude.rs`**: `ClaudeCli` trait (marketplace_add, install_plugin,
  uninstall_plugin, marketplace_remove, list_plugins, list_marketplaces)
  is deleted along with `RealClaude`. Nothing left calls into `claude
  plugin *` subcommands.
- **`refmap.rs`**: deleted — it exists to compute "which plugins does no
  profile reference" against the shared install, which no longer applies.
- **Lockfile (`lock.rs`)**: unchanged.

### Error handling

- Marketplace/plugin fetch failure (bad `source`, network, unrecognized
  `source` shape): abort provisioning for that profile, naming the
  offending plugin id. No partial vendor tree is left half-populated for
  that plugin (copy into a temp dir, then rename into place).
- `update` against a moved lock SHA: re-copy wholesale, replacing the vendor
  dir atomically (temp dir + rename), never patching in place.
- Skill-name collision at vendor time: hard error, no overwrite.

### Testing

- New seam analogous to today's `ClaudeCli`/`GitCli` mocks: a vendoring
  step that operates against an injectable filesystem root, so
  copy/resolve logic is unit-testable without real git or `~/.claude-profiles`.
- Golden test for `.claude-plugin/marketplace.json` resolution using the
  real two-source-kind shape found in a local marketplace (relative path +
  `{"source": "github", "repo": ...}` in the same file), not just a
  synthetic fixture.
- `pin_marketplaces` tests are largely reusable as-is (same `GitCli`
  interface); only the `mkt_install_dir` wiring changes.

### Documentation updates

- `docs/how-it-works.md`: rewrite "Isolation is runtime gating, not install
  isolation" → "Isolation is install isolation," describing vendor dirs;
  drop the `enabledPlugins`-override narrative from the launch-flow list;
  the "What is NOT gated" section shrinks — manifest-less skills are no
  longer an unfixable limitation, since vendoring wraps them.
- `docs/commands.md`: remove `disable`/`gc` entries; update `status` and
  `remove`'s described behavior.
- `docs/profiles.md`: note that `plugins`/`marketplaces` entries resolve to
  per-profile vendored copies, not shared user-scope installs.
- `README.md`: rewrite "What a profile controls" and the "Provisioning
  installs plugins into the shared user scope..." explanation (the
  disable/gc mini-workflow described there goes away); the "Important
  limitation" section on manifest-less skills shrinks accordingly.
