# Vendored Plugin/Skill Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace claude-profile's dependency on `claude plugin install`/`marketplace add` (which writes into the user's shared `~/.claude`) with self-managed vendoring: each profile's plugins and skills are copied into a private, per-profile directory tree under `~/.claude-profiles/store/`, and launched via `--plugin-dir` instead of a `--settings enabledPlugins` override.

**Architecture:** `claude-profile` clones marketplace repos itself (reusing the existing `GitCli` trait), resolves each plugin's source from `.claude-plugin/marketplace.json`, and copies the resolved plugin/skill directory into `~/.claude-profiles/store/<profile-key>/vendor/<plugin-id>/`. Launch adds one `--plugin-dir` flag per vendored entry. Nothing is ever written to `~/.claude`.

**Tech Stack:** Rust, existing `GitCli`/`Lockfile`/`Profile` types, `serde_json`, `anyhow`. No new dependencies.

## Global Constraints

- Never write to `~/.claude` (settings.json, `~/.claude/plugins`, `~/.claude/skills`) from any claude-profile command.
- Never mutate a user's original skill folder in place — vendoring always copies.
- No cross-profile de-duplication of vendored plugin/skill code (each profile's `vendor/` is independent). Marketplace *clones* (the source claude-profile copies plugins out of) are shared/cached once per marketplace name, not per profile — this is a cache, not a dedup mechanism for vendored output.
- No auto-update of vendored code outside the existing `update` flow.
- Match the design spec at `docs/superpowers/specs/2026-07-15-vendored-plugin-isolation-design.md` exactly; if an implementation detail below conflicts with it, the spec wins and the plan should be corrected.

---

### Task 1: `.claude-plugin/marketplace.json` parsing

**Files:**
- Create: `src/vendor.rs`
- Modify: `src/main.rs` (add `mod vendor;`)

**Interfaces:**
- Produces: `pub enum PluginSource { RelativePath(String), ExternalRepo { repo: String } }`, `pub struct MarketplacePlugin { pub name: String, pub source: PluginSource }`, `pub fn parse_marketplace_json(body: &str) -> anyhow::Result<Vec<MarketplacePlugin>>`, `pub fn find_plugin<'a>(plugins: &'a [MarketplacePlugin], name: &str) -> anyhow::Result<&'a MarketplacePlugin>`.

- [ ] **Step 1: Write the failing tests**

```rust
// src/vendor.rs (bottom of file, #[cfg(test)] mod tests)
#[cfg(test)]
mod tests {
    use super::*;

    // Real shape from a marketplace.json in the wild: one relative-path
    // plugin, one external-repo plugin, in the same file.
    const REAL_MARKETPLACE_JSON: &str = r#"{
      "name": "diagon-alley",
      "owner": { "name": "AAN", "email": "aan@mjolner.com" },
      "metadata": { "description": "d", "version": "1.0.0" },
      "plugins": [
        {
          "name": "design-extractor",
          "description": "d",
          "source": "./skills/design-extractor"
        },
        {
          "name": "openpowers",
          "description": "d",
          "source": { "source": "github", "repo": "fuzzyalej/openpowers" }
        }
      ]
    }"#;

    #[test]
    fn parses_relative_path_and_external_repo_sources() {
        let plugins = parse_marketplace_json(REAL_MARKETPLACE_JSON).unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].name, "design-extractor");
        assert_eq!(plugins[0].source, PluginSource::RelativePath("./skills/design-extractor".into()));
        assert_eq!(plugins[1].name, "openpowers");
        assert_eq!(plugins[1].source, PluginSource::ExternalRepo { repo: "fuzzyalej/openpowers".into() });
    }

    #[test]
    fn find_plugin_locates_by_name() {
        let plugins = parse_marketplace_json(REAL_MARKETPLACE_JSON).unwrap();
        let found = find_plugin(&plugins, "openpowers").unwrap();
        assert_eq!(found.name, "openpowers");
    }

    #[test]
    fn find_plugin_errors_when_missing() {
        let plugins = parse_marketplace_json(REAL_MARKETPLACE_JSON).unwrap();
        assert!(find_plugin(&plugins, "nope").is_err());
    }

    #[test]
    fn errors_on_missing_plugins_array() {
        assert!(parse_marketplace_json(r#"{"name":"x"}"#).is_err());
    }

    #[test]
    fn errors_on_unrecognized_source_shape() {
        let json = r#"{"plugins":[{"name":"x","source":{"source":"gitlab","repo":"o/r"}}]}"#;
        assert!(parse_marketplace_json(json).is_err());
    }

    #[test]
    fn errors_on_plugin_missing_source() {
        let json = r#"{"plugins":[{"name":"x"}]}"#;
        assert!(parse_marketplace_json(json).is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib vendor::`
Expected: FAIL to compile (`vendor` module/types don't exist yet).

- [ ] **Step 3: Write the implementation**

```rust
// src/vendor.rs (top of file, above the tests module)
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum PluginSource {
    /// Path relative to the marketplace repo root, e.g. "./skills/foo".
    RelativePath(String),
    /// An externally-hosted plugin, referenced by `owner/repo`.
    ExternalRepo { repo: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketplacePlugin {
    pub name: String,
    pub source: PluginSource,
}

pub fn parse_marketplace_json(body: &str) -> anyhow::Result<Vec<MarketplacePlugin>> {
    let v: Value = serde_json::from_str(body)?;
    let arr = v
        .get("plugins")
        .and_then(|p| p.as_array())
        .ok_or_else(|| anyhow::anyhow!("marketplace.json missing 'plugins' array"))?;

    let mut out = Vec::new();
    for p in arr {
        let name = p
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow::anyhow!("plugin entry missing 'name': {p}"))?;
        let source = p
            .get("source")
            .ok_or_else(|| anyhow::anyhow!("plugin '{name}' missing 'source'"))?;
        let parsed = match source {
            Value::String(s) => PluginSource::RelativePath(s.clone()),
            Value::Object(o) => {
                let kind = o.get("source").and_then(|x| x.as_str());
                let repo = o.get("repo").and_then(|x| x.as_str());
                match (kind, repo) {
                    (Some("github"), Some(r)) => PluginSource::ExternalRepo { repo: r.to_string() },
                    _ => anyhow::bail!("plugin '{name}' has unrecognized source shape: {source}"),
                }
            }
            other => anyhow::bail!("plugin '{name}' has unrecognized source shape: {other}"),
        };
        out.push(MarketplacePlugin { name: name.to_string(), source: parsed });
    }
    Ok(out)
}

pub fn find_plugin<'a>(plugins: &'a [MarketplacePlugin], name: &str) -> anyhow::Result<&'a MarketplacePlugin> {
    plugins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| anyhow::anyhow!("plugin '{name}' not found in marketplace"))
}
```

Add `mod vendor;` to `src/main.rs`'s module list (alongside `mod claude;` etc — keep alphabetical: after `mod update;`... there is no `mod update;` at top level, it's `mod commands;`; insert `mod vendor;` after `mod resolve;` and before `mod spinner;`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib vendor::`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/vendor.rs src/main.rs
git commit -m "feat: parse marketplace.json plugin sources for vendoring"
```

---

### Task 2: Atomic directory copy + skill manifest generation

**Files:**
- Create: `src/vendor_fs.rs`
- Modify: `src/main.rs` (add `mod vendor_fs;`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `pub fn copy_dir_atomic(src: &Path, dest: &Path) -> anyhow::Result<()>`, `pub fn ensure_manifest(dir: &Path, skill_name: &str) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing tests**

```rust
// src/vendor_fs.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn copies_nested_directory_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("top.txt"), "top").unwrap();
        fs::write(src.join("nested").join("deep.txt"), "deep").unwrap();

        let dest = tmp.path().join("dest");
        copy_dir_atomic(&src, &dest).unwrap();

        assert_eq!(fs::read_to_string(dest.join("top.txt")).unwrap(), "top");
        assert_eq!(fs::read_to_string(dest.join("nested").join("deep.txt")).unwrap(), "deep");
    }

    #[test]
    fn errors_if_dest_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        assert!(copy_dir_atomic(&src, &dest).is_err());
    }

    #[test]
    fn leaves_no_partial_dest_on_temp_collision_cleanup() {
        // A stale .tmp-dest from a previous crashed run must not block a fresh copy.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(tmp.path().join(".tmp-dest")).unwrap();
        copy_dir_atomic(&src, &dest).unwrap();
        assert!(dest.join("a.txt").exists());
    }

    #[test]
    fn ensure_manifest_generates_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        fs::create_dir_all(&dir).unwrap();
        ensure_manifest(&dir, "my-skill").unwrap();
        let manifest = dir.join(".claude-plugin").join("plugin.json");
        assert!(manifest.is_file());
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(manifest).unwrap()).unwrap();
        assert_eq!(v["name"], serde_json::json!("my-skill"));
    }

    #[test]
    fn ensure_manifest_is_noop_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        let manifest_dir = dir.join(".claude-plugin");
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(manifest_dir.join("plugin.json"), r#"{"name":"already-here"}"#).unwrap();
        ensure_manifest(&dir, "my-skill").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(manifest_dir.join("plugin.json")).unwrap()).unwrap();
        assert_eq!(v["name"], serde_json::json!("already-here")); // untouched
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib vendor_fs::`
Expected: FAIL to compile (module doesn't exist).

- [ ] **Step 3: Write the implementation**

```rust
// src/vendor_fs.rs (top of file, above tests module)
use std::fs;
use std::path::Path;

