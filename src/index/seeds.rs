use crate::index::default_seeds::DEFAULT_SEEDS;
use crate::fs_paths::Paths;
use std::collections::BTreeSet;

pub fn parse_user_file(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

/// Extract trailing `owner/repo` (last two path segments) from an ssh/https git URL.
fn owner_repo_from_url(url: &str) -> Option<String> {
    let tail = url.rsplit(['/', ':']).take(2).collect::<Vec<_>>();
    if tail.len() < 2 {
        return None;
    }
    let repo = tail[0].trim_end_matches(".git");
    let owner = tail[1];
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

pub fn repo_from_known_marketplace(source_repo: Option<&str>, source_url: Option<&str>) -> Option<String> {
    if let Some(r) = source_repo {
        if r.contains('/') {
            return Some(r.to_string());
        }
    }
    source_url.and_then(owner_repo_from_url)
}

fn installed_repos(paths: &Paths) -> Vec<String> {
    let path = paths.home.join(".claude/plugins/known_marketplaces.json");
    let Ok(body) = std::fs::read_to_string(&path) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return vec![] };
    let Some(obj) = v.as_object() else { return vec![] };
    obj.values()
        .filter_map(|m| {
            let src = m.get("source")?;
            let repo = src.get("repo").and_then(|r| r.as_str());
            let url = src.get("url").and_then(|u| u.as_str());
            repo_from_known_marketplace(repo, url)
        })
        .collect()
}

pub fn resolve(paths: &Paths) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    let user = std::fs::read_to_string(paths.marketplaces_seed_file())
        .map(|c| parse_user_file(&c))
        .unwrap_or_default();
    let all = DEFAULT_SEEDS
        .iter()
        .map(|s| s.to_string())
        .chain(installed_repos(paths))
        .chain(user);
    for repo in all {
        let key = repo.to_lowercase();
        if seen.insert(key) {
            out.push(repo);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_file_ignoring_comments_and_blanks() {
        let c = "# header\n\nowner/one\n  owner/two  \n# owner/three\n";
        assert_eq!(parse_user_file(c), vec!["owner/one", "owner/two"]);
    }

    #[test]
    fn normalizes_known_marketplace_sources() {
        assert_eq!(repo_from_known_marketplace(Some("fuzzyalej/diagon-alley"), None).as_deref(),
                   Some("fuzzyalej/diagon-alley"));
        assert_eq!(
            repo_from_known_marketplace(None, Some("git@ssh.dev.azure.com:v3/Org/Proj/repo")).as_deref(),
            Some("Proj/repo")
        );
        assert_eq!(
            repo_from_known_marketplace(None, Some("https://github.com/a/b.git")).as_deref(),
            Some("a/b")
        );
    }

    #[test]
    fn resolve_unions_and_dedups() {
        let tmp = std::env::temp_dir().join(format!("cpf-seeds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".claude-profiles")).unwrap();
        std::fs::create_dir_all(tmp.join(".claude/plugins")).unwrap();
        std::fs::write(tmp.join(".claude-profiles/marketplaces.txt"), "me/mine\nobra/superpowers-marketplace\n").unwrap();
        std::fs::write(tmp.join(".claude/plugins/known_marketplaces.json"),
            r#"{ "x": { "source": { "source": "github", "repo": "priv/internal" } } }"#).unwrap();
        let paths = Paths::from_home(tmp.clone());
        let seeds = resolve(&paths);
        assert!(seeds.contains(&"me/mine".to_string()));
        assert!(seeds.contains(&"priv/internal".to_string()));
        assert!(seeds.contains(&"anthropics/claude-plugins-official".to_string())); // from defaults
        // dedup: superpowers appears in both defaults and user file, once only
        assert_eq!(seeds.iter().filter(|s| s.contains("superpowers-marketplace")).count(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
