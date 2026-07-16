use crate::fs_paths::Paths;
use std::path::{Path, PathBuf};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

pub fn settings_path(scope: Scope, paths: &Paths, cwd: &Path) -> PathBuf {
    match scope {
        Scope::Global => paths.claude_settings_path(),
        Scope::Project => cwd.join(".claude").join("settings.json"),
    }
}

pub fn backup_path(scope: Scope, paths: &Paths, cwd: &Path) -> PathBuf {
    let dir = paths.statusline_backups_dir();
    match scope {
        Scope::Global => dir.join("global.json"),
        Scope::Project => {
            let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
            let mut hasher = DefaultHasher::new();
            canon.hash(&mut hasher);
            dir.join(format!("{:016x}.json", hasher.finish()))
        }
    }
}

pub fn read_json_object(path: &Path) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("invalid JSON in {}: {e}", path.display()))?;
    match value {
        serde_json::Value::Object(map) => Ok(map),
        _ => anyhow::bail!("expected a JSON object in {}", path.display()),
    }
}

pub fn write_json_object(
    path: &Path,
    map: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))?;
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn global_settings_path_is_claude_settings_json() {
        let paths = Paths::from_home(PathBuf::from("/h"));
        let cwd = PathBuf::from("/work/proj");
        assert_eq!(settings_path(Scope::Global, &paths, &cwd), PathBuf::from("/h/.claude/settings.json"));
    }

    #[test]
    fn project_settings_path_is_relative_to_cwd() {
        let paths = Paths::from_home(PathBuf::from("/h"));
        let cwd = PathBuf::from("/work/proj");
        assert_eq!(settings_path(Scope::Project, &paths, &cwd), PathBuf::from("/work/proj/.claude/settings.json"));
    }

    #[test]
    fn global_backup_path_is_fixed() {
        let paths = Paths::from_home(PathBuf::from("/h"));
        let cwd = PathBuf::from("/work/proj");
        assert_eq!(
            backup_path(Scope::Global, &paths, &cwd),
            PathBuf::from("/h/.claude-profiles/statusline-backups/global.json")
        );
    }

    #[test]
    fn project_backup_path_is_stable_and_distinct_per_cwd() {
        let paths = Paths::from_home(PathBuf::from("/h"));
        let a = backup_path(Scope::Project, &paths, &PathBuf::from("/work/proj-a"));
        let b = backup_path(Scope::Project, &paths, &PathBuf::from("/work/proj-b"));
        let a_again = backup_path(Scope::Project, &paths, &PathBuf::from("/work/proj-a"));
        assert_ne!(a, b);
        assert_eq!(a, a_again);
        assert!(a.starts_with("/h/.claude-profiles/statusline-backups/"));
    }

    #[test]
    fn read_json_object_defaults_to_empty_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let map = read_json_object(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn read_json_object_parses_existing_object() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, r#"{"foo": "bar"}"#).unwrap();
        let map = read_json_object(&path).unwrap();
        assert_eq!(map.get("foo").unwrap(), "bar");
    }

    #[test]
    fn read_json_object_errors_on_invalid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, "not json").unwrap();
        assert!(read_json_object(&path).is_err());
    }

    #[test]
    fn read_json_object_errors_on_non_object_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, "[1, 2]").unwrap();
        assert!(read_json_object(&path).is_err());
    }

    #[test]
    fn write_json_object_creates_parent_dirs_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/settings.json");
        let mut map = serde_json::Map::new();
        map.insert("foo".to_string(), serde_json::json!("bar"));
        write_json_object(&path, &map).unwrap();
        let read_back = read_json_object(&path).unwrap();
        assert_eq!(read_back, map);
    }
}