/// Copy `src` into `dest` recursively. `dest` must not already exist. Copies
/// into a temp sibling directory first and renames into place, so a failed
/// or interrupted copy never leaves a partially-populated `dest`.
pub fn copy_dir_atomic(src: &Path, dest: &Path) -> anyhow::Result<()> {
    if dest.exists() {
        anyhow::bail!("vendor target already exists: {}", dest.display());
    }
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("vendor target has no parent directory: {}", dest.display()))?;
    fs::create_dir_all(parent)?;

    let file_name = dest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("vendor target has no file name: {}", dest.display()))?;
    let tmp = parent.join(format!(".tmp-{}", file_name.to_string_lossy()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    copy_dir_recursive(src, &tmp)?;
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Ensure `dir` is a loadable plugin unit for `claude --plugin-dir`. If it
/// has no `.claude-plugin/plugin.json`, generate a minimal one. Never
/// overwrites an existing manifest.
pub fn ensure_manifest(dir: &Path, skill_name: &str) -> anyhow::Result<()> {
    let manifest_dir = dir.join(".claude-plugin");
    let manifest_path = manifest_dir.join("plugin.json");
    if manifest_path.exists() {
        return Ok(());
    }
    fs::create_dir_all(&manifest_dir)?;
    let manifest = serde_json::json!({ "name": skill_name });
    fs::write(&manifest_path, format!("{}\n", serde_json::to_string_pretty(&manifest)?))?;
    Ok(())
}
```

Add `mod vendor_fs;` to `src/main.rs`, next to `mod vendor;`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib vendor_fs::`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/vendor_fs.rs src/main.rs
git commit -m "feat: add atomic directory copy and skill manifest generation"
```

---

### Task 3: Store/vendor path helpers

**Files:**
- Modify: `src/fs_paths.rs`

**Interfaces:**
- Produces: `Paths::store_dir(&self) -> PathBuf`, `Paths::marketplace_clone_dir(&self, name: &str) -> PathBuf`, `Paths::external_marketplace_dir(&self, owner: &str, repo: &str) -> PathBuf`, `Paths::profile_vendor_dir(&self, profile_key: &str) -> PathBuf`.

- [ ] **Step 1: Write the failing tests**

```rust
// src/fs_paths.rs, inside #[cfg(test)] mod tests
#[test]
fn derives_store_and_vendor_paths() {
    let p = Paths::from_home(PathBuf::from("/h"));
    assert_eq!(p.store_dir(), PathBuf::from("/h/.claude-profiles/store"));
    assert_eq!(
        p.marketplace_clone_dir("superpowers-marketplace"),
        PathBuf::from("/h/.claude-profiles/store/marketplaces/superpowers-marketplace")
    );
    assert_eq!(
        p.external_marketplace_dir("fuzzyalej", "openpowers"),
        PathBuf::from("/h/.claude-profiles/store/marketplaces/_external/fuzzyalej--openpowers")
    );
    assert_eq!(
        p.profile_vendor_dir("rust-developer"),
        PathBuf::from("/h/.claude-profiles/store/rust-developer/vendor")
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib fs_paths::tests::derives_store_and_vendor_paths`
Expected: FAIL to compile (methods don't exist).

- [ ] **Step 3: Write the implementation**

```rust
// src/fs_paths.rs, add to impl Paths (after marketplaces_seed_file)
pub fn store_dir(&self) -> PathBuf {
    self.user_profiles_dir().join("store")
}

pub fn marketplace_clone_dir(&self, name: &str) -> PathBuf {
    self.store_dir().join("marketplaces").join(name)
}

pub fn external_marketplace_dir(&self, owner: &str, repo: &str) -> PathBuf {
    self.store_dir().join("marketplaces").join("_external").join(format!("{owner}--{repo}"))
}

pub fn profile_vendor_dir(&self, profile_key: &str) -> PathBuf {
    self.store_dir().join(profile_key).join("vendor")
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib fs_paths::`
Expected: PASS (all `fs_paths` tests, including the new one).

- [ ] **Step 5: Commit**

```bash
git add src/fs_paths.rs
git commit -m "feat: add store/vendor path helpers to Paths"
```

---

### Task 4: Rewrite provisioning to vendor marketplaces/plugins/skills

**Files:**
- Modify: `src/provision.rs`
- Modify: `src/commands/update.rs`

**Interfaces:**
- Consumes: `vendor::parse_marketplace_json`, `vendor::find_plugin`, `vendor::PluginSource` (Task 1); `vendor_fs::copy_dir_atomic`, `vendor_fs::ensure_manifest` (Task 2); `Paths::marketplace_clone_dir`, `Paths::external_marketplace_dir`, `Paths::profile_vendor_dir`, `Paths::claude_skills_dir` (Task 3); `GitCli` (existing, unchanged).
- Produces: `pub fn ensure_marketplace_clones<G: GitCli>(git: &G, profile: &Profile, paths: &Paths) -> anyhow::Result<()>`, `pub fn pin_marketplaces<G: GitCli>(git: &G, profile: &Profile, mkt_install_dir: &dyn Fn(&str) -> PathBuf, lock: &mut Lockfile, update_floating: bool) -> anyhow::Result<()>` (signature simplified: drops the now-dead `_installed_mkts: &[Marketplace]` parameter, since `claude::Marketplace` is being removed in Task 9), `pub fn vendor_plugins<G: GitCli>(git: &G, profile: &Profile, profile_key: &str, cwd: &Path, paths: &Paths, force: bool) -> anyhow::Result<()>`, `pub fn provision<G: GitCli>(git: &G, profile: &Profile, profile_key: &str, cwd: &Path, paths: &Paths, assume_yes: bool) -> anyhow::Result<()>`.

This is the core rewrite. `pin_marketplaces`'s internal logic (checkout target SHA, record it) is unchanged — only its dead `_installed_mkts` parameter is dropped. Provisioning order matters: clone marketplaces, pin them to a SHA, **then** vendor plugins out of the now-pinned checkout — otherwise a floating marketplace could resolve to whatever HEAD happens to be at copy time instead of the locked SHA.

The design spec calls out "skill-name collision at vendor time" as a hard-error case. Keying each vendored directory by the *full* `plugin_id` (e.g. `my-skill@skills-dir`, not just `my-skill`) makes that collision structurally impossible — two different profile entries can never target the same vendor path unless they're the literal same id, which is the ordinary idempotent-skip case, not a collision. No separate collision check is needed as a result; this is a resolved design decision, not a gap.

- [ ] **Step 1: Write the failing tests**

```rust
// src/provision.rs, replace the existing #[cfg(test)] mod tests with:
#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitCli;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};

    struct MockGit {
        cloned: RefCell<Vec<(String, PathBuf)>>,
        head: String,
    }
    impl GitCli for MockGit {
        fn clone(&self, url: &str, dest: &Path) -> anyhow::Result<()> {
            self.cloned.borrow_mut().push((url.to_string(), dest.to_path_buf()));
            fs::create_dir_all(dest)?;
            Ok(())
        }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok(self.head.clone()) }
        fn checkout(&self, _r: &Path, _gr: &str) -> anyhow::Result<()> { Ok(()) }
        fn is_repo(&self, _r: &Path) -> bool { false } // unpinned: SHA recording skipped, as today
        fn sparse_fetch(&self, _u: &str, _d: &Path, _s: &str) -> anyhow::Result<()> { Ok(()) }
    }

    fn profile(json: &str) -> crate::profile::Profile {
        crate::profile::Profile::from_json_str(json).unwrap()
    }

    #[test]
    fn ensure_marketplace_clones_clones_missing_marketplaces_only() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r"}}"#);
        let git = MockGit { cloned: RefCell::new(vec![]), head: "sha1".into() };

        ensure_marketplace_clones(&git, &p, &paths).unwrap();
        assert_eq!(git.cloned.borrow().len(), 1);
        assert!(paths.marketplace_clone_dir("m").is_dir());

        // Second call: already cloned, no-op.
        ensure_marketplace_clones(&git, &p, &paths).unwrap();
        assert_eq!(git.cloned.borrow().len(), 1);
    }

    #[test]
    fn vendors_relative_path_plugin_from_marketplace_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let mkt_dir = paths.marketplace_clone_dir("m");
        fs::create_dir_all(mkt_dir.join(".claude-plugin")).unwrap();
        fs::write(
            mkt_dir.join(".claude-plugin").join("marketplace.json"),
            r#"{"plugins":[{"name":"foo","source":"./skills/foo"}]}"#,
        ).unwrap();
        fs::create_dir_all(mkt_dir.join("skills").join("foo")).unwrap();
        fs::write(mkt_dir.join("skills").join("foo").join("SKILL.md"), "# foo").unwrap();

        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r"},"plugins":["foo@m"]}"#);
        let git = MockGit { cloned: RefCell::new(vec![]), head: "sha1".into() };
        vendor_plugins(&git, &p, "p", tmp.path(), &paths, false).unwrap();

        let vendored = paths.profile_vendor_dir("p").join("foo@m");
        assert!(vendored.join("SKILL.md").is_file());
    }

    #[test]
    fn vendors_external_repo_plugin_by_cloning_it() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let mkt_dir = paths.marketplace_clone_dir("m");
        fs::create_dir_all(mkt_dir.join(".claude-plugin")).unwrap();
        fs::write(
            mkt_dir.join(".claude-plugin").join("marketplace.json"),
            r#"{"plugins":[{"name":"ext","source":{"source":"github","repo":"owner/ext"}}]}"#,
        ).unwrap();

        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r"},"plugins":["ext@m"]}"#);
        let git = MockGit { cloned: RefCell::new(vec![]), head: "sha1".into() };
        vendor_plugins(&git, &p, "p", tmp.path(), &paths, false).unwrap();

        assert!(git.cloned.borrow().iter().any(|(url, _)| url.contains("owner/ext")));
        assert!(paths.profile_vendor_dir("p").join("ext@m").is_dir());
    }

    #[test]
    fn vendors_loose_skill_and_generates_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let personal_skill = paths.claude_skills_dir().join("my-skill");
        fs::create_dir_all(&personal_skill).unwrap();
        fs::write(personal_skill.join("SKILL.md"), "# my-skill").unwrap();

        let p = profile(r#"{"name":"p","plugins":["my-skill@skills-dir"]}"#);
        let git = MockGit { cloned: RefCell::new(vec![]), head: "sha1".into() };
        vendor_plugins(&git, &p, "p", tmp.path(), &paths, false).unwrap();

        let vendored = paths.profile_vendor_dir("p").join("my-skill@skills-dir");
        assert!(vendored.join("SKILL.md").is_file());
        assert!(vendored.join(".claude-plugin").join("plugin.json").is_file());
        // original skill folder untouched:
        assert!(!personal_skill.join(".claude-plugin").exists());
    }

    #[test]
    fn skips_already_vendored_plugin_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let mkt_dir = paths.marketplace_clone_dir("m");
        fs::create_dir_all(mkt_dir.join(".claude-plugin")).unwrap();
        fs::write(
            mkt_dir.join(".claude-plugin").join("marketplace.json"),
            r#"{"plugins":[{"name":"foo","source":"./foo"}]}"#,
        ).unwrap();
        fs::create_dir_all(mkt_dir.join("foo")).unwrap();
        fs::write(mkt_dir.join("foo").join("a.txt"), "v1").unwrap();

        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r"},"plugins":["foo@m"]}"#);
        let git = MockGit { cloned: RefCell::new(vec![]), head: "sha1".into() };
        vendor_plugins(&git, &p, "p", tmp.path(), &paths, false).unwrap();

        // Marketplace clone changes (simulating a moved pin):
        fs::write(mkt_dir.join("foo").join("a.txt"), "v2").unwrap();

        // Without force: existing vendor copy is left alone (still v1).
        vendor_plugins(&git, &p, "p", tmp.path(), &paths, false).unwrap();
        let vendored = paths.profile_vendor_dir("p").join("foo@m");
        assert_eq!(fs::read_to_string(vendored.join("a.txt")).unwrap(), "v1");

        // With force: re-vendored (now v2).
        vendor_plugins(&git, &p, "p", tmp.path(), &paths, true).unwrap();
        assert_eq!(fs::read_to_string(vendored.join("a.txt")).unwrap(), "v2");
    }

    #[test]
    fn errors_on_unresolvable_loose_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let p = profile(r#"{"name":"p","plugins":["missing@skills-dir"]}"#);
        let git = MockGit { cloned: RefCell::new(vec![]), head: "sha1".into() };
        assert!(vendor_plugins(&git, &p, "p", tmp.path(), &paths, false).is_err());
    }

    #[test]
    fn pins_explicit_ref_and_records_sha() {
        let git = MockGit { cloned: RefCell::new(vec![]), head: "sha_after_v1".into() };
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r#v1"}}"#);
        pin_marketplaces(&git, &p, &dir, &mut lock, false).unwrap();
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "sha_after_v1");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib provision::`
Expected: FAIL to compile (`ensure_marketplace_clones`, `vendor_plugins`, and the simplified `pin_marketplaces` signature don't exist yet; the old `provision()`/`compute_plan`/`confirm` referencing `ClaudeCli` will also fail once `claude.rs` usages are removed below).

- [ ] **Step 3: Write the implementation**

Replace the whole non-test body of `src/provision.rs` with:

```rust
use crate::fs_paths::Paths;
use crate::git::{parse_repo_ref, GitCli};
use crate::lock::{Lockfile, LockedMarketplace};
use crate::profile::Profile;
use crate::vendor::{self, PluginSource};
use crate::vendor_fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Clone any marketplace referenced by `profile` that isn't already cloned into
/// claude-profile's own store. Idempotent: existing clones are left alone (`update`
/// advances them separately via `pin_marketplaces`' checkout).
pub fn ensure_marketplace_clones<G: GitCli>(git: &G, profile: &Profile, paths: &Paths) -> anyhow::Result<()> {
    for (name, source) in &profile.marketplaces {
        let dir = paths.marketplace_clone_dir(name);
        if dir.is_dir() {
            continue;
        }
        let repo_ref = parse_repo_ref(source)?;
        git.clone(&repo_ref.clone_url(), &dir)?;
    }
    Ok(())
}

