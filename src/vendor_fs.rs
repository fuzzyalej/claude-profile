use std::fs;
use std::path::Path;

/// Copy `src` into `dest` recursively. `dest` must not already exist. Copies
/// into a temp sibling directory first and renames into place, so a failed
/// or interrupted copy never leaves a partially-populated `dest`.
pub fn copy_dir_atomic(src: &Path, dest: &Path) -> anyhow::Result<()> {
    if dest.exists() {
        anyhow::bail!("vendor target already exists: {}", dest.display());
    }
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("vendor target has no parent directory: {}", dest.display()))?;
    fs::create_dir_all(parent)?;

    let file_name = dest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("vendor target has no file name: {}", dest.display()))?;
    let tmp = parent.join(format!(".tmp-{}", file_name.to_string_lossy()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    copy_dir_recursive(src, &tmp)?;
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Ensure `dir` is a loadable plugin unit for `claude --plugin-dir`. If it
/// has no `.claude-plugin/plugin.json`, generate a minimal one. Never
/// overwrites an existing manifest.
pub fn ensure_manifest(dir: &Path, skill_name: &str) -> anyhow::Result<()> {
    let manifest_dir = dir.join(".claude-plugin");
    let manifest_path = manifest_dir.join("plugin.json");
    if manifest_path.exists() {
        return Ok(());
    }
    fs::create_dir_all(&manifest_dir)?;
    let manifest = serde_json::json!({ "name": skill_name });
    fs::write(&manifest_path, format!("{}\n", serde_json::to_string_pretty(&manifest)?))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn copies_nested_directory_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("top.txt"), "top").unwrap();
        fs::write(src.join("nested").join("deep.txt"), "deep").unwrap();

        let dest = tmp.path().join("dest");
        copy_dir_atomic(&src, &dest).unwrap();

        assert_eq!(fs::read_to_string(dest.join("top.txt")).unwrap(), "top");
        assert_eq!(fs::read_to_string(dest.join("nested").join("deep.txt")).unwrap(), "deep");
    }

    #[test]
    fn errors_if_dest_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        assert!(copy_dir_atomic(&src, &dest).is_err());
    }

    #[test]
    fn leaves_no_partial_dest_on_temp_collision_cleanup() {
        // A stale .tmp-dest from a previous crashed run must not block a fresh copy.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(tmp.path().join(".tmp-dest")).unwrap();
        copy_dir_atomic(&src, &dest).unwrap();
        assert!(dest.join("a.txt").exists());
    }

    #[test]
    fn ensure_manifest_generates_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        fs::create_dir_all(&dir).unwrap();
        ensure_manifest(&dir, "my-skill").unwrap();
        let manifest = dir.join(".claude-plugin").join("plugin.json");
        assert!(manifest.is_file());
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(manifest).unwrap()).unwrap();
        assert_eq!(v["name"], serde_json::json!("my-skill"));
    }

    #[test]
    fn ensure_manifest_is_noop_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        let manifest_dir = dir.join(".claude-plugin");
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(manifest_dir.join("plugin.json"), r#"{"name":"already-here"}"#).unwrap();
        ensure_manifest(&dir, "my-skill").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(manifest_dir.join("plugin.json")).unwrap()).unwrap();
        assert_eq!(v["name"], serde_json::json!("already-here")); // untouched
    }
}
