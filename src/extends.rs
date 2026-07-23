use crate::profile::Profile;

pub fn resolve_extends(
    profile: Profile,
    load_parent: &dyn Fn(&str) -> anyhow::Result<Profile>,
) -> anyhow::Result<Profile> {
    let parent_name = match &profile.extends {
        None => return Ok(profile),
        Some(n) => n.clone(),
    };
    if parent_name == profile.name {
        anyhow::bail!("profile '{}' extends itself", profile.name);
    }
    let parent = load_parent(&parent_name)?;
    if parent.extends.is_some() {
        anyhow::bail!("extends depth > 1 not supported (parent '{parent_name}' also extends)");
    }

    // plugins: parent first, then new child entries, deduped; then subtract removePlugins.
    let mut plugins = parent.plugins.clone();
    for id in &profile.plugins {
        if !plugins.contains(id) {
            plugins.push(id.clone());
        }
    }
    plugins.retain(|id| !profile.remove_plugins.contains(id));

    // marketplaces: parent then child (child wins).
    let mut marketplaces = parent.marketplaces.clone();
    for (k, v) in &profile.marketplaces {
        marketplaces.insert(k.clone(), v.clone());
    }

    // mcp_servers: merge objects, child wins.
    let mut mcp = parent.mcp_servers.clone();
    if let (Some(base), Some(over)) = (mcp.as_object_mut(), profile.mcp_servers.as_object()) {
        for (k, v) in over {
            base.insert(k.clone(), v.clone());
        }
    } else if !profile.mcp_servers.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        mcp = profile.mcp_servers.clone();
    }

    // plugin_dirs: union, parent first.
    let mut plugin_dirs = parent.plugin_dirs.clone();
    for d in &profile.plugin_dirs {
        if !plugin_dirs.contains(d) {
            plugin_dirs.push(d.clone());
        }
    }

    Ok(Profile {
        name: profile.name,
        description: profile.description.or(parent.description),
        author: profile.author.or(parent.author),
        extends: None,
        marketplaces,
        plugins,
        remove_plugins: Vec::new(),
        plugin_dirs,
        mcp_servers: mcp,
        bare: profile.bare || parent.bare,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;

    fn p(json: &str) -> Profile { Profile::from_json_str(json).unwrap() }

    #[test]
    fn no_extends_returns_unchanged() {
        let child = p(r#"{"name":"c","plugins":["a@m"]}"#);
        let out = resolve_extends(child, &|_| panic!("should not load")).unwrap();
        assert_eq!(out.plugins, vec!["a@m"]);
    }

    #[test]
    fn merges_plugins_marketplaces_and_applies_removes() {
        let child = p(r#"{"name":"c","extends":"base","plugins":["c1@m","shared@m"],
            "removePlugins":["drop@m"],"marketplaces":{"m":"o/child"}}"#);
        let parent = p(r#"{"name":"base","plugins":["p1@m","drop@m","shared@m"],
            "marketplaces":{"m":"o/parent","extra":"o/e"}}"#);
        let out = resolve_extends(child, &|n| { assert_eq!(n, "base"); Ok(p(
            r#"{"name":"base","plugins":["p1@m","drop@m","shared@m"],
               "marketplaces":{"m":"o/parent","extra":"o/e"}}"#)) }).unwrap();
        // union, parent-first, deduped, drop removed:
        assert_eq!(out.plugins, vec!["p1@m","shared@m","c1@m"]);
        // child marketplace wins on 'm', parent 'extra' kept:
        assert_eq!(out.marketplaces.get("m").unwrap(), "o/child");
        assert_eq!(out.marketplaces.get("extra").unwrap(), "o/e");
        let _ = parent;
    }

    #[test]
    fn rejects_depth_over_one() {
        let child = p(r#"{"name":"c","extends":"mid"}"#);
        let err = resolve_extends(child, &|_| Ok(p(r#"{"name":"mid","extends":"base"}"#)));
        assert!(err.is_err());
    }

    #[test]
    fn rejects_self_reference() {
        let child = p(r#"{"name":"c","extends":"c"}"#);
        assert!(resolve_extends(child, &|_| panic!("should not load")).is_err());
    }

    #[test]
    fn merges_mcp_dirs_bare_description() {
        let child = p(r#"{"name":"c","extends":"base","mcpServers":{"shared":{"command":"child"},"b":{"command":"cb"}},"pluginDirs":["vendor/c","vendor/p"]}"#);
        let parent = p(r#"{"name":"base","mcpServers":{"a":{"command":"pa"},"shared":{"command":"parent"}},"pluginDirs":["vendor/p"],"bare":true,"description":"parent desc"}"#);
        let out = resolve_extends(child, &|n| { assert_eq!(n, "base"); Ok(p(
            r#"{"name":"base","mcpServers":{"a":{"command":"pa"},"shared":{"command":"parent"}},"pluginDirs":["vendor/p"],"bare":true,"description":"parent desc"}"#)) }).unwrap();

        // mcp_servers: merge with child winning
        assert!(out.mcp_servers.get("a").is_some()); // from parent
        assert!(out.mcp_servers.get("b").is_some()); // from child
        assert_eq!(out.mcp_servers.get("shared").unwrap()["command"], "child"); // child wins

        // plugin_dirs: union, parent-first, deduped
        assert_eq!(out.plugin_dirs, vec!["vendor/p", "vendor/c"]);

        // bare: OR logic
        assert!(out.bare);

        // description: parent fallback
        assert_eq!(out.description, Some("parent desc".to_string()));
        let _ = parent;
    }
}