/// Vendor every plugin/skill `profile.plugins` references into
/// `store/<profile_key>/vendor/<plugin_id>/`. Already-vendored entries are left
/// alone unless `force` is set (used by `update` after a pin moves).
pub fn vendor_plugins<G: GitCli>(
    git: &G,
    profile: &Profile,
    profile_key: &str,
    cwd: &Path,
    paths: &Paths,
    force: bool,
) -> anyhow::Result<()> {
    for plugin_id in &profile.plugins {
        let dest = paths.profile_vendor_dir(profile_key).join(plugin_id);
        if dest.exists() {
            if !force {
                continue;
            }
            std::fs::remove_dir_all(&dest)?;
        }

        let (name, marketplace) = plugin_id
            .rsplit_once('@')
            .ok_or_else(|| anyhow::anyhow!("plugin id '{plugin_id}' is missing '@marketplace'"))?;

        if marketplace == "skills-dir" {
            let src = locate_loose_skill(name, cwd, paths)?;
            vendor_fs::copy_dir_atomic(&src, &dest)?;
            vendor_fs::ensure_manifest(&dest, name)?;
            continue;
        }

        let mkt_dir = paths.marketplace_clone_dir(marketplace);
        let manifest_body = std::fs::read_to_string(mkt_dir.join(".claude-plugin").join("marketplace.json"))
            .map_err(|e| anyhow::anyhow!("reading marketplace.json for '{marketplace}': {e}"))?;
        let plugins = vendor::parse_marketplace_json(&manifest_body)?;
        let entry = vendor::find_plugin(&plugins, name)?;

        match &entry.source {
            PluginSource::RelativePath(rel) => {
                let src = mkt_dir.join(rel.trim_start_matches("./"));
                vendor_fs::copy_dir_atomic(&src, &dest)?;
            }
            PluginSource::ExternalRepo { repo } => {
                let repo_ref = parse_repo_ref(repo)?;
                let ext_dir = paths.external_marketplace_dir(&repo_ref.owner, &repo_ref.repo);
                if !ext_dir.is_dir() {
                    git.clone(&repo_ref.clone_url(), &ext_dir)?;
                }
                vendor_fs::copy_dir_atomic(&ext_dir, &dest)?;
            }
        }
    }
    Ok(())
}

