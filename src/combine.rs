use crate::profile::Profile;
use std::collections::BTreeMap;

/// Merge several resolved profiles into one effective profile for a combined launch.
/// Plugins and pluginDirs union (first-seen order, deduped). Marketplaces and mcpServers
/// merge key-by-key; a key defined with *different* values by two profiles is a conflict
/// and aborts. `bare` must be uniform across the set.
pub fn combine_profiles(profiles: &[(String, Profile)]) -> anyhow::Result<Profile> {
    if profiles.is_empty() {
        anyhow::bail!("no profiles to combine");
    }
    let keys: Vec<&str> = profiles.iter().map(|(n, _)| n.as_str()).collect();

    let mut plugins: Vec<String> = Vec::new();
    let mut plugin_dirs: Vec<String> = Vec::new();
    for (_, p) in profiles {
        for id in &p.plugins {
            if !plugins.contains(id) {
                plugins.push(id.clone());
            }
        }
        for d in &p.plugin_dirs {
            if !plugin_dirs.contains(d) {
                plugin_dirs.push(d.clone());
            }
        }
    }

    // marketplaces: merge, remembering which profile first set each key for error messages.
    let mut marketplaces: BTreeMap<String, String> = BTreeMap::new();
    let mut mkt_owner: BTreeMap<String, String> = BTreeMap::new();
    for (owner, p) in profiles {
        for (k, v) in &p.marketplaces {
            match marketplaces.get(k) {
                Some(existing) if existing != v => anyhow::bail!(
                    "marketplace '{k}' defined differently by {} ({existing}) and {owner} ({v}); resolve before combining",
                    mkt_owner[k]
                ),
                Some(_) => {}
                None => {
                    marketplaces.insert(k.clone(), v.clone());
                    mkt_owner.insert(k.clone(), owner.clone());
                }
            }
        }
    }

    // mcpServers: same treatment over the JSON object.
    let mut mcp = serde_json::Map::new();
    let mut mcp_owner: BTreeMap<String, String> = BTreeMap::new();
    for (owner, p) in profiles {
        if let Some(obj) = p.mcp_servers.as_object() {
            for (k, v) in obj {
                match mcp.get(k) {
                    Some(existing) if existing != v => anyhow::bail!(
                        "MCP server '{k}' defined differently by {} and {owner}; resolve before combining",
                        mcp_owner[k]
                    ),
                    Some(_) => {}
                    None => {
                        mcp.insert(k.clone(), v.clone());
                        mcp_owner.insert(k.clone(), owner.clone());
                    }
                }
            }
        }
    }

    let bare = profiles[0].1.bare;
    if profiles.iter().any(|(_, p)| p.bare != bare) {
        anyhow::bail!("profiles disagree on `bare`; combine only profiles that share the same bare setting");
    }

    Ok(Profile {
        name: keys.join("+"),
        description: Some(format!("combined: {}", keys.join(" + "))),
        author: None,
        extends: None,
        marketplaces,
        plugins,
        remove_plugins: Vec::new(),
        plugin_dirs,
        mcp_servers: serde_json::Value::Object(mcp),
        bare,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(name: &str, json: &str) -> (String, Profile) {
        (name.to_string(), Profile::from_json_str(json).unwrap())
    }

    #[test]
    fn unions_and_dedups_plugins_and_dirs() {
        let ps = vec![
            prof("a", r#"{"name":"a","plugins":["x@m","shared@m"],"pluginDirs":["d1"]}"#),
            prof("b", r#"{"name":"b","plugins":["shared@m","y@m"],"pluginDirs":["d1","d2"]}"#),
        ];
        let c = combine_profiles(&ps).unwrap();
        assert_eq!(c.name, "a+b");
        assert_eq!(c.plugins, vec!["x@m", "shared@m", "y@m"]);
        assert_eq!(c.plugin_dirs, vec!["d1", "d2"]);
    }

    #[test]
    fn merges_disjoint_marketplaces_and_mcp() {
        let ps = vec![
            prof("a", r#"{"name":"a","marketplaces":{"m1":"o/a"},"mcpServers":{"s1":{"command":"a"}}}"#),
            prof("b", r#"{"name":"b","marketplaces":{"m2":"o/b"},"mcpServers":{"s2":{"command":"b"}}}"#),
        ];
        let c = combine_profiles(&ps).unwrap();
        assert_eq!(c.marketplaces.get("m1").unwrap(), "o/a");
        assert_eq!(c.marketplaces.get("m2").unwrap(), "o/b");
        assert!(c.mcp_servers.get("s1").is_some());
        assert!(c.mcp_servers.get("s2").is_some());
    }

    #[test]
    fn identical_keys_merge_without_conflict() {
        let ps = vec![
            prof("a", r#"{"name":"a","marketplaces":{"m":"o/same"},"mcpServers":{"s":{"command":"x"}}}"#),
            prof("b", r#"{"name":"b","marketplaces":{"m":"o/same"},"mcpServers":{"s":{"command":"x"}}}"#),
        ];
        let c = combine_profiles(&ps).unwrap();
        assert_eq!(c.marketplaces.get("m").unwrap(), "o/same");
    }

    #[test]
    fn conflicting_marketplace_aborts() {
        let ps = vec![
            prof("a", r#"{"name":"a","marketplaces":{"m":"o/a"}}"#),
            prof("b", r#"{"name":"b","marketplaces":{"m":"o/b"}}"#),
        ];
        let err = combine_profiles(&ps).unwrap_err().to_string();
        assert!(err.contains("marketplace 'm'"));
        assert!(err.contains("a") && err.contains("b"));
    }

    #[test]
    fn conflicting_mcp_server_aborts() {
        let ps = vec![
            prof("a", r#"{"name":"a","mcpServers":{"s":{"command":"a"}}}"#),
            prof("b", r#"{"name":"b","mcpServers":{"s":{"command":"b"}}}"#),
        ];
        let err = combine_profiles(&ps).unwrap_err().to_string();
        assert!(err.contains("MCP server 's'"));
    }

    #[test]
    fn bare_disagreement_aborts() {
        let ps = vec![
            prof("a", r#"{"name":"a","bare":true}"#),
            prof("b", r#"{"name":"b","bare":false}"#),
        ];
        assert!(combine_profiles(&ps).unwrap_err().to_string().contains("bare"));
    }
}
