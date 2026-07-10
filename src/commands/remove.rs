use crate::fs_paths::Paths;
use crate::lock::lock_path;
use crate::resolve::{resolve, ProfileSource};
use std::path::{Path, PathBuf};

pub enum RemovePlan {
    Profile { json: PathBuf, lock: PathBuf },
    Pack { dir: PathBuf },
}

pub fn remove_target(
    name_or_repo: &str,
    paths: &Paths,
    cwd: &Path,
    env_dir: Option<&Path>,
    examples_dir: &Path,
) -> anyhow::Result<RemovePlan> {
    if name_or_repo.contains('/') {
        let repo_ref = crate::git::parse_repo_ref(name_or_repo)?;
        let dir = paths.user_profiles_dir().join("packs").join(repo_ref.pack_dir_name());
        if !dir.is_dir() {
            anyhow::bail!("pack '{name_or_repo}' is not installed");
        }
        return Ok(RemovePlan::Pack { dir });
    }
    let resolved = resolve(name_or_repo, paths, cwd, env_dir, examples_dir)?;
    if matches!(resolved.source, ProfileSource::ExampleDir) {
        anyhow::bail!("cannot remove engine example profile '{name_or_repo}'");
    }
    if matches!(resolved.source, ProfileSource::Pack(_)) {
        anyhow::bail!(
            "'{name_or_repo}' belongs to an installed pack; remove the whole pack with `claude-profile remove <owner/repo>` instead"
        );
    }
    let lock = lock_path(name_or_repo, &resolved.path, &resolved.source, paths);
    Ok(RemovePlan::Profile { json: resolved.path, lock })
}

pub fn apply(plan: &RemovePlan) -> anyhow::Result<()> {
    match plan {
        RemovePlan::Profile { json, lock } => {
            std::fs::remove_file(json)?;
            if lock.exists() {
                std::fs::remove_file(lock)?;
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
        let plan = remove_target("foo", &paths, tmp.path(), None, &tmp.path().join("examples")).unwrap();
        match plan {
            RemovePlan::Profile { json, lock } => {
                assert!(json.ends_with(".claude-profiles/foo.json"));
                assert!(lock.ends_with(".claude-profiles/foo.lock"));
            }
            _ => panic!("expected Profile plan"),
        }
    }

    #[test]
    fn refuses_example_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let examples = tmp.path().join("examples");
        fs::create_dir_all(&examples).unwrap();
        fs::write(examples.join("demo.json"), r#"{"name":"demo"}"#).unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().join("home"));
        let err = remove_target("demo", &paths, tmp.path(), None, &examples);
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
        let err = remove_target("foo", &paths, tmp.path(), None, &tmp.path().join("examples"));
        assert!(err.is_err());
    }

    #[test]
    fn plans_pack_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let packdir = home.join(".claude-profiles/packs/o--r");
        fs::create_dir_all(&packdir).unwrap();
        let paths = crate::fs_paths::Paths::from_home(home);
        let plan = remove_target("o/r", &paths, tmp.path(), None, &tmp.path().join("examples")).unwrap();
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
        fs::write(&json, "{}").unwrap();
        fs::write(&lock, "{}").unwrap();
        apply(&RemovePlan::Profile { json: json.clone(), lock: lock.clone() }).unwrap();
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
}
