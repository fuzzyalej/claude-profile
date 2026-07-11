use crate::git::GitCli;
use crate::lock::Lockfile;
use crate::profile::Profile;
use crate::provision::pin_marketplaces;
use crate::claude::Marketplace;
use std::path::PathBuf;

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

pub fn reresolve_profile<G: GitCli>(
    git: &G,
    profile: &Profile,
    mkt_dirs: &dyn Fn(&str) -> PathBuf,
    lock: &mut Lockfile,
) -> anyhow::Result<()> {
    let no_mkts: Vec<Marketplace> = Vec::new();
    pin_marketplaces(git, profile, &no_mkts, mkt_dirs, lock, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitCli;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    struct MockGit { head: String, checkouts: RefCell<Vec<String>> }
    impl GitCli for MockGit {
        fn clone(&self, _u: &str, _d: &Path) -> anyhow::Result<()> { Ok(()) }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok(self.head.clone()) }
        fn is_repo(&self, _r: &Path) -> bool { true }
        fn checkout(&self, _r: &Path, gr: &str) -> anyhow::Result<()> { self.checkouts.borrow_mut().push(gr.into()); Ok(()) }
    }
    fn prof(json: &str) -> crate::profile::Profile { crate::profile::Profile::from_json_str(json).unwrap() }

    #[test]
    fn reresolve_moves_floating_to_head() {
        let p = prof(r#"{"name":"p","marketplaces":{"m":"o/r"}}"#);
        let git = MockGit { head: "newhead".into(), checkouts: RefCell::new(vec![]) };
        let mut lock = crate::lock::Lockfile::new("p");
        lock.marketplaces.insert("m".into(), crate::lock::LockedMarketplace { source: "o/r".into(), sha: "old".into() });
        reresolve_profile(&git, &p, &|_| PathBuf::from("/m"), &mut lock).unwrap();
        assert!(git.checkouts.borrow().is_empty()); // floating: no checkout to a pin
        assert_eq!(lock.marketplaces.get("m").unwrap().sha, "newhead"); // moved to HEAD
    }

    #[test]
    fn frozen_check_errors_on_stale_lock() {
        let p = prof(r#"{"name":"p","marketplaces":{"m1":"o/r","m2":"o/s"}}"#);
        let mut lf = crate::lock::Lockfile::new("p");
        lf.marketplaces.insert("m1".into(), crate::lock::LockedMarketplace { source: "o/r".into(), sha: "a".into() });
        let err = frozen_check(&[("p".into(), p, lf)]);
        assert!(err.is_err()); // m2 unlocked → stale
    }

    #[test]
    fn frozen_check_passes_when_all_locked() {
        let p = prof(r#"{"name":"p","marketplaces":{"m":"o/r"}}"#);
        let mut lf = crate::lock::Lockfile::new("p");
        lf.marketplaces.insert("m".into(), crate::lock::LockedMarketplace { source: "o/r".into(), sha: "a".into() });
        assert!(frozen_check(&[("p".into(), p, lf)]).is_ok());
    }
}
