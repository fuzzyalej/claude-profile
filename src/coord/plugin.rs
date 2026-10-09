use crate::fs_paths::Paths;
use anyhow::Result;
use std::path::PathBuf;

include!(concat!(env!("OUT_DIR"), "/bundled_plugin_files.rs"));

const STAMP: &str = ".version";

/// Writes the baked-in coordinator plugin into the engine-owned bundled plugins directory.
/// The directory is rebuilt whenever its stamped version differs.
pub fn seed_coordinator_plugin(paths: &Paths, version: &str) -> Result<PathBuf> {
    let dir = paths.bundled_plugins_dir().join("coordinator");
    if std::fs::read_to_string(dir.join(STAMP)).is_ok_and(|v| v == version) {
        return Ok(dir);
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    for (rel, body) in BUNDLED_PLUGIN_FILES {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, body)?;
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(STAMP), version)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn seeds_plugin_files() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let dir = seed_coordinator_plugin(&paths, "1.0.0").unwrap();
        assert_eq!(dir, paths.bundled_plugins_dir().join("coordinator"));
        assert!(dir.join(".claude-plugin/plugin.json").is_file());
        assert!(dir.join("skills/coordinating-workers/SKILL.md").is_file());
        assert_eq!(fs::read_to_string(dir.join(".version")).unwrap(), "1.0.0");
    }

    #[test]
    fn reseeds_on_version_change() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let dir = seed_coordinator_plugin(&paths, "1.0.0").unwrap();
        let skill = dir.join("skills/coordinating-workers/SKILL.md");
        let original = fs::read_to_string(&skill).unwrap();
        fs::write(&skill, "edited").unwrap();
        fs::write(dir.join("stale.txt"), "old").unwrap();

        seed_coordinator_plugin(&paths, "1.0.0").unwrap();
        assert_eq!(fs::read_to_string(&skill).unwrap(), "edited");

        seed_coordinator_plugin(&paths, "1.0.1").unwrap();
        assert_eq!(fs::read_to_string(&skill).unwrap(), original);
        assert!(!dir.join("stale.txt").exists());
        assert_eq!(fs::read_to_string(dir.join(".version")).unwrap(), "1.0.1");
    }

    #[test]
    fn skill_mentions_every_tool() {
        let skill = BUNDLED_PLUGIN_FILES
            .iter()
            .find(|(p, _)| *p == "skills/coordinating-workers/SKILL.md")
            .map(|(_, body)| *body)
            .unwrap();
        for tool in ["list_profiles", "spawn", "status", "result", "send", "cancel", "cleanup"] {
            assert!(skill.contains(&format!("`{tool}")), "{tool}");
        }
    }

    #[test]
    fn skill_mentions_clean_profile() {
        let skill = BUNDLED_PLUGIN_FILES
            .iter()
            .find(|(p, _)| *p == "skills/coordinating-workers/SKILL.md")
            .map(|(_, body)| *body)
            .unwrap();
        assert!(skill.contains("[\"clean\"]"));
    }

    #[test]
    fn plugin_manifest_names_the_plugin() {
        let manifest = BUNDLED_PLUGIN_FILES
            .iter()
            .find(|(p, _)| *p == ".claude-plugin/plugin.json")
            .map(|(_, body)| *body)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(manifest).unwrap();
        assert_eq!(v["name"], "coordinator");
    }
}
