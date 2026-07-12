# Design: `claude-profile find` — cross-marketplace plugin/skill discovery

**Date:** 2026-07-11
**Status:** Approved (design)

## Problem

Composing a profile (e.g. `python-developer`) currently means knowing which plugins exist.
Discovery is manual and biased toward whatever marketplace happens to be installed locally.
There are ~200 marketplaces and thousands of plugins across the ecosystem. We want a local,
offline-searchable index so a query like `find python` or `find "backend architecture"`
returns a ranked list of candidate `plugin@marketplace` ids, ready to drop into a profile.

## Non-goals

- Not a full-text search of skill/command bodies. Index is **metadata-only**
  (plugin name + description + category from each marketplace's manifest). This is ~90% of
  discovery value at a fraction of the cost.
- Not a package installer. `find` only surfaces candidates; installing/enabling remains the
  job of profiles + `claude`.
- No new runtime dependency on aggregator sites. Sync must work even if aggregators change
  their page formats.

## CLI

```
claude-profile find <query...>            # search cached index (auto-syncs on first run)
claude-profile find --sync                # rebuild the index from seeds (network)
claude-profile find python --limit 15
claude-profile find "backend" --json
claude-profile find react --marketplace superpowers-marketplace
claude-profile find --refresh-seeds       # best-effort: re-harvest aggregator seed list
```

Clap subcommand:
```rust
Find {
    query: Vec<String>,
    #[arg(long)] sync: bool,
    #[arg(long)] refresh_seeds: bool,
    #[arg(long)] json: bool,
    #[arg(long)] limit: Option<usize>,   // default 20
    #[arg(long)] marketplace: Option<String>,
}
```

Default human output, one line per hit:
```
pyright-lsp@claude-plugins-official   Python language server (Pyright) for type checking…
    repo: anthropics/claude-plugins-official
```
The `plugin@marketplace` id and the `marketplace -> owner/repo` mapping are exactly what a
profile's `plugins` and `marketplaces` fields need, so results are copy-paste ready.

## Architecture

Five small, independently testable units under a new `src/index/` module (plus one command).

| Unit | File | Responsibility | I/O |
|---|---|---|---|
| `index::seeds` | `src/index/seeds.rs` | Produce the deduped marketplace repo list: embedded curated defaults ∪ installed marketplaces ∪ user file. | reads files only |
| `index::model` | `src/index/model.rs` | Parse a `marketplace.json` and normalize each plugin into `IndexEntry`. | pure |
| `index::fetch` | `src/index/fetch.rs` | Obtain one marketplace's `marketplace.json` (local clone if present, else sparse+shallow git fetch). | `GitCli` + fs |
| `index::search` | `src/index/search.rs` | Rank `&[IndexEntry]` against query terms. | pure |
| `commands::find` | `src/commands/find.rs` | Orchestrate sync + search + output. | wires the above |

### Data model

```rust
pub struct IndexEntry {
    pub plugin: String,        // e.g. "pyright-lsp"
    pub marketplace: String,   // e.g. "claude-plugins-official"
    pub repo: String,          // "owner/repo" for the profile `marketplaces` field
    pub description: String,
    pub category: Option<String>,
}
// Persisted index: { "generated_at": <rfc3339 str>, "entries": [IndexEntry, ...] }
```

`repo` is the **marketplace** repo (what you install `plugin@marketplace` from), not the
plugin's upstream `source`. A plugin's own `source` in the manifest is informational and is
ignored for indexing purposes.

### Seed list

Source of truth is a union, deduped case-insensitively on `owner/repo`:

1. **Embedded defaults** — `src/index/default_seeds.rs`: a static `&[&str]` of `owner/repo`,
   harvested **once** from the community aggregators (ComposioHQ/awesome-claude-plugins,
   quemsah/awesome-claude-plugins, claudemarketplaces.com) at build time by the author and
   committed. Always includes `anthropics/claude-plugins-official` and
   `obra/superpowers-marketplace`.
2. **Installed marketplaces** — parsed from `~/.claude/plugins/known_marketplaces.json`
   (`source.repo` for github, or `owner/repo` extracted from `source.url` for git). Picks up
   private/internal marketplaces (e.g. the user's Azure DevOps ones) automatically.
3. **User file** — `~/.claude-profiles/marketplaces.txt`, one `owner/repo` per line, `#`
   comments, blank lines ignored. Created with a header comment on first `sync` if absent.

`--refresh-seeds` runs a **best-effort** scraper that fetches the aggregators and rewrites a
`~/.claude-profiles/.index-cache/harvested-seeds.txt`, merged into the union on subsequent
syncs. Failure here warns and is non-fatal; it never blocks `sync`.

### `marketplace.json` source normalization

The manifest's top-level identity gives `marketplace` name + `owner`. Each plugin entry's
`source` may be:
- `"./plugins/x"` or `"./external_plugins/x"` → same repo as the marketplace; `repo` = the
  marketplace's own `owner/repo`.
- `{ "source": "github", "repo": "owner/repo" }` → still installed via the marketplace, so
  `repo` stays the marketplace's `owner/repo`.
- `{ "source": "git-subdir"|"url", "url": "https://…" }` → same; marketplace repo wins.

So normalization only needs the marketplace repo (from the seed we fetched) + each plugin's
`name`, `description`, `category`. Plugin `source` shapes are parsed only enough to not error.

### Fetch strategy (metadata-only, no HTTP crate)

For each seed `owner/repo`:
1. If a clone exists at `~/.claude/plugins/marketplaces/<name>/.claude-plugin/marketplace.json`,
   read it directly (fast path, already on disk for installed marketplaces).
2. Otherwise sparse+shallow fetch just the manifest into
   `~/.claude-profiles/.index-cache/repos/<owner--repo>/`:
   ```
   git clone --depth 1 --filter=blob:none --sparse <clone_url> <dest>
   git -C <dest> sparse-checkout set .claude-plugin
   ```
   Then read `<dest>/.claude-plugin/marketplace.json`.

New `GitCli` method: `sparse_fetch(&self, url: &str, dest: &Path, subpath: &str)`. Real impl
shells out to `git` as above; the existing mock `GitCli` in tests records calls and can be
fed a canned manifest via a fixture dir.

## Data flow

```
seeds (embedded ∪ installed ∪ user file)
      │  sync
      ▼
per-repo marketplace.json  ──model::normalize──►  Vec<IndexEntry>
      │
      ▼
~/.claude-profiles/.index-cache/index.json
      │  find <query>   (offline)
      ▼
search::rank  ──►  ranked IndexEntry  ──►  human table | --json
```

## Error handling

- A seed that fails to clone/read/parse → `eprintln!` warning, skip, continue. Sync reports
  `indexed N plugins from M marketplaces (K skipped)`.
- `find` with a missing index → auto-run `sync` once, then search. With `--sync`/`--refresh-seeds`
  it rebuilds first regardless.
- `find` offline with a stale-but-present index → search it, print the index's `generated_at`
  so staleness is visible. Never fails just because the network is down.
- Empty query without `--sync`/`--refresh-seeds` → usage error.

## Testing (TDD)

Pure units (no network), fixture-driven:
- `search::rank` — ordering: exact name match > name substring > description match; multi-term
  coverage beats single-term; `--marketplace` filter; `--limit`.
- `model::normalize` — every `source` shape above; missing `description`/`category`; a
  malformed entry is skipped without failing the file.
- `seeds` — union + case-insensitive dedup across the three sources; parsing
  `known_marketplaces.json` github vs git-url shapes; comment/blank-line handling in the user
  file.
- `fetch` — mock `GitCli`: local-clone fast path vs sparse-fetch path selection; a fetch error
  is surfaced as skip, not panic.
- `commands::find` — end-to-end over a temp `HOME` with a fixture index: human + `--json`
  output shape.

Follows the repo's existing inline `#[cfg(test)] mod tests` convention and mock `GitCli`.

## Files touched

- New: `src/index/mod.rs`, `seeds.rs`, `model.rs`, `fetch.rs`, `search.rs`,
  `default_seeds.rs`; `src/commands/find.rs`.
- Edit: `src/main.rs` (enum + match + `mod index;`), `src/commands/mod.rs`,
  `src/git.rs` (`sparse_fetch` on `GitCli` + mock), `src/fs_paths.rs` (index-cache path helper).
- Docs: `docs/commands.md` (`find`), `README.md` (mention discovery), a short note in
  `docs/profiles.md` linking authoring → discovery.

## Open items intentionally deferred (YAGNI)

- Full-text skill/command search (on-demand deep clone) — revisit only if metadata proves
  insufficient.
- A `--profile-snippet` flag emitting ready-to-paste JSON — nice-to-have after the core lands.
- Ranking by popularity/stars — needs a data source we don't have offline.
