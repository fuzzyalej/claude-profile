use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub plugin: String,
    pub marketplace: String,
    pub repo: String,
    pub description: String,
    pub category: Option<String>,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    plugins: Vec<PluginEntry>,
}

#[derive(Deserialize)]
struct PluginEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

pub fn normalize_manifest(json: &str, repo: &str) -> anyhow::Result<Vec<IndexEntry>> {
    let m: Manifest = serde_json::from_str(json)?;
    let marketplace = m.name.unwrap_or_else(|| {
        repo.rsplit('/').next().unwrap_or(repo).to_string()
    });
    let entries = m
        .plugins
        .into_iter()
        .filter_map(|p| {
            let plugin = p.name?;
            Some(IndexEntry {
                plugin,
                marketplace: marketplace.clone(),
                repo: repo.to_string(),
                description: p.description.unwrap_or_default(),
                category: p.category,
            })
        })
        .collect();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
      "name": "official",
      "plugins": [
        { "name": "pyright-lsp", "description": "Python LSP", "source": "./plugins/pyright-lsp", "category": "language" },
        { "name": "github", "description": "GitHub MCP", "source": { "source": "url", "url": "https://x/y.git" } },
        { "description": "no name, skipped" }
      ]
    }"#;

    #[test]
    fn normalizes_all_source_shapes_and_skips_nameless() {
        let entries = normalize_manifest(MANIFEST, "anthropics/claude-plugins-official").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], IndexEntry {
            plugin: "pyright-lsp".into(),
            marketplace: "official".into(),
            repo: "anthropics/claude-plugins-official".into(),
            description: "Python LSP".into(),
            category: Some("language".into()),
        });
        // marketplace repo wins even when the plugin points elsewhere:
        assert_eq!(entries[1].repo, "anthropics/claude-plugins-official");
        assert_eq!(entries[1].category, None);
    }

    #[test]
    fn falls_back_to_repo_segment_when_manifest_unnamed() {
        let j = r#"{ "plugins": [ { "name": "p", "description": "d" } ] }"#;
        let entries = normalize_manifest(j, "obra/superpowers-marketplace").unwrap();
        assert_eq!(entries[0].marketplace, "superpowers-marketplace");
    }
}
