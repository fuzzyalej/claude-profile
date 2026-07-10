use serde::Deserialize;
use std::collections::BTreeMap;

fn default_mcp() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    // Phase 2: name-vs-filename validation and owner/repo default-profile resolution.
    #[allow(dead_code)]
    pub name: String,
    // Phase 2/3: profile inheritance / documentation UI will surface this.
    #[allow(dead_code)]
    #[serde(default)]
    pub description: Option<String>,
    // Phase 2/3: profile inheritance (`extends`) resolution.
    #[allow(dead_code)]
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub marketplaces: BTreeMap<String, String>,
    #[serde(default)]
    pub plugins: Vec<String>,
    // Phase 2/3: plugin removal when composing/extending profiles.
    #[allow(dead_code)]
    #[serde(default)]
    pub remove_plugins: Vec<String>,
    #[serde(default)]
    pub plugin_dirs: Vec<String>,
    #[serde(default = "default_mcp")]
    pub mcp_servers: serde_json::Value,
    #[serde(default)]
    pub bare: bool,
}

impl Profile {
    pub fn from_json_str(s: &str) -> anyhow::Result<Profile> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_profile() {
        let json = r#"{ "name": "rust-developer", "plugins": ["superpowers@superpowers-marketplace"] }"#;
        let p = Profile::from_json_str(json).unwrap();
        assert_eq!(p.name, "rust-developer");
        assert_eq!(p.plugins, vec!["superpowers@superpowers-marketplace"]);
        assert!(p.marketplaces.is_empty());
        assert!(!p.bare);
        assert_eq!(p.mcp_servers, serde_json::json!({}));
    }

    #[test]
    fn parses_full_profile_with_marketplaces_and_bare() {
        let json = r#"{
            "name": "x", "description": "d",
            "marketplaces": { "m": "owner/repo#v1" },
            "plugins": ["a@m"], "removePlugins": ["b@m"],
            "pluginDirs": ["vendor/x"], "mcpServers": { "s": { "command": "echo" } },
            "bare": true
        }"#;
        let p = Profile::from_json_str(json).unwrap();
        assert_eq!(p.marketplaces.get("m").unwrap(), "owner/repo#v1");
        assert_eq!(p.remove_plugins, vec!["b@m"]);
        assert_eq!(p.plugin_dirs, vec!["vendor/x"]);
        assert!(p.bare);
        assert!(p.mcp_servers.get("s").is_some());
    }
}
