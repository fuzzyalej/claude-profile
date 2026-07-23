use crate::fs_paths::Paths;
use std::path::{Path, PathBuf};

fn template(name: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": name,
        "description": "",
        "marketplaces": {},
        "plugins": [],
        "pluginDirs": [],
        "mcpServers": {}
    })).unwrap() + "\n"
}

pub fn scaffold(name: &str, dest_dir: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let path = dest_dir.join(format!("{name}.json"));
    if path.exists() {
        anyhow::bail!("profile '{name}' already exists at {}", path.display());
    }
    std::fs::write(&path, template(name))?;
    Ok(path)
}

pub fn run(name: &str, paths: &Paths) -> anyhow::Result<()> {
    let path = scaffold(name, &paths.user_profiles_dir())?;
    println!("created {}", path.display());
    println!("edit it, then launch with: claude-profile {name}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolds_valid_parseable_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let path = scaffold("myprof", tmp.path()).unwrap();
        assert!(path.ends_with("myprof.json"));
        let body = std::fs::read_to_string(&path).unwrap();
        let p = crate::profile::Profile::from_json_str(&body).unwrap();
        assert_eq!(p.name, "myprof");
    }

    #[test]
    fn refuses_to_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        scaffold("x", tmp.path()).unwrap();
        assert!(scaffold("x", tmp.path()).is_err());
    }
}
