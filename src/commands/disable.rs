use crate::fs_paths::Paths;
use crate::profile::Profile;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Plugins referenced by `target` that no *other* profile in `profiles` references.
/// These are safe to disable globally: no other profile depends on them, and launching
/// `target` itself re-enables them for that session.
pub fn unshared_plugins(target: &str, profiles: &[(String, Profile)]) -> anyhow::Result<Vec<String>> {
    let target_profile = profiles
        .iter()
        .find(|(name, _)| name == target)
        .ok_or_else(|| anyhow::anyhow!("profile '{target}' not found"))?;

    let mut shared: BTreeSet<&str> = BTreeSet::new();
    for (name, profile) in profiles {
        if name == target {
            continue;
        }
        for id in &profile.plugins {
            shared.insert(id.as_str());
        }
    }

    let mut out: Vec<String> = target_profile
        .1
        .plugins
        .iter()
        .filter(|id| !shared.contains(id.as_str()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Set `enabledPlugins[id] = false` for each id in the global settings.json, preserving
/// every other setting. Creates the file / the `enabledPlugins` map if absent.
pub fn disable_in_settings(settings_path: &Path, ids: &[String]) -> anyhow::Result<()> {
    let mut root: serde_json::Value = if settings_path.exists() {
        serde_json::from_str(&fs::read_to_string(settings_path)?)?
    } else {
        serde_json::json!({})
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} is not a JSON object", settings_path.display()))?;
    let entry = obj
        .entry("enabledPlugins")
        .or_insert_with(|| serde_json::json!({}));
    let map = entry
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("enabledPlugins in settings.json is not an object"))?;
    for id in ids {
        map.insert(id.clone(), serde_json::Value::Bool(false));
    }
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(settings_path, format!("{}\n", serde_json::to_string_pretty(&root)?))?;
    Ok(())
}

pub fn run(
    paths: &Paths,
    profiles: &[(String, Profile)],
    target: &str,
    dry_run: bool,
) -> anyhow::Result<()> {
    let ids = unshared_plugins(target, profiles)?;
    if ids.is_empty() {
        println!("no unshared plugins to disable for '{target}' (all its plugins are used by other profiles)");
        return Ok(());
    }
    let path = paths.claude_settings_path();
    if dry_run {
        println!("would disable {} unshared plugin(s) for '{target}':", ids.len());
    } else {
        disable_in_settings(&path, &ids)?;
        println!("disabled {} unshared plugin(s) for '{target}' in {}:", ids.len(), path.display());
    }
    for id in &ids {
        println!("  {id}");
    }
    if !dry_run {
        println!("they stay installed; launch `claude-profile {target}` to use them again.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(name: &str, plugins: &[&str]) -> (String, Profile) {
        let list = plugins.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(",");
        (name.to_string(), Profile::from_json_str(&format!(r#"{{"name":"{name}","plugins":[{list}]}}"#)).unwrap())
    }

    #[test]
    fn unshared_excludes_plugins_used_by_other_profiles() {
        let profiles = vec![
            prof("a", &["x@m", "y@m"]),
            prof("b", &["y@m", "z@m"]),
        ];
        // y@m is shared with b; x@m is only in a.
        assert_eq!(unshared_plugins("a", &profiles).unwrap(), vec!["x@m".to_string()]);
    }

    #[test]
    fn unshared_errors_for_unknown_profile() {
        let profiles = vec![prof("a", &["x@m"])];
        assert!(unshared_plugins("nope", &profiles).is_err());
    }

    #[test]
    fn disable_preserves_other_settings_and_sets_false() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".claude").join("settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"model":"opus","enabledPlugins":{"x@m":true,"keep@m":true}}"#).unwrap();

        disable_in_settings(&path, &["x@m".to_string(), "new@m".to_string()]).unwrap();

        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["model"], serde_json::json!("opus")); // unrelated setting preserved
        assert_eq!(v["enabledPlugins"]["x@m"], serde_json::json!(false));
        assert_eq!(v["enabledPlugins"]["keep@m"], serde_json::json!(true)); // untouched
        assert_eq!(v["enabledPlugins"]["new@m"], serde_json::json!(false)); // added
    }

    #[test]
    fn disable_creates_settings_and_enabled_map_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".claude").join("settings.json");
        disable_in_settings(&path, &["x@m".to_string()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["enabledPlugins"]["x@m"], serde_json::json!(false));
    }
}
