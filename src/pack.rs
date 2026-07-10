use crate::fs_paths::Paths;
use crate::git::{parse_repo_ref, GitCli};
use crate::profile::Profile;
use std::path::{Path, PathBuf};

fn packs_dir(paths: &Paths) -> PathBuf {
    paths.user_profiles_dir().join("packs")
}

pub fn install_pack<G: GitCli>(git: &G, spec: &str, paths: &Paths) -> anyhow::Result<PathBuf> {
    let repo_ref = parse_repo_ref(spec)?;
    let dest = packs_dir(paths).join(repo_ref.pack_dir_name());
    if dest.is_dir() {
        git.pull(&dest)?;
    } else {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        git.clone(&repo_ref.clone_url(), &dest)?;
    }
    if let Some(ref r) = repo_ref.git_ref {
        git.checkout(&dest, r)?;
    }
    Ok(dest)
}

pub fn update_all_packs<G: GitCli>(git: &G, paths: &Paths) -> anyhow::Result<Vec<String>> {
    let mut updated = Vec::new();
    let dir = packs_dir(paths);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                git.pull(&entry.path())?;
                updated.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    Ok(updated)
}

pub fn default_profile_name(pack_dir: &Path) -> anyhow::Result<String> {
    let root = pack_dir.join("profile.json");
    if root.is_file() {
        let p = Profile::from_json_str(&std::fs::read_to_string(&root)?)?;
        return Ok(p.name);
    }
    let profiles = pack_dir.join("profiles");
    let mut found: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&profiles) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                found.push(path.file_stem().unwrap().to_string_lossy().to_string());
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => anyhow::bail!("pack has no profiles"),
        _ => anyhow::bail!("pack has multiple profiles; specify one by name"),
    }
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
        pulled: RefCell<Vec<PathBuf>>,
        checkouts: RefCell<Vec<(PathBuf, String)>>,
    }
    impl GitCli for MockGit {
        fn clone(&self, u: &str, d: &Path) -> anyhow::Result<()> {
            self.cloned.borrow_mut().push((u.into(), d.to_path_buf()));
            fs::create_dir_all(d).unwrap(); Ok(())
        }
        fn pull(&self, r: &Path) -> anyhow::Result<()> { self.pulled.borrow_mut().push(r.to_path_buf()); Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok("sha".into()) }
        fn checkout(&self, r: &Path, g: &str) -> anyhow::Result<()> {
            self.checkouts.borrow_mut().push((r.to_path_buf(), g.into()));
            Ok(())
        }
    }

    #[test]
    fn install_pack_clones_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        let dir = install_pack(&git, "fuzzyalej/rust-profile", &paths).unwrap();
        assert!(dir.ends_with("packs/fuzzyalej--rust-profile"));
        assert_eq!(git.cloned.borrow().len(), 1);
        assert_eq!(git.cloned.borrow()[0].0, "https://github.com/fuzzyalej/rust-profile.git");
    }

    #[test]
    fn install_pack_pulls_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let dest = paths.user_profiles_dir().join("packs").join("o--r");
        fs::create_dir_all(&dest).unwrap();
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        install_pack(&git, "o/r", &paths).unwrap();
        assert!(git.cloned.borrow().is_empty());
        assert_eq!(git.pulled.borrow().len(), 1);
    }

    #[test]
    fn install_pack_checks_out_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        let dir = install_pack(&git, "o/r#v1.2.3", &paths).unwrap();
        let checkouts = git.checkouts.borrow();
        assert_eq!(checkouts.len(), 1);
        assert_eq!(checkouts[0].0, dir);
        assert_eq!(checkouts[0].1, "v1.2.3");
    }

    #[test]
    fn install_pack_checks_out_ref_after_pull() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let dest = paths.user_profiles_dir().join("packs").join("o--r");
        fs::create_dir_all(&dest).unwrap();
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        install_pack(&git, "o/r#tag1", &paths).unwrap();
        assert!(git.cloned.borrow().is_empty());
        assert_eq!(git.pulled.borrow().len(), 1);
        let checkouts = git.checkouts.borrow();
        assert_eq!(checkouts.len(), 1);
        assert_eq!(checkouts[0].1, "tag1");
    }

    #[test]
    fn update_all_packs_pulls_each_and_returns_names() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let packs = paths.user_profiles_dir().join("packs");
        fs::create_dir_all(packs.join("a--b")).unwrap();
        fs::create_dir_all(packs.join("c--d")).unwrap();
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        let mut updated = update_all_packs(&git, &paths).unwrap();
        updated.sort();
        assert_eq!(git.pulled.borrow().len(), 2);
        assert_eq!(updated, vec!["a--b".to_string(), "c--d".to_string()]);
    }

    #[test]
    fn default_profile_from_single_profiles_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("only.json"), r#"{"name":"only"}"#).unwrap();
        assert_eq!(default_profile_name(tmp.path()).unwrap(), "only");
    }

    #[test]
    fn default_profile_from_root_profile_json() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("profile.json"), r#"{"name":"rooted"}"#).unwrap();
        assert_eq!(default_profile_name(tmp.path()).unwrap(), "rooted");
    }

    #[test]
    fn default_profile_name_errors_on_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        assert!(default_profile_name(tmp.path()).is_err());
    }

    #[test]
    fn default_profile_name_errors_on_many() {
        let tmp = tempfile::tempdir().unwrap();
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("one.json"), r#"{"name":"one"}"#).unwrap();
        fs::write(profiles.join("two.json"), r#"{"name":"two"}"#).unwrap();
        assert!(default_profile_name(tmp.path()).is_err());
    }

    #[test]
    fn default_profile_name_prefers_root_profile_json() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("profile.json"), r#"{"name":"rooted"}"#).unwrap();
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("other.json"), r#"{"name":"other"}"#).unwrap();
        assert_eq!(default_profile_name(tmp.path()).unwrap(), "rooted");
    }
}
