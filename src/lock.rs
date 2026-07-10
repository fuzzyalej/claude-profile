use crate::fs_paths::Paths;
use crate::profile;
use crate::resolve::ProfileSource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedMarketplace {
    pub source: String,
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    pub profile: String,
    #[serde(default)]
    pub marketplaces: BTreeMap<String, LockedMarketplace>,
}

impl Lockfile {
    pub fn new(profile: &str) -> Lockfile {
        Lockfile { profile: profile.to_string(), marketplaces: BTreeMap::new() }
    }

    pub fn load(path: &Path) -> anyhow::Result<Option<Lockfile>> {
        if !path.is_file() {
            return Ok(None);
        }
        let body = std::fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&body)?))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    #[allow(dead_code)] // Phase 3: --frozen hard-fail staleness enforcement (not yet wired)
    pub fn is_stale_against(&self, profile: &profile::Profile) -> bool {
        profile.marketplaces.keys().any(|name| !self.marketplaces.contains_key(name))
    }
}

pub fn lock_path(profile_name: &str, profile_path: &Path, source: &ProfileSource, paths: &Paths) -> PathBuf {
    match source {
        ProfileSource::Pack(_) | ProfileSource::ExampleDir => {
            paths.locks_dir().join(format!("{profile_name}.lock"))
        }
        _ => profile_path.with_extension("lock"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Lockfile, LockedMarketplace, lock_path};
    use crate::resolve::ProfileSource;

    #[test]
    fn round_trips_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.lock");
        let mut lf = Lockfile::new("x");
        lf.marketplaces.insert("m".into(), LockedMarketplace { source: "o/r".into(), sha: "abc123".into() });
        lf.save(&path).unwrap();
        let loaded = Lockfile::load(&path).unwrap().unwrap();
        assert_eq!(loaded.profile, "x");
        assert_eq!(loaded.marketplaces.get("m").unwrap().sha, "abc123");
    }

    #[test]
    fn load_missing_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(Lockfile::load(&tmp.path().join("nope.lock")).unwrap().is_none());
    }

    #[test]
    fn lock_path_sibling_for_userdir_locksdir_for_pack() {
        let paths = crate::fs_paths::Paths::from_home(std::path::PathBuf::from("/h"));
        let ppath = std::path::PathBuf::from("/h/.claude-profiles/foo.json");
        let sib = lock_path("foo", &ppath, &ProfileSource::UserDir, &paths);
        assert_eq!(sib, std::path::PathBuf::from("/h/.claude-profiles/foo.lock"));
        let pack = lock_path("foo", &ppath, &ProfileSource::Pack("o--r".into()), &paths);
        assert_eq!(pack, std::path::PathBuf::from("/h/.claude-profiles/locks/foo.lock"));
    }

    #[test]
    fn staleness_detects_unlocked_marketplace() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","marketplaces":{"m1":"o/r","m2":"o/s"}}"#).unwrap();
        let mut lf = Lockfile::new("p");
        lf.marketplaces.insert("m1".into(), LockedMarketplace { source: "o/r".into(), sha: "a".into() });
        assert!(lf.is_stale_against(&profile)); // m2 not locked
        lf.marketplaces.insert("m2".into(), LockedMarketplace { source: "o/s".into(), sha: "b".into() });
        assert!(!lf.is_stale_against(&profile));
    }
}
