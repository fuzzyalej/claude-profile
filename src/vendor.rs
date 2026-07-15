use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum PluginSource {
    /// Path relative to the marketplace repo root, e.g. "./skills/foo".
    RelativePath(String),
    /// An externally-hosted plugin, referenced by `owner/repo` or a full git URL
    /// (`{"source":"github","repo":...}` or `{"source":"url","url":...}`); the
    /// whole repo is vendored as the plugin's content.
    ExternalRepo { repo: String },
    /// A plugin living in a subdirectory of an external repo, pinned to an
    /// explicit `sha` and/or a floating `ref` (branch/tag) — only `path` within
    /// the repo is vendored, not the whole thing.
    GitSubdir { url: String, path: String, git_ref: Option<String>, sha: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketplacePlugin {
    pub name: String,
    pub source: PluginSource,
}

pub fn parse_marketplace_json(body: &str) -> anyhow::Result<Vec<MarketplacePlugin>> {
    let v: Value = serde_json::from_str(body)?;
    let arr = v
        .get("plugins")
        .and_then(|p| p.as_array())
        .ok_or_else(|| anyhow::anyhow!("marketplace.json missing 'plugins' array"))?;

    let mut out = Vec::new();
    for p in arr {
        let name = p
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow::anyhow!("plugin entry missing 'name': {p}"))?;
        let source = p
            .get("source")
            .ok_or_else(|| anyhow::anyhow!("plugin '{name}' missing 'source'"))?;
        let parsed = match source {
            Value::String(s) => PluginSource::RelativePath(s.clone()),
            Value::Object(o) => parse_object_source(name, o, source)?,
            other => anyhow::bail!("plugin '{name}' has unrecognized source shape: {other}"),
        };
        out.push(MarketplacePlugin { name: name.to_string(), source: parsed });
    }
    Ok(out)
}

fn parse_object_source(
    name: &str,
    o: &serde_json::Map<String, Value>,
    source: &Value,
) -> anyhow::Result<PluginSource> {
    let kind = o.get("source").and_then(|x| x.as_str());
    let repo = o.get("repo").and_then(|x| x.as_str());
    let url = o.get("url").and_then(|x| x.as_str());
    match (kind, repo, url) {
        (Some("github"), Some(r), _) => Ok(PluginSource::ExternalRepo { repo: r.to_string() }),
        (Some("url"), _, Some(u)) => Ok(PluginSource::ExternalRepo { repo: u.to_string() }),
        (Some("git-subdir"), _, Some(u)) => {
            let path = o.get("path").and_then(|x| x.as_str())
                .ok_or_else(|| anyhow::anyhow!("plugin '{name}' git-subdir source missing 'path'"))?;
            let git_ref = o.get("ref").and_then(|x| x.as_str()).map(str::to_string);
            let sha = o.get("sha").and_then(|x| x.as_str()).map(str::to_string);
            Ok(PluginSource::GitSubdir { url: u.to_string(), path: path.to_string(), git_ref, sha })
        }
        _ => anyhow::bail!("plugin '{name}' has unrecognized source shape: {source}"),
    }
}

pub fn find_plugin<'a>(plugins: &'a [MarketplacePlugin], name: &str) -> anyhow::Result<&'a MarketplacePlugin> {
    plugins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| anyhow::anyhow!("plugin '{name}' not found in marketplace"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real shape from a marketplace.json in the wild: one relative-path
    // plugin, one external-repo plugin, in the same file.
    const REAL_MARKETPLACE_JSON: &str = r#"{
      "name": "diagon-alley",
      "owner": { "name": "AAN", "email": "aan@mjolner.com" },
      "metadata": { "description": "d", "version": "1.0.0" },
      "plugins": [
        {
          "name": "design-extractor",
          "description": "d",
          "source": "./skills/design-extractor"
        },
        {
          "name": "openpowers",
          "description": "d",
          "source": { "source": "github", "repo": "fuzzyalej/openpowers" }
        }
      ]
    }"#;

    #[test]
    fn parses_relative_path_and_external_repo_sources() {
        let plugins = parse_marketplace_json(REAL_MARKETPLACE_JSON).unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].name, "design-extractor");
        assert_eq!(plugins[0].source, PluginSource::RelativePath("./skills/design-extractor".into()));
        assert_eq!(plugins[1].name, "openpowers");
        assert_eq!(plugins[1].source, PluginSource::ExternalRepo { repo: "fuzzyalej/openpowers".into() });
    }

    #[test]
    fn find_plugin_locates_by_name() {
        let plugins = parse_marketplace_json(REAL_MARKETPLACE_JSON).unwrap();
        let found = find_plugin(&plugins, "openpowers").unwrap();
        assert_eq!(found.name, "openpowers");
    }

    #[test]
    fn find_plugin_errors_when_missing() {
        let plugins = parse_marketplace_json(REAL_MARKETPLACE_JSON).unwrap();
        assert!(find_plugin(&plugins, "nope").is_err());
    }

    #[test]
    fn parses_bare_url_source() {
        // Real shape seen in the wild, e.g. obra/superpowers-marketplace's `superpowers` entry.
        let json = r#"{"plugins":[{"name":"superpowers","source":{"source":"url","url":"https://github.com/obra/superpowers.git"}}]}"#;
        let plugins = parse_marketplace_json(json).unwrap();
        assert_eq!(
            plugins[0].source,
            PluginSource::ExternalRepo { repo: "https://github.com/obra/superpowers.git".into() }
        );
    }

    #[test]
    fn parses_git_subdir_source_with_ref_and_sha() {
        // Real shape seen in the wild, e.g. 42Crunch-AI/claude-plugins' subdir plugins.
        let json = r#"{"plugins":[{"name":"api-security-testing","source":{
            "source":"git-subdir",
            "url":"https://github.com/42Crunch-AI/claude-plugins.git",
            "path":"plugins/api-security-testing",
            "ref":"v1.5.5",
            "sha":"30287f5e3f122a646d1ac5ca3ab96e130c52a3ad"
        }}]}"#;
        let plugins = parse_marketplace_json(json).unwrap();
        assert_eq!(
            plugins[0].source,
            PluginSource::GitSubdir {
                url: "https://github.com/42Crunch-AI/claude-plugins.git".into(),
                path: "plugins/api-security-testing".into(),
                git_ref: Some("v1.5.5".into()),
                sha: Some("30287f5e3f122a646d1ac5ca3ab96e130c52a3ad".into()),
            }
        );
    }

    #[test]
    fn parses_git_subdir_source_without_ref_or_sha() {
        let json = r#"{"plugins":[{"name":"x","source":{
            "source":"git-subdir","url":"https://github.com/o/r.git","path":"plugins/x"
        }}]}"#;
        let plugins = parse_marketplace_json(json).unwrap();
        assert_eq!(
            plugins[0].source,
            PluginSource::GitSubdir {
                url: "https://github.com/o/r.git".into(),
                path: "plugins/x".into(),
                git_ref: None,
                sha: None,
            }
        );
    }

    #[test]
    fn errors_on_git_subdir_missing_path() {
        let json = r#"{"plugins":[{"name":"x","source":{"source":"git-subdir","url":"https://github.com/o/r.git"}}]}"#;
        assert!(parse_marketplace_json(json).is_err());
    }

    #[test]
    fn errors_on_missing_plugins_array() {
        assert!(parse_marketplace_json(r#"{"name":"x"}"#).is_err());
    }

    #[test]
    fn errors_on_unrecognized_source_shape() {
        let json = r#"{"plugins":[{"name":"x","source":{"source":"gitlab","repo":"o/r"}}]}"#;
        assert!(parse_marketplace_json(json).is_err());
    }

    #[test]
    fn errors_on_plugin_missing_source() {
        let json = r#"{"plugins":[{"name":"x"}]}"#;
        assert!(parse_marketplace_json(json).is_err());
    }
}
