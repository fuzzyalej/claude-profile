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
    vendor_plugins(git, profile, profile_key, cwd, paths, true, lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitCli;
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path as StdPath;

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
        let mkt_dir = paths.marketplace_clone_dir("m");
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
