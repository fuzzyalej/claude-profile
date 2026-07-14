use crate::claude::{ClaudeCli, InstalledPlugin, Marketplace};
use crate::profile::Profile;
use std::io::Write;
use crate::git::{parse_repo_ref, GitCli};
use crate::lock::{Lockfile, LockedMarketplace};
use std::path::PathBuf;
use crate::spinner::spin;

pub struct Plan {
    pub marketplaces: Vec<(String, String)>,
    pub plugins: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.marketplaces.is_empty() && self.plugins.is_empty()
    }
}

pub fn compute_plan(profile: &Profile, installed_mkts: &[Marketplace], installed_plugins: &[InstalledPlugin]) -> Plan {
    let have_mkt: std::collections::BTreeSet<&str> =
        installed_mkts.iter().map(|m| m.name.as_str()).collect();
    let have_plugin: std::collections::BTreeSet<&str> =
        installed_plugins.iter().map(|p| p.id.as_str()).collect();

    let marketplaces = profile.marketplaces.iter()
        .filter(|(name, _)| !have_mkt.contains(name.as_str()))
        .map(|(name, src)| (name.clone(), src.clone()))
        .collect();
    let plugins = profile.plugins.iter()
        .filter(|id| !have_plugin.contains(id.as_str()))
        .cloned()
        .collect();
    Plan { marketplaces, plugins }
}

fn confirm(plan: &Plan) -> bool {
    println!("claude-profile will install the following into your user scope:");
    for (name, src) in &plan.marketplaces {
        println!("  marketplace: {name}  (source: {src})");
    }
    for id in &plan.plugins {
        println!("  plugin:      {id}");
    }
    print!("Proceed? [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes")
}

pub fn provision<C: ClaudeCli>(cli: &C, profile: &Profile, assume_yes: bool) -> anyhow::Result<()> {
    let mkts = cli.list_marketplaces()?;
    let plugins = cli.list_plugins()?;
    let plan = compute_plan(profile, &mkts, &plugins);
    if plan.is_empty() {
        return Ok(());
    }
    if !assume_yes && !confirm(&plan) {
        anyhow::bail!("provisioning declined by user");
    }
    for (name, src) in &plan.marketplaces {
        spin(
            &format!("Adding marketplace {name}..."),
            &format!("✔ Added marketplace {name}"),
            || cli.marketplace_add(src),
        )?;
    }
    for id in &plan.plugins {
        spin(
            &format!("Installing plugin {id}..."),
            &format!("✔ Installed plugin {id}"),
            || cli.install_plugin(id),
        )?;
    }
    Ok(())
}

