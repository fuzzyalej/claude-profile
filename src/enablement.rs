use crate::claude::InstalledPlugin;
use crate::profile::Profile;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

pub struct SkillEntry {
    pub name: String,
    pub has_manifest: bool,
}

pub struct Enablement {
    pub enabled_plugins: BTreeMap<String, bool>,
    pub leaking_skills: Vec<String>,
    pub suppressed_mcp: Vec<String>,
}

fn is_nonempty_obj(v: &serde_json::Value) -> bool {
    v.as_object().map(|o| !o.is_empty()).unwrap_or(false)
}

pub fn build(profile: &Profile, installed: &[InstalledPlugin], skills: &[SkillEntry]) -> Enablement {
    let keep: BTreeSet<&str> = profile.plugins.iter().map(|s| s.as_str()).collect();
    let mut enabled_plugins = BTreeMap::new();

    for p in installed {
        enabled_plugins.insert(p.id.clone(), keep.contains(p.id.as_str()));
    }
    // Ensure profile entries are present as true even if not yet listed.
    for id in &profile.plugins {
        enabled_plugins.insert(id.clone(), true);
    }

    let mut leaking_skills = Vec::new();
    for s in skills {
        if s.has_manifest {
            let id = format!("{}@skills-dir", s.name);
            // Only gate (set false) skills the profile does not explicitly keep.
            if !keep.contains(id.as_str()) {
                enabled_plugins.insert(id, false);
            } else {
                enabled_plugins.insert(id, true);
            }
        } else {
            leaking_skills.push(s.name.clone());
        }
    }

    let mut suppressed_mcp = Vec::new();
    for p in installed {
        if keep.contains(p.id.as_str()) && is_nonempty_obj(&p.mcp_servers) {
            suppressed_mcp.push(p.id.clone());
        }
    }

    Enablement { enabled_plugins, leaking_skills, suppressed_mcp }
}

pub fn scan_skills_dir(dir: &Path) -> Vec<SkillEntry> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let has_manifest = entry.path()
                    .join(".claude-plugin").join("plugin.json").is_file();
                out.push(SkillEntry { name, has_manifest });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::InstalledPlugin;

    fn plug(id: &str, mcp: serde_json::Value) -> InstalledPlugin {
        InstalledPlugin { id: id.into(), enabled: true, scope: "user".into(), mcp_servers: mcp }
    }

    #[test]
    fn disables_all_installed_except_profile_entries() {
        let json = r#"{"name":"p","plugins":["keep@m"]}"#;
        let profile = crate::profile::Profile::from_json_str(json).unwrap();
        let installed = vec![
            plug("keep@m", serde_json::json!({})),
            plug("drop@m", serde_json::json!({})),
        ];
        let e = build(&profile, &installed, &[]);
        assert_eq!(e.enabled_plugins.get("keep@m"), Some(&true));
        assert_eq!(e.enabled_plugins.get("drop@m"), Some(&false));
    }

    #[test]
    fn manifest_skill_gated_manifestless_leaks() {
        let profile = crate::profile::Profile::from_json_str(r#"{"name":"p"}"#).unwrap();
        let skills = vec![
            SkillEntry { name: "gated".into(), has_manifest: true },
            SkillEntry { name: "leaky".into(), has_manifest: false },
        ];
        let e = build(&profile, &[], &skills);
        assert_eq!(e.enabled_plugins.get("gated@skills-dir"), Some(&false));
        assert_eq!(e.leaking_skills, vec!["leaky".to_string()]);
    }

    #[test]
    fn flags_bundled_mcp_of_enabled_profile_plugin() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","plugins":["srv@m"]}"#).unwrap();
        let installed = vec![plug("srv@m", serde_json::json!({"x":{"command":"echo"}}))];
        let e = build(&profile, &installed, &[]);
        assert_eq!(e.suppressed_mcp, vec!["srv@m".to_string()]);
    }

    #[test]
    fn kept_manifest_skill_stays_enabled() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","plugins":["mine@skills-dir"]}"#).unwrap();
        let skills = vec![SkillEntry { name: "mine".into(), has_manifest: true }];
        let e = build(&profile, &[], &skills);
        assert_eq!(e.enabled_plugins.get("mine@skills-dir"), Some(&true));
        assert!(!e.leaking_skills.contains(&"mine".to_string()));
    }

    #[test]
    fn non_object_mcp_servers_not_suppressed() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","plugins":["srv@m"]}"#).unwrap();

        let installed_null = vec![plug("srv@m", serde_json::json!(null))];
        let e_null = build(&profile, &installed_null, &[]);
        assert!(e_null.suppressed_mcp.is_empty());

        let installed_array = vec![plug("srv@m", serde_json::json!([1, 2]))];
        let e_array = build(&profile, &installed_array, &[]);
        assert!(e_array.suppressed_mcp.is_empty());

        let installed_empty_obj = vec![plug("srv@m", serde_json::json!({}))];
        let e_empty_obj = build(&profile, &installed_empty_obj, &[]);
        assert!(e_empty_obj.suppressed_mcp.is_empty());
    }

    #[test]
    fn plugin_id_colliding_with_skill_id_is_consistent() {
        let profile = crate::profile::Profile::from_json_str(
            r#"{"name":"p","plugins":["x@skills-dir"]}"#).unwrap();
        let skills = vec![SkillEntry { name: "x".into(), has_manifest: true }];
        let installed = vec![plug("x@skills-dir", serde_json::json!({}))];
        let e = build(&profile, &installed, &skills);
        assert_eq!(e.enabled_plugins.get("x@skills-dir"), Some(&true));
    }
}