/// Find a personal skill's source folder: project-local `.claude/skills/<name>`
/// first, then the user's personal `~/.claude/skills/<name>`. The result is only
/// ever *read* — vendoring always copies, never modifies the original.
fn locate_loose_skill(name: &str, cwd: &Path, paths: &Paths) -> anyhow::Result<PathBuf> {
    let project = cwd.join(".claude").join("skills").join(name);
    if project.is_dir() {
        return Ok(project);
    }
    let personal = paths.claude_skills_dir().join(name);
    if personal.is_dir() {
        return Ok(personal);
    }
    anyhow::bail!("loose skill '{name}' not found under .claude/skills or ~/.claude/skills")
}

fn confirm(profile: &Profile, missing_marketplaces: &[String], missing_plugins: &[String]) -> bool {
    println!("claude-profile will vendor the following into ~/.claude-profiles/store/{}/:", profile.name);
    for name in missing_marketplaces {
        println!("  marketplace clone: {name}");
    }
    for id in missing_plugins {
        println!("  plugin/skill:      {id}");
    }
    print!("Proceed? [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes")
}

/// Top-level provisioning entry point: clone missing marketplaces, pin them to
/// their target SHA, then vendor every plugin/skill the profile references.
/// Prompts for confirmation before doing anything new (`assume_yes` skips it).
pub fn provision<G: GitCli>(
    git: &G,
    profile: &Profile,
    profile_key: &str,
    cwd: &Path,
    paths: &Paths,
    assume_yes: bool,
) -> anyhow::Result<()> {
    let missing_marketplaces: Vec<String> = profile
        .marketplaces
        .keys()
        .filter(|name| !paths.marketplace_clone_dir(name).is_dir())
        .cloned()
        .collect();
    let missing_plugins: Vec<String> = profile
        .plugins
        .iter()
        .filter(|id| !paths.profile_vendor_dir(profile_key).join(id).exists())
        .cloned()
        .collect();

    if !missing_marketplaces.is_empty() || !missing_plugins.is_empty() {
        if !assume_yes && !confirm(profile, &missing_marketplaces, &missing_plugins) {
            anyhow::bail!("provisioning declined by user");
        }
    }

    ensure_marketplace_clones(git, profile, paths)
}

/// Which commit-ish to check out before recording a marketplace's SHA (None = leave at HEAD).
/// On update, floating marketplaces move to HEAD while explicit `#ref`s stay pinned; otherwise
/// a previously-locked SHA is reproduced, falling back to the profile's explicit `#ref`.
fn checkout_target(
    update_floating: bool,
    locked: Option<&LockedMarketplace>,
    explicit_ref: &Option<String>,
) -> Option<String> {
    if update_floating {
        return explicit_ref.clone();
    }
    match locked {
        Some(l) => Some(l.sha.clone()),
        None => explicit_ref.clone(),
    }
}

pub fn pin_marketplaces<G: GitCli>(
    git: &G,
    profile: &Profile,
    mkt_install_dir: &dyn Fn(&str) -> PathBuf,
    lock: &mut Lockfile,
    update_floating: bool,
) -> anyhow::Result<()> {
    for (name, source) in &profile.marketplaces {
        let repo_ref = parse_repo_ref(source)?;
        let dir = mkt_install_dir(name);

        if !git.is_repo(&dir) {
            eprintln!(
                "WARNING: marketplace '{name}' at {} is not a git checkout; recording unpinned (not reproducible)",
                dir.display()
            );
            lock.marketplaces
                .insert(name.clone(), LockedMarketplace { source: source.clone(), sha: String::new() });
            continue;
        }

        let target = checkout_target(update_floating, lock.marketplaces.get(name), &repo_ref.git_ref);
        if let Some(ref t) = target {
            git.checkout(&dir, t)?;
        }
        let sha = git.head_sha(&dir)?;
        lock.marketplaces.insert(name.clone(), LockedMarketplace { source: source.clone(), sha });
    }
    Ok(())
}
```

Note `provision()` above intentionally stops at `ensure_marketplace_clones` — plugin vendoring must happen *after* `pin_marketplaces` checks out the target SHA (see Task 10 for the exact call order in `main.rs`), so `vendor_plugins` is invoked separately from `main.rs`, not from inside `provision()`.

Now update `src/commands/update.rs`: drop the dead `Marketplace` plumbing and re-vendor plugins after re-pinning.

```rust
// src/commands/update.rs — replace the whole file
use crate::fs_paths::Paths;
use crate::git::GitCli;
use crate::lock::Lockfile;
use crate::profile::Profile;
use crate::provision::{pin_marketplaces, vendor_plugins};
use std::path::{Path, PathBuf};

pub fn frozen_check(profiles: &[(String, Profile, Lockfile)]) -> anyhow::Result<()> {
    let stale: Vec<&str> = profiles.iter()
        .filter(|(_, p, lf)| lf.is_stale_against(p))
        .map(|(n, _, _)| n.as_str())
        .collect();
    if !stale.is_empty() {
        anyhow::bail!("--frozen: lock out of date for profile(s): {}", stale.join(", "));
    }
    Ok(())
}

