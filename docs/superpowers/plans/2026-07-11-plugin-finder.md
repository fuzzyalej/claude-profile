# `claude-profile find` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `claude-profile find <query>` subcommand that searches a local, offline index of plugins across many marketplaces and returns profile-ready `plugin@marketplace` ids.

**Architecture:** A new `src/index/` module with five focused units — `model` (parse/normalize a marketplace manifest), `search` (pure ranking), `seeds` (marketplace repo list), `fetch` (obtain a manifest via local clone or sparse git fetch), and `mod` (sync orchestration + persist/load). A thin `commands::find` wires them to the CLI. Metadata-only: the index holds plugin name + description + category, not skill bodies.

**Tech Stack:** Rust, `clap` (derive), `serde`/`serde_json`, `anyhow`. Network only via shelling out to `git` through the existing `GitCli` trait — no HTTP crate.

## Global Constraints

- No new crate dependencies beyond the existing `clap`, `serde`, `serde_json`, `anyhow`. Network I/O is done by shelling out to `git` via `GitCli`.
- Follow the repo's inline `#[cfg(test)] mod tests` convention; tests are pure/fixture-driven, never hit the network.
- `IndexEntry.repo` is always the **marketplace** repo (`owner/repo`), never a plugin's upstream `source`.
- Index cache lives under `~/.claude-profiles/.index-cache/`; nothing generated is committed to git.
- Sync is fault-tolerant: a failing marketplace warns and is skipped, never aborts the run.
- Spec: `docs/superpowers/specs/2026-07-11-plugin-finder-design.md`.

---

### Task 1: Cache/seed path helpers on `Paths`

**Files:**
- Modify: `src/fs_paths.rs`

