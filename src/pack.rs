use crate::fs_paths::Paths;
use crate::git::{parse_repo_ref, GitCli};
use crate::profile::Profile;
use std::fs;
use std::path::{Path, PathBuf};

fn packs_dir(paths: &Paths) -> PathBuf {
    paths.user_profiles_dir().join("packs")
}

/// Profile files in a cloned repo, as (source path, path relative to the pack root):
/// a root `profile.json` and every `profiles/*.json`.
fn collect_profile_files(repo: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    let root = repo.join("profile.json");
    if root.is_file() {
        out.push((root, PathBuf::from("profile.json")));
    }
    if let Ok(entries) = fs::read_dir(repo.join("profiles")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let rel = Path::new("profiles").join(path.file_name().unwrap());
                out.push((path, rel));
            }
        }
    }
    out
}

/// Copy only the profile files from a cloned repo into `dest`, discarding everything
/// else. Aborts if the repo has no profiles at all.
fn materialize_profiles(src: &Path, dest: &Path, spec: &str) -> anyhow::Result<()> {
    let files = collect_profile_files(src);
    if files.is_empty() {
        anyhow::bail!(
            "repo '{spec}' contains no profiles (expected a profile.json or profiles/*.json); nothing to install"
        );
    }
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;
    for (from, rel) in files {
        let to = dest.join(&rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&from, &to)?;
    }
    Ok(())
}

/// Install a profile repo as profiles-only: clone into a temp dir, verify it actually
/// carries profiles (else abort), and copy just the profile files into
/// `~/.claude-profiles/packs/owner--repo/`. The rest of the repo is discarded.
pub fn install_pack<G: GitCli>(git: &G, spec: &str, paths: &Paths) -> anyhow::Result<PathBuf> {
    let repo_ref = parse_repo_ref(spec)?;
    let (_tmp, src) = fetch_ephemeral(git, spec)?;
    let dest = packs_dir(paths).join(repo_ref.pack_dir_name());
    materialize_profiles(&src, &dest, spec)?;
    Ok(dest)
}

/// Clone a profile repo into a throwaway temp dir (auto-removed when the returned
/// `TempDir` drops). Used by `show` to preview a repo without installing it.
pub fn fetch_ephemeral<G: GitCli>(git: &G, spec: &str) -> anyhow::Result<(tempfile::TempDir, PathBuf)> {
    let repo_ref = parse_repo_ref(spec)?;
    let tmp = tempfile::tempdir()?;
    let dest = tmp.path().join(repo_ref.pack_dir_name());
    git.clone(&repo_ref.clone_url(), &dest)?;
    if let Some(ref r) = repo_ref.git_ref {
        git.checkout(&dest, r)?;
    }
    Ok((tmp, dest))
}

pub fn update_all_packs<G: GitCli>(git: &G, paths: &Paths) -> anyhow::Result<Vec<String>> {
    let mut updated = Vec::new();
    let dir = packs_dir(paths);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            // Packs are stored profiles-only (no `.git`), so there's nothing to pull.
            // Skip anything that isn't a git checkout rather than erroring.
            if entry.path().is_dir() && git.is_repo(&entry.path()) {
                git.pull(&entry.path())?;
                updated.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    Ok(updated)
}

/// Load the pack's default profile (root `profile.json`, or its sole `profiles/*.json`).
pub fn read_default_profile(pack_dir: &Path) -> anyhow::Result<Profile> {
    let root = pack_dir.join("profile.json");
    if root.is_file() {
        return Profile::from_json_str(&std::fs::read_to_string(&root)?);
    }
    let name = default_profile_name(pack_dir)?;
    let path = pack_dir.join("profiles").join(format!("{name}.json"));
    Profile::from_json_str(&std::fs::read_to_string(&path)?)
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
            // Simulate a repo that carries a profile so install_pack's validation passes.
            fs::create_dir_all(d.join("profiles")).unwrap();
            fs::write(d.join("profiles").join("default.json"), r#"{"name":"default"}"#).unwrap();
            Ok(())
        }
        fn pull(&self, r: &Path) -> anyhow::Result<()> { self.pulled.borrow_mut().push(r.to_path_buf()); Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok("sha".into()) }
        fn is_repo(&self, _r: &Path) -> bool { true }
        fn checkout(&self, r: &Path, g: &str) -> anyhow::Result<()> {
            self.checkouts.borrow_mut().push((r.to_path_buf(), g.into()));
            Ok(())
        }
    }

    #[test]
    fn install_pack_clones_and_keeps_only_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        let dir = install_pack(&git, "fuzzyalej/rust-profile", &paths).unwrap();
        assert!(dir.ends_with("packs/fuzzyalej--rust-profile"));
        assert_eq!(git.cloned.borrow().len(), 1);
        assert_eq!(git.cloned.borrow()[0].0, "https://github.com/fuzzyalej/rust-profile.git");
        // the cloned fixture's profile was copied into the pack
        assert!(dir.join("profiles").join("default.json").is_file());
    }

    #[test]
    fn install_pack_checks_out_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().to_path_buf());
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        install_pack(&git, "o/r#v1.2.3", &paths).unwrap();
        let checkouts = git.checkouts.borrow();
        assert_eq!(checkouts.len(), 1);
        assert_eq!(checkouts[0].1, "v1.2.3");
    }

    #[test]
    fn materialize_profiles_strips_non_profile_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("profiles")).unwrap();
        fs::create_dir_all(src.join("code")).unwrap();
        fs::write(src.join("profile.json"), r#"{"name":"root"}"#).unwrap();
        fs::write(src.join("profiles").join("a.json"), r#"{"name":"a"}"#).unwrap();
        fs::write(src.join("README.md"), "hi").unwrap();
        fs::write(src.join("code").join("lib.rs"), "fn main(){}").unwrap();

        let dest = tmp.path().join("dest");
        materialize_profiles(&src, &dest, "o/r").unwrap();
        assert!(dest.join("profile.json").is_file());
        assert!(dest.join("profiles").join("a.json").is_file());
        assert!(!dest.join("README.md").exists());
        assert!(!dest.join("code").exists());
    }

    #[test]
    fn materialize_profiles_aborts_when_no_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("README.md"), "no profiles here").unwrap();
        let dest = tmp.path().join("dest");
        let err = materialize_profiles(&src, &dest, "o/r").unwrap_err();
        assert!(err.to_string().contains("no profiles"));
        assert!(!dest.exists());
    }

    #[test]
    fn fetch_ephemeral_clones_and_checks_out_into_tempdir() {
        let git = MockGit { cloned: RefCell::new(vec![]), pulled: RefCell::new(vec![]), checkouts: RefCell::new(vec![]) };
        let (tmp, dest) = fetch_ephemeral(&git, "git@ssh.dev.azure.com:v3/Org/Tools/pack#tag1").unwrap();
        assert!(dest.starts_with(tmp.path()));
        assert!(dest.ends_with("Tools--pack"));
        assert_eq!(git.cloned.borrow().len(), 1);
        assert_eq!(git.cloned.borrow()[0].0, "git@ssh.dev.azure.com:v3/Org/Tools/pack");
        assert_eq!(git.checkouts.borrow()[0].1, "tag1");
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
