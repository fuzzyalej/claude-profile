use crate::fs_paths::Paths;
use crate::lock::lock_path;
use crate::resolve::{resolve, ProfileSource};
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plans_profile_removal_with_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let updir = home.join(".claude-profiles");
        fs::create_dir_all(&updir).unwrap();
        fs::write(updir.join("foo.json"), r#"{"name":"foo"}"#).unwrap();
        let paths = crate::fs_paths::Paths::from_home(home);
        let plan = remove_target("foo", &paths, tmp.path(), None, &tmp.path().join("bundled")).unwrap();
        match plan {
            RemovePlan::Profile { json, lock, vendor } => {
                assert!(json.ends_with(".claude-profiles/foo.json"));
                assert!(lock.ends_with(".claude-profiles/foo.lock"));
                assert!(vendor.ends_with(".claude-profiles/store/foo/vendor"));
            }
            _ => panic!("expected Profile plan"),
        }
    }

    #[test]
    fn refuses_bundled_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("bundled");
        fs::create_dir_all(&bundled).unwrap();
        fs::write(bundled.join("demo.json"), r#"{"name":"demo"}"#).unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().join("home"));
        let err = remove_target("demo", &paths, tmp.path(), None, &bundled);
        assert!(err.is_err());
    }

    #[test]
    fn refuses_pack_internal_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let profiles_dir = home.join(".claude-profiles/packs/o--r/profiles");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(profiles_dir.join("foo.json"), r#"{"name":"foo"}"#).unwrap();
        let paths = crate::fs_paths::Paths::from_home(home);
        let err = remove_target("foo", &paths, tmp.path(), None, &tmp.path().join("bundled"));
        assert!(err.is_err());
    }

    #[test]
    fn plans_pack_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let packdir = home.join(".claude-profiles/packs/o--r");
        fs::create_dir_all(&packdir).unwrap();
        let paths = crate::fs_paths::Paths::from_home(home);
        let plan = remove_target("o/r", &paths, tmp.path(), None, &tmp.path().join("bundled")).unwrap();
        match plan {
            RemovePlan::Pack { dir } => assert!(dir.ends_with("packs/o--r")),
            _ => panic!("expected Pack plan"),
        }
    }

    #[test]
    fn apply_deletes_profile_and_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let json = tmp.path().join("foo.json");
        let lock = tmp.path().join("foo.lock");
        let vendor = tmp.path().join("vendor");
        fs::write(&json, "{}").unwrap();
        fs::write(&lock, "{}").unwrap();
        apply(&RemovePlan::Profile { json: json.clone(), lock: lock.clone(), vendor }).unwrap();
        assert!(!json.exists());
        assert!(!lock.exists());
    }

    #[test]
    fn apply_deletes_pack_dir_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let pack_dir = tmp.path().join("o--r");
        let profiles_dir = pack_dir.join("profiles");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(profiles_dir.join("a.json"), r#"{"name":"a"}"#).unwrap();
        apply(&RemovePlan::Pack { dir: pack_dir.clone() }).unwrap();
        assert!(!pack_dir.exists());
    }

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
}