**Interfaces:**
- Produces: `Paths::index_cache_dir() -> PathBuf`, `Paths::index_file() -> PathBuf`, `Paths::marketplaces_seed_file() -> PathBuf`, `Paths::index_repos_dir() -> PathBuf`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/fs_paths.rs`:

```rust
    #[test]
    fn derives_index_paths_from_home() {
        let p = Paths::from_home(PathBuf::from("/h"));
        assert_eq!(p.index_cache_dir(), PathBuf::from("/h/.claude-profiles/.index-cache"));
        assert_eq!(p.index_file(), PathBuf::from("/h/.claude-profiles/.index-cache/index.json"));
        assert_eq!(p.index_repos_dir(), PathBuf::from("/h/.claude-profiles/.index-cache/repos"));
        assert_eq!(p.marketplaces_seed_file(), PathBuf::from("/h/.claude-profiles/marketplaces.txt"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test fs_paths::tests::derives_index_paths_from_home`
Expected: FAIL — no method `index_cache_dir`.

- [ ] **Step 3: Write minimal implementation**

Add to `impl Paths` in `src/fs_paths.rs`:

```rust
    pub fn index_cache_dir(&self) -> PathBuf {
        self.user_profiles_dir().join(".index-cache")
    }

    pub fn index_file(&self) -> PathBuf {
        self.index_cache_dir().join("index.json")
    }

    pub fn index_repos_dir(&self) -> PathBuf {
        self.index_cache_dir().join("repos")
    }

    pub fn marketplaces_seed_file(&self) -> PathBuf {
        self.user_profiles_dir().join("marketplaces.txt")
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test fs_paths::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/fs_paths.rs
git commit -m "feat(find): add index-cache path helpers"
```

---

### Task 2: `IndexEntry` model + manifest normalization

**Files:**
- Create: `src/index/mod.rs`
- Create: `src/index/model.rs`
- Modify: `src/main.rs` (add `mod index;`)

**Interfaces:**
- Produces:
  - `pub struct IndexEntry { pub plugin: String, pub marketplace: String, pub repo: String, pub description: String, pub category: Option<String> }` (derives `Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize`).
  - `pub fn normalize_manifest(json: &str, repo: &str) -> anyhow::Result<Vec<IndexEntry>>` — parses a `marketplace.json` string; `repo` is the seed `owner/repo` the manifest came from. Uses the manifest's own top-level `name` as `marketplace`, falling back to the repo's trailing path segment. Skips malformed plugin entries (missing `name`) without failing.

- [ ] **Step 1: Write the failing test**

Create `src/index/model.rs`:

```rust
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
      "name": "official",
      "plugins": [
        { "name": "pyright-lsp", "description": "Python LSP", "source": "./plugins/pyright-lsp", "category": "language" },
        { "name": "github", "description": "GitHub MCP", "source": { "source": "url", "url": "https://x/y.git" } },
        { "description": "no name, skipped" }
      ]
    }"#;

    #[test]
    fn normalizes_all_source_shapes_and_skips_nameless() {
        let entries = normalize_manifest(MANIFEST, "anthropics/claude-plugins-official").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], IndexEntry {
            plugin: "pyright-lsp".into(),
            marketplace: "official".into(),
            repo: "anthropics/claude-plugins-official".into(),
            description: "Python LSP".into(),
            category: Some("language".into()),
        });
        // marketplace repo wins even when the plugin points elsewhere:
        assert_eq!(entries[1].repo, "anthropics/claude-plugins-official");
        assert_eq!(entries[1].category, None);
    }

    #[test]
    fn falls_back_to_repo_segment_when_manifest_unnamed() {
        let j = r#"{ "plugins": [ { "name": "p", "description": "d" } ] }"#;
        let entries = normalize_manifest(j, "obra/superpowers-marketplace").unwrap();
        assert_eq!(entries[0].marketplace, "superpowers-marketplace");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

First create `src/index/mod.rs` with `pub mod model;` and add `mod index;` to `src/main.rs` (after the other `mod` lines).
Run: `cargo test index::model`
Expected: FAIL — `normalize_manifest`/`IndexEntry` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/index/model.rs` (above the `tests` module):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub plugin: String,
    pub marketplace: String,
    pub repo: String,
    pub description: String,
    pub category: Option<String>,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    plugins: Vec<PluginEntry>,
}

#[derive(Deserialize)]
struct PluginEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

pub fn normalize_manifest(json: &str, repo: &str) -> anyhow::Result<Vec<IndexEntry>> {
    let m: Manifest = serde_json::from_str(json)?;
    let marketplace = m.name.unwrap_or_else(|| {
        repo.rsplit('/').next().unwrap_or(repo).to_string()
    });
    let entries = m
        .plugins
        .into_iter()
        .filter_map(|p| {
            let plugin = p.name?;
            Some(IndexEntry {
                plugin,
                marketplace: marketplace.clone(),
                repo: repo.to_string(),
                description: p.description.unwrap_or_default(),
                category: p.category,
            })
        })
        .collect();
    Ok(entries)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test index::model`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/index/mod.rs src/index/model.rs src/main.rs
git commit -m "feat(find): IndexEntry model + manifest normalization"
```

---

### Task 3: Ranking / search

**Files:**
- Create: `src/index/search.rs`
- Modify: `src/index/mod.rs` (add `pub mod search;`)

**Interfaces:**
- Consumes: `crate::index::model::IndexEntry`.
- Produces: `pub fn rank<'a>(entries: &'a [IndexEntry], query: &str, marketplace: Option<&str>, limit: usize) -> Vec<&'a IndexEntry>`. Case-insensitive. Score per entry = sum over query terms of: 6 if term == plugin name, 3 if plugin name contains term, 2 if description contains term, 1 if category contains term. Entries with score 0 are dropped. `marketplace` filters to entries whose `marketplace` equals it (case-insensitive) before ranking. Ties broken by plugin name ascending. Truncated to `limit`.

- [ ] **Step 1: Write the failing test**

Create `src/index/search.rs`:

```rust
use crate::index::model::IndexEntry;

#[cfg(test)]
mod tests {
    use super::*;

    fn e(plugin: &str, mkt: &str, desc: &str, cat: Option<&str>) -> IndexEntry {
        IndexEntry {
            plugin: plugin.into(),
            marketplace: mkt.into(),
            repo: format!("owner/{mkt}"),
            description: desc.into(),
            category: cat.map(|c| c.into()),
        }
    }

    #[test]
    fn ranks_name_hits_above_description_hits() {
        let entries = vec![
            e("django-helper", "a", "web framework", None),
            e("logger", "a", "python logging utility", None),
            e("python", "a", "the python toolkit", None),
        ];
        let got = rank(&entries, "python", None, 10);
        assert_eq!(got[0].plugin, "python");     // exact name
        assert_eq!(got[1].plugin, "logger");     // description hit
        assert_eq!(got.len(), 2);                // django-helper scores 0, dropped
    }

    #[test]
    fn multi_term_and_marketplace_filter_and_limit() {
        let entries = vec![
            e("a", "mkt1", "backend architecture guide", None),
            e("b", "mkt2", "backend only", None),
            e("c", "mkt1", "architecture only", None),
        ];
        let got = rank(&entries, "backend architecture", Some("mkt1"), 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].plugin, "a");          // both terms, in mkt1
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Add `pub mod search;` to `src/index/mod.rs`.
Run: `cargo test index::search`
Expected: FAIL — `rank` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/index/search.rs`:

```rust
fn score(entry: &IndexEntry, terms: &[String]) -> u32 {
    let name = entry.plugin.to_lowercase();
    let desc = entry.description.to_lowercase();
    let cat = entry.category.as_deref().unwrap_or("").to_lowercase();
    let mut total = 0;
    for t in terms {
        if name == *t {
            total += 6;
        } else if name.contains(t.as_str()) {
            total += 3;
        }
        if desc.contains(t.as_str()) {
            total += 2;
        }
        if cat.contains(t.as_str()) {
            total += 1;
        }
    }
    total
}

pub fn rank<'a>(
    entries: &'a [IndexEntry],
    query: &str,
    marketplace: Option<&str>,
    limit: usize,
) -> Vec<&'a IndexEntry> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    let mkt = marketplace.map(|m| m.to_lowercase());
    let mut scored: Vec<(u32, &IndexEntry)> = entries
        .iter()
        .filter(|e| match &mkt {
            Some(m) => e.marketplace.to_lowercase() == *m,
            None => true,
        })
        .map(|e| (score(e, &terms), e))
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.plugin.cmp(&b.1.plugin)));
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test index::search`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/index/search.rs src/index/mod.rs
git commit -m "feat(find): pure ranking over index entries"
```

---

### Task 4: Seed list resolution

**Files:**
- Create: `src/index/default_seeds.rs`
- Create: `src/index/seeds.rs`
- Modify: `src/index/mod.rs` (add `pub mod default_seeds; pub mod seeds;`)

**Interfaces:**
- Consumes: `crate::fs_paths::Paths`.
- Produces:
  - `default_seeds::DEFAULT_SEEDS: &[&str]` — embedded `owner/repo` marketplace list.
  - `seeds::repo_from_known_marketplace(source_repo: Option<&str>, source_url: Option<&str>) -> Option<String>` — normalize a `known_marketplaces.json` source to `owner/repo`.
  - `seeds::parse_user_file(contents: &str) -> Vec<String>` — one `owner/repo` per non-blank, non-`#` line.
  - `seeds::resolve(paths: &Paths) -> Vec<String>` — union of DEFAULT_SEEDS + installed marketplaces (`~/.claude/plugins/known_marketplaces.json`) + user file, deduped case-insensitively on `owner/repo`, sorted. Missing files are treated as empty.

- [ ] **Step 1: Write the failing test**

Create `src/index/seeds.rs`:

```rust
use crate::fs_paths::Paths;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_file_ignoring_comments_and_blanks() {
        let c = "# header\n\nowner/one\n  owner/two  \n# owner/three\n";
        assert_eq!(parse_user_file(c), vec!["owner/one", "owner/two"]);
    }

    #[test]
    fn normalizes_known_marketplace_sources() {
        assert_eq!(repo_from_known_marketplace(Some("fuzzyalej/diagon-alley"), None).as_deref(),
                   Some("fuzzyalej/diagon-alley"));
        assert_eq!(
            repo_from_known_marketplace(None, Some("git@ssh.dev.azure.com:v3/Org/Proj/repo")).as_deref(),
            Some("Proj/repo")
        );
        assert_eq!(
            repo_from_known_marketplace(None, Some("https://github.com/a/b.git")).as_deref(),
            Some("a/b")
        );
    }

    #[test]
    fn resolve_unions_and_dedups() {
        let tmp = std::env::temp_dir().join(format!("cpf-seeds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".claude-profiles")).unwrap();
        std::fs::create_dir_all(tmp.join(".claude/plugins")).unwrap();
        std::fs::write(tmp.join(".claude-profiles/marketplaces.txt"), "me/mine\nobra/superpowers-marketplace\n").unwrap();
        std::fs::write(tmp.join(".claude/plugins/known_marketplaces.json"),
            r#"{ "x": { "source": { "source": "github", "repo": "priv/internal" } } }"#).unwrap();
        let paths = Paths::from_home(tmp.clone());
        let seeds = resolve(&paths);
        assert!(seeds.contains(&"me/mine".to_string()));
        assert!(seeds.contains(&"priv/internal".to_string()));
        assert!(seeds.contains(&"anthropics/claude-plugins-official".to_string())); // from defaults
        // dedup: superpowers appears in both defaults and user file, once only
        assert_eq!(seeds.iter().filter(|s| s.contains("superpowers-marketplace")).count(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Create `src/index/default_seeds.rs` with a starter list (guaranteed-real marketplace repos; expand via harvest in Step 3b):

```rust
/// Curated marketplace repos, harvested from community aggregators. Extend via
/// `find --refresh-seeds` or by editing `~/.claude-profiles/marketplaces.txt`.
pub const DEFAULT_SEEDS: &[&str] = &[
    "anthropics/claude-plugins-official",
    "obra/superpowers-marketplace",
    "fuzzyalej/diagon-alley",
];
```

Add `pub mod default_seeds; pub mod seeds;` to `src/index/mod.rs`.
Run: `cargo test index::seeds`
Expected: FAIL — `resolve`/`parse_user_file`/`repo_from_known_marketplace` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/index/seeds.rs`:

```rust
use crate::index::default_seeds::DEFAULT_SEEDS;
use std::collections::BTreeSet;

pub fn parse_user_file(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

/// Extract trailing `owner/repo` (last two path segments) from an ssh/https git URL.
fn owner_repo_from_url(url: &str) -> Option<String> {
    let tail = url.rsplit(['/', ':']).take(2).collect::<Vec<_>>();
    if tail.len() < 2 {
        return None;
    }
    let repo = tail[0].trim_end_matches(".git");
    let owner = tail[1];
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

pub fn repo_from_known_marketplace(source_repo: Option<&str>, source_url: Option<&str>) -> Option<String> {
    if let Some(r) = source_repo {
        if r.contains('/') {
            return Some(r.to_string());
        }
    }
    source_url.and_then(owner_repo_from_url)
}

fn installed_repos(paths: &Paths) -> Vec<String> {
    let path = paths.home.join(".claude/plugins/known_marketplaces.json");
    let Ok(body) = std::fs::read_to_string(&path) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return vec![] };
    let Some(obj) = v.as_object() else { return vec![] };
    obj.values()
        .filter_map(|m| {
            let src = m.get("source")?;
            let repo = src.get("repo").and_then(|r| r.as_str());
            let url = src.get("url").and_then(|u| u.as_str());
            repo_from_known_marketplace(repo, url)
        })
        .collect()
}

pub fn resolve(paths: &Paths) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    let user = std::fs::read_to_string(paths.marketplaces_seed_file())
        .map(|c| parse_user_file(&c))
        .unwrap_or_default();
    let all = DEFAULT_SEEDS
        .iter()
        .map(|s| s.to_string())
        .chain(installed_repos(paths))
        .chain(user);
    for repo in all {
        let key = repo.to_lowercase();
        if seen.insert(key) {
            out.push(repo);
        }
    }
    out.sort();
    out
}
```

- [ ] **Step 3b: Harvest the real default seed list**

Populate `DEFAULT_SEEDS` with the actual community marketplaces. During implementation, fetch the aggregators and extract the `owner/repo` of each *marketplace* they list (not the aggregator repos themselves):
- `https://github.com/ComposioHQ/awesome-claude-plugins` (README)
- `https://github.com/quemsah/awesome-claude-plugins` (data/README)
- `https://claudemarketplaces.com/`

Use WebFetch to pull each, extract `github.com/<owner>/<repo>` entries that are marketplaces (repos containing `.claude-plugin/marketplace.json`), dedup, and replace the starter list in `src/index/default_seeds.rs` (keep the three starters). Target ~50–150 entries. If harvest is inconclusive for a repo, omit it — `resolve` tolerates a short list.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test index::seeds`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/index/seeds.rs src/index/default_seeds.rs src/index/mod.rs
git commit -m "feat(find): seed resolution (defaults + installed + user file)"
```

---

### Task 5: `sparse_fetch` on `GitCli`

**Files:**
- Modify: `src/git.rs` (trait method + `RealGit` impl + test-mock in `git.rs` if any)
- Modify: `src/pack.rs` (extend `MockGit` with the new method)

**Interfaces:**
- Produces: `GitCli::sparse_fetch(&self, url: &str, dest: &Path, subpath: &str) -> anyhow::Result<()>` — shallow, blobless, sparse clone of only `subpath`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/git.rs` (real command not run in unit test — assert the method exists via the mock in pack.rs; here just document intent with a compile-level test):

```rust
    #[test]
    fn sparse_fetch_is_on_the_trait() {
        // Compile-time proof the method exists with the expected signature.
        fn _assert<G: GitCli>(g: &G, p: &std::path::Path) {
            let _ = g.sparse_fetch("https://x/y.git", p, ".claude-plugin");
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib git::`
Expected: FAIL — `sparse_fetch` not a member of `GitCli`.

- [ ] **Step 3: Write minimal implementation**

In `src/git.rs`, add to the `trait GitCli` block:

```rust
    /// Shallow, blobless, sparse clone of only `subpath` from `url` into `dest`.
    /// Used to fetch a single `.claude-plugin/marketplace.json` cheaply.
    fn sparse_fetch(&self, url: &str, dest: &Path, subpath: &str) -> anyhow::Result<()>;
```

Add to `impl GitCli for RealGit`:

```rust
    fn sparse_fetch(&self, url: &str, dest: &Path, subpath: &str) -> anyhow::Result<()> {
        let dest_s = dest.to_string_lossy();
        self.run(
            &["clone", "--depth", "1", "--filter=blob:none", "--sparse", url, &dest_s],
            None,
        )?;
        self.run(&["-C", &dest_s, "sparse-checkout", "set", subpath], None)?;
        Ok(())
    }
```

In `src/pack.rs`, add to `impl GitCli for MockGit` (record the call so tests can assert it):

```rust
        fn sparse_fetch(&self, _u: &str, _d: &std::path::Path, _s: &str) -> anyhow::Result<()> {
            Ok(())
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib`
Expected: PASS (all existing tests + new one).

- [ ] **Step 5: Commit**

```bash
git add src/git.rs src/pack.rs
git commit -m "feat(find): sparse_fetch on GitCli for cheap manifest fetch"
```

---

### Task 6: Fetch a marketplace manifest

**Files:**
- Create: `src/index/fetch.rs`
- Modify: `src/index/mod.rs` (add `pub mod fetch;`)

**Interfaces:**
- Consumes: `GitCli`, `Paths`, `model::normalize_manifest`.
- Produces: `fetch::manifest_json<G: GitCli>(git: &G, paths: &Paths, repo: &str) -> anyhow::Result<String>` — returns the raw `marketplace.json` for `repo`. Fast path: if `~/.claude/plugins/marketplaces/<name>/.claude-plugin/marketplace.json` exists (where `<name>` is the repo's trailing segment), read it. Else `sparse_fetch` into `paths.index_repos_dir().join(<owner--repo>)` and read `<dest>/.claude-plugin/marketplace.json`. Helper `fetch::cache_dir_name(repo: &str) -> String` maps `owner/repo` → `owner--repo`.

- [ ] **Step 1: Write the failing test**

Create `src/index/fetch.rs`:

```rust
use crate::fs_paths::Paths;
use crate::git::GitCli;
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeGit { calls: RefCell<Vec<(String, String)>> }
    impl GitCli for FakeGit {
        fn clone(&self, _u: &str, _d: &Path) -> anyhow::Result<()> { Ok(()) }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok("sha".into()) }
        fn checkout(&self, _r: &Path, _g: &str) -> anyhow::Result<()> { Ok(()) }
        fn is_repo(&self, _r: &Path) -> bool { true }
        fn sparse_fetch(&self, u: &str, d: &Path, _s: &str) -> anyhow::Result<()> {
            self.calls.borrow_mut().push((u.into(), d.to_string_lossy().into()));
            // simulate the fetched manifest landing on disk
            std::fs::create_dir_all(d.join(".claude-plugin")).unwrap();
            std::fs::write(d.join(".claude-plugin/marketplace.json"),
                r#"{ "name": "m", "plugins": [] }"#).unwrap();
            Ok(())
        }
    }

    #[test]
    fn cache_dir_name_flattens_slash() {
        assert_eq!(cache_dir_name("owner/repo"), "owner--repo");
    }

    #[test]
    fn fetches_via_sparse_when_no_local_clone() {
        let tmp = std::env::temp_dir().join(format!("cpf-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let paths = Paths::from_home(tmp.clone());
        let git = FakeGit { calls: RefCell::new(vec![]) };
        let json = manifest_json(&git, &paths, "owner/repo").unwrap();
        assert!(json.contains("\"name\": \"m\""));
        assert_eq!(git.calls.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Add `pub mod fetch;` to `src/index/mod.rs`.
Run: `cargo test index::fetch`
Expected: FAIL — `manifest_json`/`cache_dir_name` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/index/fetch.rs`:

```rust
pub fn cache_dir_name(repo: &str) -> String {
    repo.replace('/', "--")
}

fn marketplace_name(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

pub fn manifest_json<G: GitCli>(git: &G, paths: &Paths, repo: &str) -> anyhow::Result<String> {
    let local = paths
        .home
        .join(".claude/plugins/marketplaces")
        .join(marketplace_name(repo))
        .join(".claude-plugin/marketplace.json");
    if local.exists() {
        return Ok(std::fs::read_to_string(local)?);
    }
    let dest = paths.index_repos_dir().join(cache_dir_name(repo));
    let _ = std::fs::remove_dir_all(&dest);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = format!("https://github.com/{repo}.git");
    git.sparse_fetch(&url, &dest, ".claude-plugin")?;
    let manifest = dest.join(".claude-plugin/marketplace.json");
    Ok(std::fs::read_to_string(manifest)?)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test index::fetch`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/index/fetch.rs src/index/mod.rs
git commit -m "feat(find): fetch marketplace manifest (local clone or sparse)"
```

---

### Task 7: Sync orchestration + persist/load

**Files:**
- Modify: `src/index/mod.rs`

**Interfaces:**
- Consumes: `seeds::resolve`, `fetch::manifest_json`, `model::{IndexEntry, normalize_manifest}`, `Paths`, `GitCli`.
- Produces:
  - `pub struct Index { pub generated_at: String, pub entries: Vec<IndexEntry> }` (Serialize/Deserialize).
  - `pub struct SyncReport { pub marketplaces: usize, pub skipped: usize, pub plugins: usize }`.
  - `pub fn sync<G: GitCli>(git: &G, paths: &Paths) -> anyhow::Result<SyncReport>` — resolve seeds, fetch+normalize each (warn+skip failures to stderr), write `paths.index_file()` as JSON, return counts.
  - `pub fn load(paths: &Paths) -> anyhow::Result<Index>` — read + parse `paths.index_file()`.

- [ ] **Step 1: Write the failing test**

Add a `tests` module at the bottom of `src/index/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_paths::Paths;
    use crate::git::GitCli;
    use std::cell::RefCell;
    use std::path::Path;

    struct OneMarketplaceGit;
    impl GitCli for OneMarketplaceGit {
        fn clone(&self, _u: &str, _d: &Path) -> anyhow::Result<()> { Ok(()) }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok("s".into()) }
        fn checkout(&self, _r: &Path, _g: &str) -> anyhow::Result<()> { Ok(()) }
        fn is_repo(&self, _r: &Path) -> bool { true }
        fn sparse_fetch(&self, _u: &str, d: &Path, _s: &str) -> anyhow::Result<()> {
            std::fs::create_dir_all(d.join(".claude-plugin")).unwrap();
            std::fs::write(d.join(".claude-plugin/marketplace.json"),
                r#"{ "name": "m", "plugins": [ { "name": "p", "description": "d" } ] }"#).unwrap();
            Ok(())
        }
    }

    #[test]
    fn sync_then_load_roundtrips() {
        let tmp = std::env::temp_dir().join(format!("cpf-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // isolate seeds: empty user file, no installed marketplaces -> just DEFAULT_SEEDS,
        // but network is mocked so every default resolves to the same fixture manifest.
        std::fs::create_dir_all(tmp.join(".claude-profiles")).unwrap();
        std::fs::write(tmp.join(".claude-profiles/marketplaces.txt"), "solo/mkt\n").unwrap();
        let paths = Paths::from_home(tmp.clone());
        let git = OneMarketplaceGit;
        let report = sync(&git, &paths).unwrap();
        assert!(report.plugins >= 1);
        let idx = load(&paths).unwrap();
        assert!(idx.entries.iter().any(|e| e.plugin == "p"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test index::tests::sync_then_load_roundtrips`
Expected: FAIL — `sync`/`load`/`Index` not found.

- [ ] **Step 3: Write minimal implementation**

Add to the top of `src/index/mod.rs` (below the `pub mod` lines):

```rust
use crate::fs_paths::Paths;
use crate::git::GitCli;
use model::IndexEntry;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Index {
    pub generated_at: String,
    pub entries: Vec<IndexEntry>,
}

pub struct SyncReport {
    pub marketplaces: usize,
    pub skipped: usize,
    pub plugins: usize,
}

pub fn sync<G: GitCli>(git: &G, paths: &Paths) -> anyhow::Result<SyncReport> {
    let seeds = seeds::resolve(paths);
    let mut entries: Vec<IndexEntry> = Vec::new();
    let mut ok = 0usize;
    let mut skipped = 0usize;
    for repo in &seeds {
        match fetch::manifest_json(git, paths, repo)
            .and_then(|j| model::normalize_manifest(&j, repo))
        {
            Ok(mut es) => {
                ok += 1;
                entries.append(&mut es);
            }
            Err(e) => {
                skipped += 1;
                eprintln!("warning: skipped marketplace {repo}: {e}");
            }
        }
    }
    let index = Index {
        generated_at: now_rfc3339(),
        entries,
    };
    std::fs::create_dir_all(paths.index_cache_dir())?;
    std::fs::write(paths.index_file(), serde_json::to_string_pretty(&index)?)?;
    Ok(SyncReport { marketplaces: ok, skipped, plugins: index.entries.len() })
}

pub fn load(paths: &Paths) -> anyhow::Result<Index> {
    let body = std::fs::read_to_string(paths.index_file())?;
    Ok(serde_json::from_str(&body)?)
}

fn now_rfc3339() -> String {
    // No chrono dependency: seconds since epoch is enough to show staleness.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch:{secs}")
}
```

Ensure `pub mod default_seeds; pub mod fetch; pub mod model; pub mod search; pub mod seeds;` are all declared at the top of `src/index/mod.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test index::`
Expected: PASS (all index tests).

- [ ] **Step 5: Commit**

```bash
git add src/index/mod.rs
git commit -m "feat(find): sync orchestration + index persist/load"
```

---

### Task 8: `find` command + CLI wiring

**Files:**
- Create: `src/commands/find.rs`
- Modify: `src/commands/mod.rs` (add `pub mod find;`)
- Modify: `src/main.rs` (enum variant + match arm)

**Interfaces:**
- Consumes: `index::{sync, load, search::rank, SyncReport}`, `Paths`, `git::RealGit`.
- Produces: `commands::find::run(paths: &Paths, query: &[String], sync_flag: bool, refresh_seeds: bool, json: bool, limit: Option<usize>, marketplace: Option<&str>) -> anyhow::Result<i32>`.

- [ ] **Step 1: Write the failing test**

Create `src/commands/find.rs`:

```rust
use crate::fs_paths::Paths;
use crate::index;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::model::IndexEntry;

    #[test]
    fn render_human_lists_id_and_repo() {
        let entries = vec![IndexEntry {
            plugin: "pyright-lsp".into(),
            marketplace: "official".into(),
            repo: "anthropics/claude-plugins-official".into(),
            description: "Python LSP".into(),
            category: None,
        }];
        let refs: Vec<&IndexEntry> = entries.iter().collect();
        let out = render_human(&refs);
        assert!(out.contains("pyright-lsp@official"));
        assert!(out.contains("anthropics/claude-plugins-official"));
        assert!(out.contains("Python LSP"));
    }

    #[test]
    fn render_json_is_array_of_entries() {
        let entries = vec![IndexEntry {
            plugin: "p".into(), marketplace: "m".into(), repo: "o/m".into(),
            description: "d".into(), category: None,
        }];
        let refs: Vec<&IndexEntry> = entries.iter().collect();
        let out = render_json(&refs).unwrap();
        assert!(out.trim_start().starts_with('['));
        assert!(out.contains("\"plugin\": \"p\""));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Add `pub mod find;` to `src/commands/mod.rs`.
Run: `cargo test commands::find`
Expected: FAIL — `render_human`/`render_json` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/commands/find.rs`:

```rust
use crate::index::model::IndexEntry;

fn render_human(entries: &[&IndexEntry]) -> String {
    let mut s = String::new();
    for e in entries {
        s.push_str(&format!("{}@{}   {}\n", e.plugin, e.marketplace, e.description));
        s.push_str(&format!("    repo: {}\n", e.repo));
    }
    s
}

fn render_json(entries: &[&IndexEntry]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(entries)?)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    paths: &Paths,
    query: &[String],
    sync_flag: bool,
    refresh_seeds: bool,
    json: bool,
    limit: Option<usize>,
    marketplace: Option<&str>,
) -> anyhow::Result<i32> {
    let git = crate::git::RealGit;
    if refresh_seeds {
        eprintln!("note: --refresh-seeds harvesting not yet implemented; using existing seeds");
    }
    let index_missing = !paths.index_file().exists();
    if sync_flag || refresh_seeds || index_missing {
        if index_missing && !sync_flag && !refresh_seeds {
            eprintln!("no index found; syncing (this fetches marketplace manifests)…");
        }
        let r = index::sync(&git, paths)?;
        eprintln!(
            "indexed {} plugins from {} marketplaces ({} skipped)",
            r.plugins, r.marketplaces, r.skipped
        );
    }
    if query.is_empty() {
        if sync_flag || refresh_seeds {
            return Ok(0); // sync-only invocation
        }
        anyhow::bail!("give a search query, e.g. `claude-profile find python`");
    }
    let idx = index::load(paths)?;
    let q = query.join(" ");
    let hits = index::search::rank(&idx.entries, &q, marketplace, limit.unwrap_or(20));
    if json {
        println!("{}", render_json(&hits)?);
    } else if hits.is_empty() {
        eprintln!("no matches for '{q}' (index generated {})", idx.generated_at);
    } else {
        print!("{}", render_human(&hits));
    }
    Ok(0)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test commands::find`
Expected: PASS (2 tests).

- [ ] **Step 5: Wire into the CLI**

In `src/main.rs`, add to `enum Command`:

```rust
    /// Search a local index of plugins across marketplaces. Returns profile-ready
    /// `plugin@marketplace` ids. First run auto-syncs; use --sync to rebuild.
    Find {
        query: Vec<String>,
        #[arg(long)]
        sync: bool,
        #[arg(long)]
        refresh_seeds: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        marketplace: Option<String>,
    },
```

In the `match cli.command` block in `run()`, add:

```rust
        Some(Command::Find { query, sync, refresh_seeds, json, limit, marketplace }) => {
            commands::find::run(&paths, &query, sync, refresh_seeds, json, limit, marketplace.as_deref())
        }
```

- [ ] **Step 6: Verify end-to-end build + tests**

Run: `cargo build && cargo test`
Expected: builds clean, all tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/commands/find.rs src/commands/mod.rs src/main.rs
git commit -m "feat(find): wire find command into CLI"
```

---

### Task 9: Documentation

**Files:**
- Modify: `docs/commands.md`
- Modify: `README.md`
- Modify: `docs/profiles.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Document `find` in `docs/commands.md`**

Add a `### find` section describing: purpose (discover plugins across marketplaces), the CLI flags (`--sync`, `--refresh-seeds`, `--json`, `--limit`, `--marketplace`), where the index and seed file live (`~/.claude-profiles/.index-cache/index.json`, `~/.claude-profiles/marketplaces.txt`), and that output is copy-paste-ready `plugin@marketplace` ids for a profile's `plugins`/`marketplaces`.

- [ ] **Step 2: Mention discovery in `README.md`**

Add a short bullet/section under usage: "Discover plugins: `claude-profile find python` searches a local cross-marketplace index; add your own marketplaces in `~/.claude-profiles/marketplaces.txt`."

- [ ] **Step 3: Link discovery from `docs/profiles.md`**

In the authoring intro, add one line: "To find candidate plugins to add, use [`find`](commands.md#find)."

- [ ] **Step 4: Commit**

```bash
git add docs/commands.md README.md docs/profiles.md
git commit -m "docs(find): document plugin discovery command"
```

---

## Self-Review

**Spec coverage:** CLI surface (Task 8), metadata-only model (Task 2), seed union w/ embedded+installed+user (Task 4), sparse fetch (Tasks 5–6), sync/persist/load (Task 7), ranking incl. marketplace filter + limit (Task 3), fault-tolerant skip (Task 7), offline load w/ staleness via `generated_at` (Tasks 7–8), docs (Task 9). `--refresh-seeds` is accepted but stubbed with a clear notice (Task 8) — full harvester deferred per spec's "best-effort / non-fatal" framing; the one-time harvest is Task 4 Step 3b.

**Placeholder scan:** none — every code step has complete code. Task 4 Step 3b is a real research action (harvest) with a working committed fallback, not a placeholder.

**Type consistency:** `IndexEntry` fields (`plugin`, `marketplace`, `repo`, `description`, `category`) identical across Tasks 2/3/6/7/8. `sparse_fetch(url, dest, subpath)` signature identical in Tasks 5/6/7. `rank(entries, query, marketplace, limit)` identical in Tasks 3/8. `manifest_json(git, paths, repo)` identical in Tasks 6/7.
