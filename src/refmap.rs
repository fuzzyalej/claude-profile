use crate::claude::{InstalledPlugin, Marketplace};
use crate::profile::Profile;
use std::collections::BTreeMap;

pub struct RefMap {
    pub plugin_refs: BTreeMap<String, Vec<String>>,
    pub marketplace_refs: BTreeMap<String, Vec<String>>,
}

pub fn build_refmap(profiles: &[(String, Profile)]) -> RefMap {
    let mut plugin_refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut marketplace_refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, profile) in profiles {
        for id in &profile.plugins {
            plugin_refs.entry(id.clone()).or_default().push(name.clone());
        }
        for mkt in profile.marketplaces.keys() {
            marketplace_refs.entry(mkt.clone()).or_default().push(name.clone());
        }
    }
    RefMap { plugin_refs, marketplace_refs }
}

pub fn unreferenced_plugins(installed: &[InstalledPlugin], refmap: &RefMap) -> Vec<String> {
    installed.iter()
        .filter(|p| !p.id.ends_with("@skills-dir"))
        .filter(|p| !refmap.plugin_refs.contains_key(&p.id))
        .map(|p| p.id.clone())
        .collect()
}

pub fn unreferenced_marketplaces(installed: &[Marketplace], refmap: &RefMap) -> Vec<String> {
    installed.iter()
        .filter(|m| !refmap.marketplace_refs.contains_key(&m.name))
        .map(|m| m.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{InstalledPlugin, Marketplace};

    fn prof(json: &str) -> crate::profile::Profile {
        crate::profile::Profile::from_json_str(json).unwrap()
    }
    fn plug(id: &str) -> InstalledPlugin {
        InstalledPlugin { id: id.into(), enabled: false, scope: "user".into(), mcp_servers: serde_json::json!({}) }
    }

    #[test]
    fn maps_plugins_and_marketplaces_to_profiles() {
        let profiles = vec![
            ("a".to_string(), prof(r#"{"name":"a","marketplaces":{"m":"o/r"},"plugins":["x@m"]}"#)),
            ("b".to_string(), prof(r#"{"name":"b","plugins":["x@m","y@m"]}"#)),
        ];
        let rm = build_refmap(&profiles);
        assert_eq!(rm.plugin_refs.get("x@m").unwrap(), &vec!["a".to_string(), "b".to_string()]);
        assert_eq!(rm.plugin_refs.get("y@m").unwrap(), &vec!["b".to_string()]);
        assert_eq!(rm.marketplace_refs.get("m").unwrap(), &vec!["a".to_string()]);
    }

    #[test]
    fn unreferenced_excludes_referenced_and_skills_dir() {
        let profiles = vec![("a".to_string(), prof(r#"{"name":"a","plugins":["keep@m"]}"#))];
        let rm = build_refmap(&profiles);
        let installed = vec![plug("keep@m"), plug("orphan@m"), plug("loose@skills-dir")];
        let un = unreferenced_plugins(&installed, &rm);
        assert_eq!(un, vec!["orphan@m".to_string()]); // keep referenced; skills-dir excluded
    }

    #[test]
    fn unreferenced_marketplaces_computed() {
        let profiles = vec![("a".to_string(), prof(r#"{"name":"a","marketplaces":{"used":"o/r"}}"#))];
        let rm = build_refmap(&profiles);
        let installed = vec![Marketplace { name: "used".into() }, Marketplace { name: "stale".into() }];
        assert_eq!(unreferenced_marketplaces(&installed, &rm), vec!["stale".to_string()]);
    }
}