/// Re-resolve a profile's floating marketplaces to HEAD, record the new SHA,
/// then re-vendor its plugins so vendored code matches the newly pinned SHA.
pub fn reresolve_profile<G: GitCli>(
    git: &G,
    profile: &Profile,
    profile_key: &str,
    cwd: &Path,
    paths: &Paths,
    mkt_dirs: &dyn Fn(&str) -> PathBuf,
    lock: &mut Lockfile,
) -> anyhow::Result<()> {
    pin_marketplaces(git, profile, mkt_dirs, lock, true)?;
    vendor_plugins(git, profile, profile_key, cwd, paths, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitCli;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path as StdPath, PathBuf};

    struct MockGit { head: String, checkouts: RefCell<Vec<String>> }
    impl GitCli for MockGit {
        fn clone(&self, _u: &str, d: &StdPath) -> anyhow::Result<()> { fs::create_dir_all(d)?; Ok(()) }
        fn pull(&self, _r: &StdPath) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &StdPath) -> anyhow::Result<String> { Ok(self.head.clone()) }
        fn is_repo(&self, _r: &StdPath) -> bool { true }
        fn checkout(&self, _r: &StdPath, gr: &str) -> anyhow::Result<()> { self.checkouts.borrow_mut().push(gr.into()); Ok(()) }
        fn sparse_fetch(&self, _u: &str, _d: &StdPath, _s: &str) -> anyhow::Result<()> { Ok(()) }
    }
    fn prof(json: &str) -> crate::profile::Profile { crate::profile::Profile::from_json_str(json).unwrap() }

    #[test]
    fn reresolve_moves_floating_to_head_and_revendors() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let mkt_dir = tmp.path().join("mkt");
        fs::create_dir_all(mkt_dir.join(".claude-plugin")).unwrap();
        fs::write(mkt_dir.join(".claude-plugin").join("marketplace.json"),
            r#"{"plugins":[{"name":"foo","source":"./foo"}]}"#).unwrap();
        fs::create_dir_all(mkt_dir.join("foo")).unwrap();
        fs::write(mkt_dir.join("foo").join("a.txt"), "v2").unwrap();

        let p = prof(r#"{"name":"p","marketplaces":{"m":"o/r"},"plugins":["foo@m"]}"#);
        let git = MockGit { head: "newhead".into(), checkouts: RefCell::new(vec![]) };
        let mut lock = crate::lock::Lockfile::new("p");
        lock.marketplaces.insert("m".into(), crate::lock::LockedMarketplace { source: "o/r".into(), sha: "old".into() });

        reresolve_profile(&git, &p, "p", tmp.path(), &paths, &|_| mkt_dir.clone(), &mut lock).unwrap();

        assert!(git.checkouts.borrow().is_empty()); // floating: no checkout to a pin
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "newhead");
        let vendored = paths.profile_vendor_dir("p").join("foo@m");
        assert_eq!(fs::read_to_string(vendored.join("a.txt")).unwrap(), "v2");
    }

    #[test]
    fn frozen_check_errors_on_stale_lock() {
        let p = prof(r#"{"name":"p","marketplaces":{"m1":"o/r","m2":"o/s"}}"#);
        let mut lf = crate::lock::Lockfile::new("p");
        lf.marketplaces.insert("m1".into(), crate::lock::LockedMarketplace { source: "o/r".into(), sha: "a".into() });
        let err = frozen_check(&[("p".into(), p, lf)]);
        assert!(err.is_err());
    }

    #[test]
    fn frozen_check_passes_when_all_locked() {
        let p = prof(r#"{"name":"p","marketplaces":{"m":"o/r"}}"#);
        let mut lf = crate::lock::Lockfile::new("p");
        lf.marketplaces.insert("m".into(), crate::lock::LockedMarketplace { source: "o/r".into(), sha: "a".into() });
        assert!(frozen_check(&[("p".into(), p, lf)]).is_ok());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib provision:: commands::update::`
Expected: PASS (all tests in both modules).

- [ ] **Step 5: Commit**

```bash
git add src/provision.rs src/commands/update.rs
git commit -m "feat: vendor plugins/skills into per-profile store instead of installing globally"
```

---

### Task 5: Launch via `--plugin-dir` instead of `--settings enabledPlugins`

**Files:**
- Modify: `src/launch.rs`

**Interfaces:**
- Consumes: `Paths::profile_vendor_dir` (Task 3).
- Produces: `pub fn build_args(profile: &Profile, profile_key: &str, paths: &Paths, extra: &[String]) -> anyhow::Result<Vec<String>>` (replaces the old `build_args(profile, enablement, extra)` — the `Enablement` parameter is gone entirely).

- [ ] **Step 1: Write the failing tests**

```rust
// src/launch.rs — replace the existing #[cfg(test)] mod tests with:
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_paths::Paths;
    use std::fs;

    #[test]
    fn assembles_plugin_dir_flags_for_vendored_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let vendor = paths.profile_vendor_dir("p");
        fs::create_dir_all(vendor.join("foo@m")).unwrap();
        fs::create_dir_all(vendor.join("bar@m")).unwrap();

        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","plugins":["foo@m","bar@m"],"mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();

        let flags: Vec<&String> = args.iter().collect();
        assert_eq!(flags.iter().filter(|a| a.as_str() == "--plugin-dir").count(), 2);
        assert!(args.contains(&vendor.join("foo@m").to_string_lossy().to_string()));
        assert!(args.contains(&vendor.join("bar@m").to_string_lossy().to_string()));
    }

    #[test]
    fn includes_profile_plugin_dirs_alongside_vendored_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        fs::create_dir_all(paths.profile_vendor_dir("p")).unwrap();

        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","pluginDirs":["vendor/x"],"mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();
        let d = args.iter().position(|a| a == "vendor/x").unwrap();
        assert_eq!(args[d - 1], "--plugin-dir");
    }

    #[test]
    fn no_settings_flag_is_emitted() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let p = crate::profile::Profile::from_json_str(r#"{"name":"p","mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();
        assert!(!args.contains(&"--settings".to_string()));
    }

    #[test]
    fn mcp_config_strict_and_bare_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","mcpServers":{"srv":{"command":"echo"}},"bare":true}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        let i = args.iter().position(|a| a == "--mcp-config").unwrap();
        let mcp_config: serde_json::Value = serde_json::from_str(&args[i + 1]).unwrap();
        assert_eq!(mcp_config, serde_json::json!({"mcpServers": {"srv": {"command": "echo"}}}));
        assert!(args.contains(&"--bare".to_string()));
    }

    #[test]
    fn forwards_extra_args() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let p = crate::profile::Profile::from_json_str(r#"{"name":"p","mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &["--model".into(), "opus".into()]).unwrap();
        assert_eq!(&args[args.len() - 2..], &["--model".to_string(), "opus".to_string()]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib launch::`
Expected: FAIL to compile (old `build_args` signature takes `Enablement`, not `profile_key`/`paths`).

- [ ] **Step 3: Write the implementation**

```rust
// src/launch.rs — replace build_args, keep spawn() unchanged
use crate::fs_paths::Paths;
use crate::profile::Profile;
use std::process::Command;

pub fn build_args(
    profile: &Profile,
    profile_key: &str,
    paths: &Paths,
    extra: &[String],
) -> anyhow::Result<Vec<String>> {
    let mut args = vec![
        "--strict-mcp-config".to_string(),
        "--mcp-config".to_string(),
        serde_json::to_string(&serde_json::json!({ "mcpServers": profile.mcp_servers }))?,
    ];

    let vendor_dir = paths.profile_vendor_dir(profile_key);
    if vendor_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&vendor_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for dir in entries {
            args.push("--plugin-dir".to_string());
            args.push(dir.to_string_lossy().to_string());
        }
    }

    for dir in &profile.plugin_dirs {
        args.push("--plugin-dir".to_string());
        args.push(dir.clone());
    }

    if profile.bare {
        args.push("--bare".to_string());
    }
    // Append forwarded args directly (no `--`): claude treats everything after a
    // `--` as the positional prompt, so a separator would turn flags like
    // `--model opus` into prompt text. As options they parse correctly, and a
    // trailing prompt still lands as the positional arg.
    args.extend(extra.iter().cloned());
    Ok(args)
}

pub fn spawn(profile_name: &str, args: &[String]) -> anyhow::Result<i32> {
    let status = Command::new("claude")
        .args(args)
        .env("CLAUDE_PROFILE", profile_name)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to spawn claude: {e}"))?;
    Ok(status.code().unwrap_or(1))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib launch::`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/launch.rs
git commit -m "feat: launch via --plugin-dir over vendored entries instead of --settings"
```

---

### Task 6: Rewrite `status` to report the local vendor store

**Files:**
- Modify: `src/commands/status.rs`

**Interfaces:**
- Consumes: `Paths::store_dir` (Task 3).
- Produces: `pub fn run(paths: &Paths, profiles: &[(String, Profile)]) -> anyhow::Result<()>` (replaces the old `ClaudeCli`-based `run`); `pub fn format_status(paths: &Paths, profiles: &[(String, Profile)]) -> anyhow::Result<String>`.

- [ ] **Step 1: Write the failing tests**

```rust
// src/commands/status.rs — replace the whole file
use crate::fs_paths::Paths;
use crate::profile::Profile;

pub fn format_status(paths: &Paths, profiles: &[(String, Profile)]) -> anyhow::Result<String> {
    let mut out = String::from("Vendored profiles:\n");
    for (name, _profile) in profiles {
        let vendor_dir = paths.profile_vendor_dir(name);
        if !vendor_dir.is_dir() {
            out.push_str(&format!("  {name}  (not yet provisioned)\n"));
            continue;
        }
        let mut entries: Vec<String> = std::fs::read_dir(&vendor_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        out.push_str(&format!("  {name}  ({} vendored)\n", entries.len()));
        for id in entries {
            out.push_str(&format!("    - {id}\n"));
        }
    }
    Ok(out)
}

pub fn run(paths: &Paths, profiles: &[(String, Profile)]) -> anyhow::Result<()> {
    print!("{}", format_status(paths, profiles)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_vendored_entries_per_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        fs::create_dir_all(paths.profile_vendor_dir("p").join("foo@m")).unwrap();
        fs::create_dir_all(paths.profile_vendor_dir("p").join("bar@m")).unwrap();

        let profiles = vec![("p".to_string(), Profile::from_json_str(r#"{"name":"p"}"#).unwrap())];
        let s = format_status(&paths, &profiles).unwrap();
        assert!(s.contains("p  (2 vendored)"));
        assert!(s.contains("foo@m"));
        assert!(s.contains("bar@m"));
    }

    #[test]
    fn reports_not_yet_provisioned_when_no_vendor_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let profiles = vec![("p".to_string(), Profile::from_json_str(r#"{"name":"p"}"#).unwrap())];
        let s = format_status(&paths, &profiles).unwrap();
        assert!(s.contains("p  (not yet provisioned)"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib commands::status::`
Expected: FAIL to compile (old file still references `claude::ClaudeCli`/`refmap`, and the tests above reference the new signature).

- [ ] **Step 3: Implementation already written above (this task replaces the whole file).**

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib commands::status::`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/commands/status.rs
git commit -m "feat: report vendored store contents instead of claude plugin list"
```

---

### Task 7: `remove` also deletes the profile's vendor tree

**Files:**
- Modify: `src/commands/remove.rs`

**Interfaces:**
- Consumes: `Paths::store_dir` (Task 3).
- Produces: `RemovePlan::Profile` gains a `vendor: PathBuf` field; `apply` deletes it if present.

- [ ] **Step 1: Write the failing tests**

```rust
// src/commands/remove.rs — add to #[cfg(test)] mod tests
#[test]
fn plan_profile_removal_includes_vendor_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let updir = home.join(".claude-profiles");
    fs::create_dir_all(&updir).unwrap();
    fs::write(updir.join("foo.json"), r#"{"name":"foo"}"#).unwrap();
    let paths = crate::fs_paths::Paths::from_home(home);
    let plan = remove_target("foo", &paths, tmp.path(), None, &tmp.path().join("bundled")).unwrap();
    match plan {
        RemovePlan::Profile { vendor, .. } => {
            assert!(vendor.ends_with(".claude-profiles/store/foo/vendor"));
        }
        _ => panic!("expected Profile plan"),
    }
}

#[test]
fn apply_deletes_vendor_dir_if_present() {
    let tmp = tempfile::tempdir().unwrap();
    let json = tmp.path().join("foo.json");
    let lock = tmp.path().join("foo.lock");
    let vendor = tmp.path().join("store").join("foo").join("vendor");
    fs::write(&json, "{}").unwrap();
    fs::create_dir_all(vendor.join("some-plugin@m")).unwrap();
    apply(&RemovePlan::Profile { json: json.clone(), lock: lock.clone(), vendor: vendor.clone() }).unwrap();
    assert!(!json.exists());
    assert!(!vendor.exists());
}

#[test]
fn apply_tolerates_missing_vendor_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let json = tmp.path().join("foo.json");
    fs::write(&json, "{}").unwrap();
    let plan = RemovePlan::Profile {
        json: json.clone(), lock: tmp.path().join("foo.lock"), vendor: tmp.path().join("nope"),
    };
    apply(&plan).unwrap(); // must not error just because vendor was never provisioned
    assert!(!json.exists());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib commands::remove::`
Expected: FAIL to compile (`RemovePlan::Profile` has no `vendor` field yet).

- [ ] **Step 3: Write the implementation**

```rust
// src/commands/remove.rs — modify RemovePlan and its construction/apply
pub enum RemovePlan {
    Profile { json: PathBuf, lock: PathBuf, vendor: PathBuf },
    Pack { dir: PathBuf },
}

pub fn remove_target(
    name_or_repo: &str,
    paths: &Paths,
    cwd: &Path,
    env_dir: Option<&Path>,
    bundled_dir: &Path,
) -> anyhow::Result<RemovePlan> {
    if name_or_repo.contains('/') {
        let repo_ref = crate::git::parse_repo_ref(name_or_repo)?;
        let dir = paths.user_profiles_dir().join("packs").join(repo_ref.pack_dir_name());
        if !dir.is_dir() {
            anyhow::bail!("pack '{name_or_repo}' is not installed");
        }
        return Ok(RemovePlan::Pack { dir });
    }
    let resolved = resolve(name_or_repo, paths, cwd, env_dir, bundled_dir)?;
    if matches!(resolved.source, ProfileSource::BundledDir) {
        anyhow::bail!("cannot remove engine bundled profile '{name_or_repo}'");
    }
    if matches!(resolved.source, ProfileSource::Pack(_)) {
        anyhow::bail!(
            "'{name_or_repo}' belongs to an installed pack; remove the whole pack with `claude-profile remove <owner/repo>` instead"
        );
    }
    let lock = lock_path(name_or_repo, &resolved.path, &resolved.source, paths);
    let vendor = paths.profile_vendor_dir(name_or_repo);
    Ok(RemovePlan::Profile { json: resolved.path, lock, vendor })
}

pub fn apply(plan: &RemovePlan) -> anyhow::Result<()> {
    match plan {
        RemovePlan::Profile { json, lock, vendor } => {
            std::fs::remove_file(json)?;
            if lock.exists() {
                std::fs::remove_file(lock)?;
            }
            if vendor.exists() {
                std::fs::remove_dir_all(vendor)?;
            }
        }
        RemovePlan::Pack { dir } => {
            std::fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}
```

Update the two existing tests that construct `RemovePlan::Profile { json, lock }` (`plans_profile_removal_with_lock`'s match arm and `apply_deletes_profile_and_lock`) to also destructure/pass `vendor`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib commands::remove::`
Expected: PASS (all tests, including the 3 new ones).

- [ ] **Step 5: Commit**

```bash
git add src/commands/remove.rs
git commit -m "feat: remove a profile's vendored plugin tree along with its profile file"
```

---

### Task 8: Simplify `self-uninstall` (no more "provisioned into ~/.claude" caveat)

**Files:**
- Modify: `src/commands/self_uninstall.rs`

**Interfaces:**
- Produces: `pub fn run(paths: &Paths, purge: bool) -> anyhow::Result<()>` (drops the `referenced_only_by_profiles: &[String]` parameter — nothing is left in `~/.claude` to warn about once vendoring lands, since `--purge` now removes every profile's vendor tree along with everything else under `~/.claude-profiles`).

`run()` shells out via `std::env::current_exe()` and prints to stdout, so it isn't
meaningfully unit-testable beyond what `plan`/`apply` (already covered by 5 existing
tests) exercise. This task has no new test to write — it only removes dead
parameters/output from `run`, and the existing `plan`/`apply` tests are the
right-sized coverage for that. Compilation itself is the check: `main.rs` (Task 10)
won't build against a `run(paths, purge)` call unless this signature is correct.

- [ ] **Step 1: Write the implementation**

```rust
// src/commands/self_uninstall.rs — replace `run`, drop the `referenced_only_by_profiles` parameter
pub fn run(paths: &Paths, purge: bool) -> anyhow::Result<()> {
    let current = std::env::current_exe()?;
    let pl = plan(current, paths, purge);
    println!("removing binary: {}", pl.binary.display());
    if let Some(dir) = &pl.purge_dir {
        println!("purging profile data (including all vendored plugins/skills): {}", dir.display());
    }
    apply(&pl)
}
```

- [ ] **Step 2: Run existing tests to verify nothing broke**

Run: `cargo test --lib commands::self_uninstall::`
Expected: PASS (the existing `plan`/`apply` tests — 5 total — are untouched by this change).

- [ ] **Step 3: Commit**

```bash
git add src/commands/self_uninstall.rs
git commit -m "feat: simplify self-uninstall now that nothing is left in ~/.claude"
```

---

### Task 9: Delete obsolete modules (`claude.rs`, `enablement.rs`, `refmap.rs`, `disable.rs`, `gc.rs`)

**Files:**
- Delete: `src/claude.rs`, `src/enablement.rs`, `src/refmap.rs`, `src/commands/disable.rs`, `src/commands/gc.rs`
- Modify: `src/commands/mod.rs`

**Interfaces:**
- Consumes: nothing (this task only removes now-dead code; Tasks 4-8 already removed every real usage of these modules' types).
- Produces: nothing new.

- [ ] **Step 1: Confirm nothing still references the modules being deleted**

Run: `grep -rn "claude::\|enablement::\|refmap::\|commands::disable\|commands::gc" src/ --include=*.rs`
Expected: only matches inside the files being deleted in this task, plus `src/main.rs` (handled in Task 10 — leave `mod claude;` etc. in `main.rs` for now; Task 10 removes those lines together with the `Command::Disable`/`Command::Gc` variants, since both changes touch the same match arms).

- [ ] **Step 2: Delete the files**

```bash
git rm src/claude.rs src/enablement.rs src/refmap.rs src/commands/disable.rs src/commands/gc.rs
```

- [ ] **Step 3: Remove their entries from `src/commands/mod.rs`**

Read the current file first; remove the `pub mod disable;` and `pub mod gc;` lines, leaving the rest (`list`, `show`, `new`, `test`, `find`, `self_uninstall`, `status`, `remove`, `update`) untouched.

- [ ] **Step 4: Verify the crate does not yet compile (expected — `main.rs` still references deleted items)**

Run: `cargo build 2>&1 | grep "error\[" | head -20`
Expected: errors in `src/main.rs` only (unresolved `mod claude`, `mod enablement`, `mod refmap`, `Command::Disable`, `Command::Gc`, etc.) — confirms Task 9 didn't miss a reference anywhere else. Task 10 fixes `main.rs`.

- [ ] **Step 5: Commit**

```bash
git commit -m "chore: delete modules made obsolete by vendoring (claude.rs, enablement, refmap, disable, gc)"
```

---

### Task 10: Rewire `main.rs` end to end

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: everything produced by Tasks 1-9.
- Produces: a fully compiling, fully passing `claude-profile` binary with `disable`/`gc` removed from the CLI surface.

- [ ] **Step 1: Read the current file fresh (already read earlier in this session) and apply the following changes**

1. **Module list**: remove `mod claude;`, `mod enablement;`, `mod refmap;`; add `mod vendor;`, `mod vendor_fs;` (if not already present from Tasks 1-2). Remove the now-unused `use claude::ClaudeCli;` import line.

2. **`Command` enum**: delete the `Disable { .. }` and `Gc { .. }` variants entirely.

3. **`profiles_for_refmap`**: rename to `load_all_profiles` (it no longer feeds a "refmap" — the name was already generic, just misleading now):
   ```rust
   type LoadedProfiles = (Vec<(String, profile::Profile)>, Vec<(PathBuf, String)>);

   fn load_all_profiles(
       paths: &fs_paths::Paths, cwd: &std::path::Path,
       env: Option<&std::path::Path>, bundled: &std::path::Path,
   ) -> LoadedProfiles {
       let mut out = Vec::new();
       let mut failed = Vec::new();
       for (name, path, _src) in resolve::list_available(paths, cwd, env, bundled) {
           match std::fs::read_to_string(&path) {
               Ok(body) => match profile::Profile::from_json_str(&body) {
                   Ok(p) => out.push((name, p)),
                   Err(e) => failed.push((path, e.to_string())),
               },
               Err(e) => failed.push((path, e.to_string())),
           }
       }
       (out, failed)
   }
   ```
   Update every call site (`Status`, `Gc`'s removal means no call there anymore, `Remove`'s `--prune` block is deleted per point 6 below, `SelfUninstall`) from `profiles_for_refmap` to `load_all_profiles`, and from `ProfilesForRefmap` to `LoadedProfiles`.

4. **`Command::Status` handler**:
   ```rust
   Some(Command::Status) => {
       let (profiles, failed) = load_all_profiles(&paths, &cwd, env.as_deref(), &bundled);
       for (path, err) in &failed {
           eprintln!("warning: could not parse profile {}: {err}", path.display());
       }
       commands::status::run(&paths, &profiles)?;
       Ok(0)
   }
   ```

5. **`Command::Disable` / `Command::Gc` handlers**: delete both match arms, and delete `handle_disable` in its entirety.

6. **`Command::Remove` handler**: delete the `--prune` block (it existed only to call `gc`, which no longer exists), and delete the `prune` field from the `Remove` variant and its doc comment:
   ```rust
   /// Delete a personal profile or cloned pack.
   Remove { target: String },
   ```
   ```rust
   Some(Command::Remove { target }) => {
       let plan = commands::remove::remove_target(&target, &paths, &cwd, env.as_deref(), &bundled)?;
       commands::remove::apply(&plan)?;
       println!("removed {target}");
       Ok(0)
   }
   ```

7. **`Command::SelfUninstall` handler**: drop the refmap-derived `referenced` argument:
   ```rust
   Some(Command::SelfUninstall { purge }) => {
       commands::self_uninstall::run(&paths, purge)?;
       Ok(0)
   }
   ```
   Delete the now-unused `refmap::build_refmap` call and `referenced` variable in this arm.

8. **`handle_update`**: remove the `claude::RealClaude`/`installed_mkts_install_dirs` dependency; marketplace directories now come straight from `paths.marketplace_clone_dir`:
   ```rust
   fn handle_update(
       frozen: bool,
       paths: &fs_paths::Paths,
       cwd: &std::path::Path,
       env: Option<&std::path::Path>,
       bundled: &std::path::Path,
   ) -> anyhow::Result<()> {
       let updated = pack::update_all_packs(&git::RealGit, paths)?;
       for name in &updated { println!("updated pack {name}"); }
       let (profiles, failed) = load_all_profiles(paths, cwd, env, bundled);
       for (path, err) in &failed {
           eprintln!("warning: skipping unparseable profile {}: {err}", path.display());
       }
       let dir_lookup = |n: &str| paths.marketplace_clone_dir(n);
       if frozen {
           let mut triples = Vec::new();
           for (name, profile) in &profiles {
               let Some(profile) = resolve_extends_or_warn(name, profile, paths, cwd, env, bundled) else { continue };
               let resolved = resolve::resolve(name, paths, cwd, env, bundled)?;
               let lp = lock::lock_path(name, &resolved.path, &resolved.source, paths);
               let lf = lock::Lockfile::load(&lp)?.unwrap_or_else(|| lock::Lockfile::new(name));
               triples.push((name.clone(), profile, lf));
           }
           commands::update::frozen_check(&triples)?;
           println!("--frozen: all locks up to date");
       } else {
           for (name, profile) in &profiles {
               let Some(profile) = resolve_extends_or_warn(name, profile, paths, cwd, env, bundled) else { continue };
               provision::ensure_marketplace_clones(&git::RealGit, &profile, paths)?;
               let resolved = resolve::resolve(name, paths, cwd, env, bundled)?;
               let lp = lock::lock_path(name, &resolved.path, &resolved.source, paths);
               let mut lf = lock::Lockfile::load(&lp)?.unwrap_or_else(|| lock::Lockfile::new(name));
               commands::update::reresolve_profile(&git::RealGit, &profile, name, cwd, paths, &dir_lookup, &mut lf)?;
               lf.save(&lp)?;
           }
       }
       Ok(())
   }
   ```
   (Note: unlike the old code, a missing marketplace is no longer a "skip with a warning" case — `ensure_marketplace_clones` clones it on the spot, since claude-profile owns the clone itself now.)

9. **`provision_pin_launch`**: rewrite to the new provisioning/launch sequence — clone marketplaces, pin them, vendor plugins against the pinned checkout, then build launch args from the vendor dir:
   ```rust
   fn provision_pin_launch(
       profile: &profile::Profile,
       key: &str,
       lock_file: &std::path::Path,
       assume_yes: bool,
       extra: &[String],
       cwd: &std::path::Path,
       paths: &fs_paths::Paths,
   ) -> anyhow::Result<i32> {
       provision::provision(&git::RealGit, profile, key, cwd, paths, assume_yes)?;

       let mut lock = lock::Lockfile::load(lock_file)?.unwrap_or_else(|| lock::Lockfile::new(key));
       let dir_lookup = |n: &str| paths.marketplace_clone_dir(n);
       provision::pin_marketplaces(&git::RealGit, profile, &dir_lookup, &mut lock, false)?;
       if let Some(parent) = lock_file.parent() {
           std::fs::create_dir_all(parent)?;
       }
       lock.save(lock_file)?;

       provision::vendor_plugins(&git::RealGit, profile, key, cwd, paths, false)?;

       let args = launch::build_args(profile, key, paths, extra)?;
       launch::spawn(key, &args)
   }
   ```
   Update its two call sites (`handle_launch`'s `[name]` arm and `launch_combined`) to pass `cwd` and drop the no-longer-used `paths.claude_skills_dir()` plumbing.

10. **Delete now-unused functions**: `print_enablement_warnings`, `ensure_marketplaces_installed`, `installed_mkts_install_dirs` — nothing calls any of them anymore.

11. **`profiles_for_refmap` call in `handle_disable`**: the whole function is deleted per point 5.

- [ ] **Step 2: Full workspace build**

Run: `cargo build 2>&1 | tail -40`
Expected: builds cleanly with zero errors and zero warnings about unused imports/dead code (fix any stragglers surfaced here — e.g. an accidental leftover `use claude::...`).

- [ ] **Step 3: Full test suite**

Run: `cargo test 2>&1 | tail -60`
Expected: all tests across every module pass (this exercises Tasks 1-9's tests together for the first time as one binary).

- [ ] **Step 4: Manual smoke test against a throwaway profile**

```bash
mkdir -p /tmp/cp-smoke/.claude-profiles
cat > /tmp/cp-smoke/.claude-profiles/smoke.json <<'EOF'
{"name":"smoke","marketplaces":{},"plugins":[],"mcpServers":{}}
EOF
HOME=/tmp/cp-smoke cargo run --quiet -- status
```
Expected: prints `Vendored profiles:` with `smoke  (not yet provisioned)` and no error — proves the full `Status` path compiles and runs against a real (empty) profile without touching the real `~/.claude`.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs src/commands/mod.rs
git commit -m "feat: wire main.rs to vendored provisioning/launch, drop disable/gc commands"
```

---

### Task 11: Update documentation

**Files:**
- Modify: `docs/how-it-works.md`
- Modify: `docs/commands.md`
- Modify: `docs/profiles.md`
- Modify: `README.md`

No tests apply to prose; each step is read-current-content, rewrite the affected section, commit.

- [ ] **Step 1: `docs/how-it-works.md`**

Replace the "The launch flow" list's step 2-3 and the whole "Isolation is runtime gating, not install isolation" section:

```markdown
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
```

Replace the "What is NOT gated" section's second bullet (manifest-less skills) — it's no longer an unfixable limitation:

```markdown
- **Manifest-less loose skills.** A bare `SKILL.md` with no
  `.claude-plugin/plugin.json` manifest under `~/.claude/skills` or
  `.claude/skills` still can't be *referenced by Claude Code's own*
  enablement system — but claude-profile no longer needs that system.
  Reference it in a profile's `plugins` list as `<name>@skills-dir` and
  claude-profile vendors a **copy** of it, generating a minimal manifest on
  that copy if one is missing. The original skill folder is never modified.
```

- [ ] **Step 2: `docs/commands.md`**

Remove the `disable`/`gc` lines from the top-level command table and their full sections; update `status`'s one-line summary and full section to describe the vendor-store report; update `remove`'s section to note it deletes the profile's vendor tree and drop the `--prune` flag documentation.

- [ ] **Step 3: `docs/profiles.md`**

Update the `plugins` field description:

```markdown
| `plugins` | array of strings, optional | Plugins/skills to vendor, as `plugin@marketplace` ids (or `<name>@skills-dir` for a personal loose skill). Each is copied into this profile's own `~/.claude-profiles/store/<profile>/vendor/`; nothing is installed globally. |
```

Replace the "Making a personal loose skill gateable" section:

```markdown
## Referencing a personal loose skill

A **loose skill** living directly under `~/.claude/skills/<name>` or
`.claude/skills/<name>` (project-local takes precedence) can be referenced
in a profile's `plugins` list as `<name>@skills-dir`. claude-profile vendors
a *copy* of that folder into the profile's own vendor directory, generating
a minimal `.claude-plugin/plugin.json` manifest on the copy if the original
doesn't have one. The original skill folder is never modified — only the
vendored copy gains a manifest.
```

- [ ] **Step 4: `README.md`**

Replace the "What a profile controls" list:

```markdown
- **Plugins and skills** — every plugin/skill a profile references is vendored (cloned/copied)
  into that profile's own directory under `~/.claude-profiles/store/`, and loaded for the
  session via `--plugin-dir`. Nothing is installed or enabled globally; a plain `claude` session
  never sees any of it.
- **Loose skills** (`~/.claude/skills`, `.claude/skills`) — reference one as `<name>@skills-dir`
  in a profile's `plugins` list and claude-profile vendors a copy, generating a manifest on that
  copy if the original lacks one. The original folder is never touched.
- **MCP servers** — launched with `--strict-mcp-config`, so only the profile's servers load;
  your user/project MCP servers never appear. Empty means none.
```

Replace the paragraph beginning "Provisioning installs plugins into the shared user scope..." (and the `disable`/`gc` mini-workflow beneath it) with:

```markdown
Each profile is a fully isolated package: `install` = vendor into
`~/.claude-profiles/store/<profile>/`, `launch` = point `claude` at that
directory via `--plugin-dir`, `remove` = delete it. There's no shared global
install to reconcile, disable, or garbage-collect.
```

Update the "Important limitation" section referenced from `how-it-works.md` to match the shrunk limitation described there (manifest-less skills are now vendorable, not an unfixable leak).

- [ ] **Step 5: Commit**

```bash
git add docs/how-it-works.md docs/commands.md docs/profiles.md README.md
git commit -m "docs: describe vendored plugin/skill isolation, drop disable/gc references"
```