pub fn pin_marketplaces<G: GitCli>(
    git: &G,
    profile: &Profile,
    _installed_mkts: &[Marketplace],
    mkt_install_dir: &dyn Fn(&str) -> PathBuf,
    lock: &mut Lockfile,
    update_floating: bool,
) -> anyhow::Result<()> {
    for (name, source) in &profile.marketplaces {
        let repo_ref = parse_repo_ref(source)?;
        let dir = mkt_install_dir(name);

        // Some marketplaces (e.g. the official `anthropics/claude-plugins-official`)
        // are installed without a `.git`, so there is no HEAD to pin. Record the
        // marketplace as unpinned and continue rather than failing the launch.
        if !git.is_repo(&dir) {
            eprintln!(
                "WARNING: marketplace '{name}' at {} is not a git checkout; recording unpinned (not reproducible)",
                dir.display()
            );
            lock.marketplaces
                .insert(name.clone(), LockedMarketplace { source: source.clone(), sha: String::new() });
            continue;
        }

        // Determine the target commit-ish to check out (None = leave at current HEAD).
        let target = checkout_target(update_floating, lock.marketplaces.get(name), &repo_ref.git_ref);
        if let Some(ref t) = target {
            git.checkout(&dir, t)?;
        }
        let sha = git.head_sha(&dir)?;
        lock.marketplaces.insert(name.clone(), LockedMarketplace { source: source.clone(), sha });
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockCli {
        plugins: Vec<InstalledPlugin>,
        mkts: Vec<Marketplace>,
        added: RefCell<Vec<String>>,
        installed: RefCell<Vec<String>>,
        uninstalled: RefCell<Vec<String>>,
        removed_mkts: RefCell<Vec<String>>,
    }
    impl ClaudeCli for MockCli {
        fn list_plugins(&self) -> anyhow::Result<Vec<InstalledPlugin>> { Ok(self.plugins.clone()) }
        fn list_marketplaces(&self) -> anyhow::Result<Vec<Marketplace>> { Ok(self.mkts.clone()) }
        fn marketplace_add(&self, s: &str) -> anyhow::Result<()> { self.added.borrow_mut().push(s.into()); Ok(()) }
        fn install_plugin(&self, id: &str) -> anyhow::Result<()> { self.installed.borrow_mut().push(id.into()); Ok(()) }
        fn uninstall_plugin(&self, id: &str) -> anyhow::Result<()> { self.uninstalled.borrow_mut().push(id.into()); Ok(()) }
        fn marketplace_remove(&self, n: &str) -> anyhow::Result<()> { self.removed_mkts.borrow_mut().push(n.into()); Ok(()) }
    }

    fn profile(json: &str) -> crate::profile::Profile {
        crate::profile::Profile::from_json_str(json).unwrap()
    }

    #[test]
    fn plan_lists_only_missing_marketplaces_and_plugins() {
        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r","have":"o/h"},
            "plugins":["new@m","present@have"]}"#);
        let mkts = vec![Marketplace { name: "have".into() }];
        let installed = vec![InstalledPlugin {
            id: "present@have".into(), enabled: false, scope: "user".into(),
            mcp_servers: serde_json::json!({}) }];
        let plan = compute_plan(&p, &mkts, &installed);
        assert_eq!(plan.marketplaces, vec![("m".to_string(), "o/r".to_string())]);
        assert_eq!(plan.plugins, vec!["new@m".to_string()]);
    }

    #[test]
    fn provision_with_assume_yes_applies_plan() {
        let p = profile(r#"{"name":"p","marketplaces":{"m":"o/r"},"plugins":["new@m"]}"#);
        let cli = MockCli { plugins: vec![], mkts: vec![], added: RefCell::new(vec![]), installed: RefCell::new(vec![]), uninstalled: RefCell::new(vec![]), removed_mkts: RefCell::new(vec![]) };
        provision(&cli, &p, true).unwrap();
        assert_eq!(*cli.added.borrow(), vec!["o/r".to_string()]);
        assert_eq!(*cli.installed.borrow(), vec!["new@m".to_string()]);
    }

    use crate::git::GitCli;
    use std::path::{Path, PathBuf};

    struct MockGit {
        head: String,
        checkouts: RefCell<Vec<(PathBuf, String)>>,
        present: bool,
    }
    impl MockGit {
        fn new(head: &str) -> Self {
            MockGit { head: head.into(), checkouts: RefCell::new(vec![]), present: true }
        }
    }
    impl GitCli for MockGit {
        fn clone(&self, _u: &str, _d: &Path) -> anyhow::Result<()> { Ok(()) }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok(self.head.clone()) }
        fn is_repo(&self, _r: &Path) -> bool { self.present }
        fn checkout(&self, r: &Path, gr: &str) -> anyhow::Result<()> {
            self.checkouts.borrow_mut().push((r.to_path_buf(), gr.to_string())); Ok(())
        }
        fn sparse_fetch(&self, _u: &str, _d: &Path, _s: &str) -> anyhow::Result<()> { Ok(()) }
    }

    #[test]
    fn pins_explicit_ref_and_records_sha() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","marketplaces":{"m":"o/r#v1"}}"#).unwrap();
        let git = MockGit::new("sha_after_v1");
        let dir = |_n: &str| PathBuf::from("/mkts/m");
        let mut lock = crate::lock::Lockfile::new("p");
        pin_marketplaces(&git, &profile, &[], &dir, &mut lock, false).unwrap();
        assert_eq!(git.checkouts.borrow()[0], (PathBuf::from("/mkts/m"), "v1".to_string()));
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "sha_after_v1");
        assert_eq!(lock.marketplaces.get("m").unwrap().source, "o/r#v1");
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
        pin_marketplaces(&git, &profile, &[], &dir, &mut lock, false).unwrap();
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
        pin_marketplaces(&git, &profile, &[], &dir, &mut lock, true).unwrap();
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
        pin_marketplaces(&git, &profile, &[], &dir, &mut lock, false).unwrap();
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
        pin_marketplaces(&git, &profile, &[], &dir, &mut lock, false).unwrap();
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
        pin_marketplaces(&git, &profile, &[], &dir, &mut lock, true).unwrap();
        // explicit ref stays pinned even when updating floating
        assert_eq!(git.checkouts.borrow()[0].1, "v3");
    }
}
