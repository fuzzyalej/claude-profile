use crate::fs_paths::Paths;
use crate::profile::Profile;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileSource {
    EnvDir,
    ProjectDir,
    UserDir,
    Pack(String),
    BundledDir,
}

pub struct ResolvedProfile {
    pub profile: Profile,
    // Phase 2/3: diagnostics/`explain` commands will report the source file path.
    #[allow(dead_code)]
    pub path: PathBuf,
    // Phase 2/3: diagnostics/`explain` commands will report where a profile resolved from.
    #[allow(dead_code)]
    pub source: ProfileSource,
}

fn load(path: &Path, source: ProfileSource) -> anyhow::Result<ResolvedProfile> {
    let body = std::fs::read_to_string(path)?;
    let profile = Profile::from_json_str(&body)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(ResolvedProfile { profile, path: path.to_path_buf(), source })
}

/// Ordered candidate (file-path, source) pairs for a profile name.
fn candidates(name: &str, paths: &Paths, cwd: &Path, env_dir: Option<&Path>, bundled_dir: &Path)
    -> Vec<(PathBuf, ProfileSource)> {
    let file = format!("{name}.json");
    let mut out = Vec::new();
    if let Some(e) = env_dir {
        out.push((e.join(&file), ProfileSource::EnvDir));
    }
    out.push((cwd.join("profiles").join(&file), ProfileSource::ProjectDir));
    out.push((cwd.join(".claude-profiles").join(&file), ProfileSource::ProjectDir));
    out.push((paths.user_profiles_dir().join(&file), ProfileSource::UserDir));
    // packs/*/profiles/<file>
    let packs = paths.user_profiles_dir().join("packs");
    if let Ok(entries) = std::fs::read_dir(&packs) {
        for entry in entries.flatten() {
            let pack = entry.file_name().to_string_lossy().to_string();
            out.push((entry.path().join("profiles").join(&file), ProfileSource::Pack(pack)));
        }
    }
    out.push((bundled_dir.join(&file), ProfileSource::BundledDir));
    out
}

/// Loads a profile by name. The caller (launch_profile) applies `extends::resolve_extends` afterward.
pub fn resolve(name: &str, paths: &Paths, cwd: &Path, env_dir: Option<&Path>, bundled_dir: &Path)
    -> anyhow::Result<ResolvedProfile> {
    for (path, source) in candidates(name, paths, cwd, env_dir, bundled_dir) {
        if path.is_file() {
            return load(&path, source);
        }
    }
    anyhow::bail!("profile '{name}' not found in any search path")
}

pub fn list_available(paths: &Paths, cwd: &Path, env_dir: Option<&Path>, bundled_dir: &Path)
    -> Vec<(String, PathBuf, ProfileSource)> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut dirs: Vec<(PathBuf, ProfileSource)> = Vec::new();
    if let Some(e) = env_dir { dirs.push((e.to_path_buf(), ProfileSource::EnvDir)); }
    dirs.push((cwd.join("profiles"), ProfileSource::ProjectDir));
    dirs.push((cwd.join(".claude-profiles"), ProfileSource::ProjectDir));
    dirs.push((paths.user_profiles_dir(), ProfileSource::UserDir));
    let packs = paths.user_profiles_dir().join("packs");
    if let Ok(entries) = std::fs::read_dir(&packs) {
        for entry in entries.flatten() {
            let pack = entry.file_name().to_string_lossy().to_string();
            dirs.push((entry.path().join("profiles"), ProfileSource::Pack(pack)));
        }
    }
    dirs.push((bundled_dir.to_path_buf(), ProfileSource::BundledDir));

    for (dir, source) in dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    let name = path.file_stem().unwrap().to_string_lossy().to_string();
                    if seen.insert(name.clone()) {
                        out.push((name, path, source.clone()));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &std::path::Path, name: &str, body: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn user_dir_wins_over_bundled_and_reports_source() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundled = tmp.path().join("bundled");
        let cwd = tmp.path().join("cwd");
        write(&home.join(".claude-profiles"), "foo.json", r#"{"name":"foo","plugins":["u@m"]}"#);
        write(&bundled, "foo.json", r#"{"name":"foo","plugins":["e@m"]}"#);

        let paths = crate::fs_paths::Paths::from_home(home.clone());
        let r = resolve("foo", &paths, &cwd, None, &bundled).unwrap();
        assert_eq!(r.profile.plugins, vec!["u@m"]);
        assert!(matches!(r.source, ProfileSource::UserDir));
    }

    #[test]
    fn falls_back_to_bundled() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundled = tmp.path().join("bundled");
        let cwd = tmp.path().join("cwd");
        write(&bundled, "bar.json", r#"{"name":"bar","plugins":["e@m"]}"#);
        let paths = crate::fs_paths::Paths::from_home(home);
        let r = resolve("bar", &paths, &cwd, None, &bundled).unwrap();
        assert!(matches!(r.source, ProfileSource::BundledDir));
    }

    #[test]
    fn missing_profile_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::fs_paths::Paths::from_home(tmp.path().join("home"));
        let err = resolve("nope", &paths, tmp.path(), None, &tmp.path().join("bundled"));
        assert!(err.is_err());
    }

    #[test]
    fn env_dir_wins_over_user_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundled = tmp.path().join("bundled");
        let cwd = tmp.path().join("cwd");
        let env_dir = tmp.path().join("envdir");
        write(&env_dir, "foo.json", r#"{"name":"foo","plugins":["env@m"]}"#);
        write(&home.join(".claude-profiles"), "foo.json", r#"{"name":"foo","plugins":["u@m"]}"#);

        let paths = crate::fs_paths::Paths::from_home(home.clone());
        let r = resolve("foo", &paths, &cwd, Some(&env_dir), &bundled).unwrap();
        assert_eq!(r.profile.plugins, vec!["env@m"]);
        assert!(matches!(r.source, ProfileSource::EnvDir));
    }

    #[test]
    fn resolves_from_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundled = tmp.path().join("bundled");
        let cwd = tmp.path().join("cwd");
        write(
            &home.join(".claude-profiles").join("packs").join("owner--repo").join("profiles"),
            "foo.json",
            r#"{"name":"foo","plugins":["pack@m"]}"#,
        );

        let paths = crate::fs_paths::Paths::from_home(home.clone());
        let r = resolve("foo", &paths, &cwd, None, &bundled).unwrap();
        assert_eq!(r.profile.plugins, vec!["pack@m"]);
        assert!(matches!(r.source, ProfileSource::Pack(ref p) if p == "owner--repo"));
    }

    #[test]
    fn list_available_dedups_keeping_highest_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundled = tmp.path().join("bundled");
        let cwd = tmp.path().join("cwd");
        write(&home.join(".claude-profiles"), "foo.json", r#"{"name":"foo","plugins":["u@m"]}"#);
        write(&bundled, "foo.json", r#"{"name":"foo","plugins":["e@m"]}"#);
        write(&bundled, "bar.json", r#"{"name":"bar","plugins":["e@m"]}"#);

        let paths = crate::fs_paths::Paths::from_home(home.clone());
        let list = list_available(&paths, &cwd, None, &bundled);

        let foo_entries: Vec<_> = list.iter().filter(|(name, _, _)| name == "foo").collect();
        assert_eq!(foo_entries.len(), 1);
        assert!(matches!(foo_entries[0].2, ProfileSource::UserDir));

        let bar_entries: Vec<_> = list.iter().filter(|(name, _, _)| name == "bar").collect();
        assert_eq!(bar_entries.len(), 1);
    }
}
