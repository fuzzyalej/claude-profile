pub mod model;
pub mod search;
pub mod default_seeds;
pub mod seeds;
pub mod fetch;

use crate::fs_paths::Paths;
use crate::git::GitCli;
use model::IndexEntry;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Index {
    pub generated_at: String,
    pub entries: Vec<IndexEntry>,
}

pub struct SyncReport {
    pub marketplaces: usize,
    pub skipped: usize,
    pub plugins: usize,
}

pub fn sync<G: GitCli>(git: &G, paths: &Paths) -> anyhow::Result<SyncReport> {
    let seeds = seeds::resolve(paths);
    let mut entries: Vec<IndexEntry> = Vec::new();
    let mut ok = 0usize;
    let mut skipped = 0usize;
    for repo in &seeds {
        match fetch::manifest_json(git, paths, repo)
            .and_then(|j| model::normalize_manifest(&j, repo))
        {
            Ok(mut es) => {
                ok += 1;
                entries.append(&mut es);
            }
            Err(e) => {
                skipped += 1;
                eprintln!("warning: skipped marketplace {repo}: {e}");
            }
        }
    }
    let index = Index {
        generated_at: now_stamp(),
        entries,
    };
    std::fs::create_dir_all(paths.index_cache_dir())?;
    std::fs::write(paths.index_file(), serde_json::to_string_pretty(&index)?)?;
    Ok(SyncReport { marketplaces: ok, skipped, plugins: index.entries.len() })
}

pub fn load(paths: &Paths) -> anyhow::Result<Index> {
    let body = std::fs::read_to_string(paths.index_file())?;
    Ok(serde_json::from_str(&body)?)
}

fn now_stamp() -> String {
    // No chrono dependency: seconds since epoch is enough to show staleness.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_paths::Paths;
    use crate::git::GitCli;
    use std::path::Path;

    struct OneMarketplaceGit;
    impl GitCli for OneMarketplaceGit {
        fn clone(&self, _u: &str, _d: &Path) -> anyhow::Result<()> { Ok(()) }
        fn pull(&self, _r: &Path) -> anyhow::Result<()> { Ok(()) }
        fn head_sha(&self, _r: &Path) -> anyhow::Result<String> { Ok("s".into()) }
        fn checkout(&self, _r: &Path, _g: &str) -> anyhow::Result<()> { Ok(()) }
        fn is_repo(&self, _r: &Path) -> bool { true }
        fn sparse_fetch(&self, _u: &str, d: &Path, _s: &str) -> anyhow::Result<()> {
            std::fs::create_dir_all(d.join(".claude-plugin")).unwrap();
            std::fs::write(d.join(".claude-plugin/marketplace.json"),
                r#"{ "name": "m", "plugins": [ { "name": "p", "description": "d" } ] }"#).unwrap();
            Ok(())
        }
    }

    #[test]
    fn sync_then_load_roundtrips() {
        let tmp = std::env::temp_dir().join(format!("cpf-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // isolate seeds: empty user file, no installed marketplaces -> just DEFAULT_SEEDS,
        // but network is mocked so every default resolves to the same fixture manifest.
        std::fs::create_dir_all(tmp.join(".claude-profiles")).unwrap();
        std::fs::write(tmp.join(".claude-profiles/marketplaces.txt"), "solo/mkt\n").unwrap();
        let paths = Paths::from_home(tmp.clone());
        let git = OneMarketplaceGit;
        let report = sync(&git, &paths).unwrap();
        assert!(report.plugins >= 1);
        let idx = load(&paths).unwrap();
        assert!(idx.entries.iter().any(|e| e.plugin == "p"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
