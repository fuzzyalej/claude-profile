use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/bundled_files.rs"));

const STAMP: &str = ".version";

/// Writes the baked-in profiles into `dir`, which the engine owns: it is rewritten
/// whenever the stamped version differs, so edits belong in ~/.claude-profiles instead.
pub fn seed(dir: &Path, version: &str) -> anyhow::Result<()> {
    if std::fs::read_to_string(dir.join(STAMP)).is_ok_and(|v| v == version) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let shipped = BUNDLED_FILES.iter().any(|(n, _)| *n == name);
            if !shipped && (name.ends_with(".json") || name.ends_with(".lock")) {
                std::fs::remove_file(entry.path())?;
            }
        }
    }

    for (name, body) in BUNDLED_FILES {
        std::fs::write(dir.join(name), body)?;
    }
    std::fs::write(dir.join(STAMP), version)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bundled_files_are_baked_in() {
        assert!(!BUNDLED_FILES.is_empty());
    }

    #[test]
    fn seed_writes_every_bundled_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bundled");
        seed(&dir, "1.0.0").unwrap();
        for (name, body) in BUNDLED_FILES {
            assert_eq!(fs::read_to_string(dir.join(name)).unwrap(), *body);
        }
    }

    #[test]
    fn seed_records_the_version() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bundled");
        seed(&dir, "1.0.0").unwrap();
        assert_eq!(fs::read_to_string(dir.join(".version")).unwrap(), "1.0.0");
    }

    #[test]
    fn seed_skips_when_version_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bundled");
        seed(&dir, "1.0.0").unwrap();
        let victim = dir.join(BUNDLED_FILES[0].0);
        fs::write(&victim, "edited").unwrap();

        seed(&dir, "1.0.0").unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "edited");
    }

    #[test]
    fn seed_rewrites_when_version_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bundled");
        seed(&dir, "1.0.0").unwrap();
        let victim = dir.join(BUNDLED_FILES[0].0);
        fs::write(&victim, "edited").unwrap();

        seed(&dir, "1.0.1").unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), BUNDLED_FILES[0].1);
    }

    #[test]
    fn seed_removes_files_no_longer_bundled() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bundled");
        fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("retired-profile.json");
        fs::write(&stale, "{}").unwrap();

        seed(&dir, "1.0.0").unwrap();

        assert!(!stale.exists());
    }
}
