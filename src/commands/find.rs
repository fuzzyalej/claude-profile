use crate::fs_paths::Paths;
use crate::index;
use crate::index::model::IndexEntry;

fn render_human(entries: &[&IndexEntry]) -> String {
    let mut s = String::new();
    for e in entries {
        s.push_str(&format!("{}@{}   {}\n", e.plugin, e.marketplace, e.description));
        s.push_str(&format!("    repo: {}\n", e.repo));
    }
    s
}

fn render_json(entries: &[&IndexEntry]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(entries)?)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    paths: &Paths,
    query: &[String],
    sync_flag: bool,
    refresh_seeds: bool,
    json: bool,
    limit: Option<usize>,
    marketplace: Option<&str>,
) -> anyhow::Result<i32> {
    let git = crate::git::RealGit;
    if refresh_seeds {
        eprintln!("note: --refresh-seeds harvesting not yet implemented; using existing seeds");
    }
    let index_missing = !paths.index_file().exists();
    if sync_flag || refresh_seeds || index_missing {
        if index_missing && !sync_flag && !refresh_seeds {
            eprintln!("no index found; syncing (this fetches marketplace manifests)…");
        }
        let r = index::sync(&git, paths)?;
        eprintln!(
            "indexed {} plugins from {} marketplaces ({} skipped)",
            r.plugins, r.marketplaces, r.skipped
        );
    }
    if query.is_empty() {
        if sync_flag || refresh_seeds {
            return Ok(0); // sync-only invocation
        }
        anyhow::bail!("give a search query, e.g. `claude-profile find python`");
    }
    let idx = index::load(paths)?;
    let q = query.join(" ");
    let hits = index::search::rank(&idx.entries, &q, marketplace, limit.unwrap_or(20));
    if json {
        println!("{}", render_json(&hits)?);
    } else if hits.is_empty() {
        eprintln!("no matches for '{q}' (index generated {})", idx.generated_at);
    } else {
        print!("{}", render_human(&hits));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::model::IndexEntry;

    #[test]
    fn render_human_lists_id_and_repo() {
        let entries = [IndexEntry {
            plugin: "pyright-lsp".into(),
            marketplace: "official".into(),
            repo: "anthropics/claude-plugins-official".into(),
            description: "Python LSP".into(),
            category: None,
        }];
        let refs: Vec<&IndexEntry> = entries.iter().collect();
        let out = render_human(&refs);
        assert!(out.contains("pyright-lsp@official"));
        assert!(out.contains("anthropics/claude-plugins-official"));
        assert!(out.contains("Python LSP"));
    }

    #[test]
    fn render_json_is_array_of_entries() {
        let entries = [IndexEntry {
            plugin: "p".into(), marketplace: "m".into(), repo: "o/m".into(),
            description: "d".into(), category: None,
        }];
        let refs: Vec<&IndexEntry> = entries.iter().collect();
        let out = render_json(&refs).unwrap();
        assert!(out.trim_start().starts_with('['));
        assert!(out.contains("\"plugin\": \"p\""));
    }
}
