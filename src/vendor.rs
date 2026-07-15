use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum PluginSource {
    /// Path relative to the marketplace repo root, e.g. "./skills/foo".
    RelativePath(String),
    /// An externally-hosted plugin, referenced by `owner/repo`.
    ExternalRepo { repo: String },
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
            Value::Object(o) => {
                let kind = o.get("source").and_then(|x| x.as_str());
                let repo = o.get("repo").and_then(|x| x.as_str());
                match (kind, repo) {
                    (Some("github"), Some(r)) => PluginSource::ExternalRepo { repo: r.to_string() },
                    _ => anyhow::bail!("plugin '{name}' has unrecognized source shape: {source}"),
                }
            }
            other => anyhow::bail!("plugin '{name}' has unrecognized source shape: {other}"),
        };
        out.push(MarketplacePlugin { name: name.to_string(), source: parsed });
    }
    Ok(out)
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
