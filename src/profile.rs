use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn default_mcp() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    // Phase 2: name-vs-filename validation and owner/repo default-profile resolution.
    #[allow(dead_code)]
    pub name: String,
    // Phase 2/3: profile inheritance / documentation UI will surface this.
    #[allow(dead_code)]
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional profile author, shown by `show`. Falls back to the source repo owner.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    // Phase 2/3: profile inheritance (`extends`) resolution.
    #[allow(dead_code)]
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub marketplaces: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<String>,
    // Phase 2/3: plugin removal when composing/extending profiles.
    #[allow(dead_code)]
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove_plugins: Vec<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plugin_dirs: Vec<String>,
    #[serde(default = "default_mcp")]
    #[serde(skip_serializing_if = "is_empty_object")]
    pub mcp_servers: serde_json::Value,
    #[serde(default)]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bare: bool,
}

fn is_empty_object(v: &serde_json::Value) -> bool {
    v.as_object().is_some_and(|o| o.is_empty())
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
    fn serializes_without_empty_fields() {
        let p = Profile::from_json_str(r#"{"name":"x","mcpServers":{"s":{"command":"echo"}}}"#).unwrap();
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, serde_json::json!({"name":"x","mcpServers":{"s":{"command":"echo"}}}));
    }

    #[test]
    fn serialized_profile_round_trips() {
        let json = r#"{"name":"x","description":"d","marketplaces":{"m":"o/r"},"plugins":["a@m"],"pluginDirs":["/p"],"bare":true}"#;
        let p = Profile::from_json_str(json).unwrap();
        let back = Profile::from_json_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.plugins, p.plugins);
        assert_eq!(back.plugin_dirs, p.plugin_dirs);
        assert!(back.bare);
    }

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
            "name": "x", "description": "d", "author": "alice",
            "marketplaces": { "m": "owner/repo#v1" },
            "plugins": ["a@m"], "removePlugins": ["b@m"],
            "pluginDirs": ["vendor/x"], "mcpServers": { "s": { "command": "echo" } },
            "bare": true
        }"#;
        let p = Profile::from_json_str(json).unwrap();
        assert_eq!(p.author.as_deref(), Some("alice"));
        assert_eq!(p.marketplaces.get("m").unwrap(), "owner/repo#v1");
        assert_eq!(p.remove_plugins, vec!["b@m"]);
        assert_eq!(p.plugin_dirs, vec!["vendor/x"]);
        assert!(p.bare);
        assert!(p.mcp_servers.get("s").is_some());
    }
}
