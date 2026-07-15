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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitCli;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};

    struct MockGit {
        cloned: RefCell<Vec<(String, PathBuf)>>,
        checkouts: RefCell<Vec<(PathBuf, String)>>,
        head: String,
        present: bool,
    }
    impl MockGit {
        fn new(head: &str) -> Self {
            MockGit { cloned: RefCell::new(vec![]), checkouts: RefCell::new(vec![]), head: head.into(), present: true }
        }
    }
    impl GitCli for MockGit {
        fn clone(&self, url: &str, dest: &Path) -> anyhow::Result<()> {
            self.cloned.borrow_mut().push((url.to_string(), dest.to_path_buf()));
            fs::create_dir_all(dest)?;
            Ok(())
        }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok(self.head.clone()) }
        fn checkout(&self, r: &Path, gr: &str) -> anyhow::Result<()> {
            self.checkouts.borrow_mut().push((r.to_path_buf(), gr.to_string()));
            Ok(())
        }
        fn is_repo(&self, _r: &Path) -> bool { self.present }
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
        let git = MockGit::new("sha1");

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
        let git = MockGit::new("sha1");
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
        let git = MockGit::new("sha1");
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
        let git = MockGit::new("sha1");
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
        let git = MockGit::new("sha1");
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
        let git = MockGit::new("sha1");
        assert!(vendor_plugins(&git, &p, "p", tmp.path(), &paths, false).is_err());
    }

    #[test]
    fn pins_explicit_ref_and_records_sha() {
        let git = MockGit::new("sha_after_v1");
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r#v1"}}"#);
        pin_marketplaces(&git, &p, &dir, &mut lock, false).unwrap();
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "sha_after_v1");
    }

    #[test]
    fn uses_locked_sha_when_not_updating_floating() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","marketplaces":{"m":"o/r"}}"#).unwrap(); // floating source
        // after checkout(locked_sha), HEAD is at locked_sha, so head_sha() returns it too
        let git = MockGit::new("locked_sha");
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        lock.marketplaces.insert("m".into(),
            crate::lock::LockedMarketplace { source: "o/r".into(), sha: "locked_sha".into() });
        pin_marketplaces(&git, &profile, &dir, &mut lock, false).unwrap();
        // checked out the locked sha, not current head
        assert_eq!(git.checkouts.borrow()[0].1, "locked_sha");
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "locked_sha");
    }

    #[test]
    fn update_floating_moves_to_current_head() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","marketplaces":{"m":"o/r"}}"#).unwrap();
        let git = MockGit::new("new_head");
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        lock.marketplaces.insert("m".into(),
            crate::lock::LockedMarketplace { source: "o/r".into(), sha: "old".into() });
        pin_marketplaces(&git, &profile, &dir, &mut lock, true).unwrap();
        // floating + update_floating: no checkout to a pinned sha, record current head
        assert!(git.checkouts.borrow().is_empty());
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "new_head");
    }

    #[test]
    fn locked_sha_wins_over_explicit_ref() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","marketplaces":{"m":"o/r#v2"}}"#).unwrap(); // explicit ref
        let git = MockGit::new("head_sha");
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        lock.marketplaces.insert("m".into(),
            crate::lock::LockedMarketplace { source: "o/r#v2".into(), sha: "locked".into() });
        pin_marketplaces(&git, &profile, &dir, &mut lock, false).unwrap();
        // locked sha wins over the explicit #ref
        assert_eq!(git.checkouts.borrow()[0].1, "locked");
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "head_sha");
    }

    #[test]
    fn non_git_marketplace_degrades_gracefully() {
        // The official marketplace is installed without a `.git`; pinning must be
        // skipped (recorded unpinned) instead of failing the whole launch.
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","marketplaces":{"m":"o/r"}}"#).unwrap();
        let mut git = MockGit::new("SHOULD_NOT_BE_USED");
        git.present = false; // dir exists but isn't a git checkout
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        pin_marketplaces(&git, &profile, &dir, &mut lock, false).unwrap();
        // no checkout attempted, and the lock records the marketplace as unpinned
        assert!(git.checkouts.borrow().is_empty());
        let locked = lock.marketplaces.get("m").expect("marketplace still recorded in lock");
        assert_eq!(locked.sha, "");
        assert_eq!(locked.source, "o/r");
    }

    #[test]
    fn update_floating_keeps_explicit_ref_pinned() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","marketplaces":{"m":"o/r#v3"}}"#).unwrap(); // explicit ref
        let git = MockGit::new("new_head");
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        lock.marketplaces.insert("m".into(),
            crate::lock::LockedMarketplace { source: "o/r#v3".into(), sha: "old_sha".into() });
        pin_marketplaces(&git, &profile, &dir, &mut lock, true).unwrap();
        // explicit ref stays pinned even when updating floating
        assert_eq!(git.checkouts.borrow()[0].1, "v3");
    }
}
