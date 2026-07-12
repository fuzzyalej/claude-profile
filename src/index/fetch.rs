use crate::fs_paths::Paths;
use crate::git::GitCli;

pub fn cache_dir_name(repo: &str) -> String {
    repo.replace('/', "--")
}

pub fn manifest_json<G: GitCli>(git: &G, paths: &Paths, repo: &str) -> anyhow::Result<String> {
    let dest = paths.index_repos_dir().join(cache_dir_name(repo));
    let _ = std::fs::remove_dir_all(&dest);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = format!("https://github.com/{repo}.git");
    git.sparse_fetch(&url, &dest, ".claude-plugin")?;
    let manifest = dest.join(".claude-plugin/marketplace.json");
    Ok(std::fs::read_to_string(manifest)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::Path;

    struct FakeGit { calls: RefCell<Vec<(String, String)>> }
    impl GitCli for FakeGit {
        fn clone(&self, _u: &str, _d: &Path) -> anyhow::Result<()> { Ok(()) }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok("sha".into()) }
        fn checkout(&self, _r: &Path, _g: &str) -> anyhow::Result<()> { Ok(()) }
        fn is_repo(&self, _r: &Path) -> bool { true }
        fn sparse_fetch(&self, u: &str, d: &Path, _s: &str) -> anyhow::Result<()> {
            self.calls.borrow_mut().push((u.into(), d.to_string_lossy().into()));
            // simulate the fetched manifest landing on disk
            std::fs::create_dir_all(d.join(".claude-plugin")).unwrap();
            std::fs::write(d.join(".claude-plugin/marketplace.json"),
                r#"{ "name": "m", "plugins": [] }"#).unwrap();
            Ok(())
        }
    }

    #[test]
    fn cache_dir_name_flattens_slash() {
        assert_eq!(cache_dir_name("owner/repo"), "owner--repo");
    }

    #[test]
    fn fetches_via_sparse_when_no_local_clone() {
        let tmp = std::env::temp_dir().join(format!("cpf-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let paths = Paths::from_home(tmp.clone());
        let git = FakeGit { calls: RefCell::new(vec![]) };
        let json = manifest_json(&git, &paths, "owner/repo").unwrap();
        assert!(json.contains("\"name\": \"m\""));
        assert_eq!(git.calls.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
