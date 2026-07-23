use crate::fs_paths::Paths;
use crate::resolve::{list_available, ProfileSource};
use std::path::{Path, PathBuf};

fn source_label(s: &ProfileSource) -> String {
    match s {
        ProfileSource::EnvDir => "env".to_string(),
        ProfileSource::ProjectDir => "project".to_string(),
        ProfileSource::UserDir => "user".to_string(),
        ProfileSource::Pack(p) => format!("pack:{p}"),
        ProfileSource::BundledDir => "bundled".to_string(),
    }
}

pub fn format_list(items: &[(String, PathBuf, ProfileSource)]) -> String {
    if items.is_empty() {
        return "No profiles found.".to_string();
    }
    let mut out = String::new();
    for (name, _path, source) in items {
        out.push_str(&format!("{name}  [{}]\n", source_label(source)));
    }
    out
}

pub fn run(paths: &Paths, cwd: &Path, env_dir: Option<&Path>, bundled_dir: &Path) -> anyhow::Result<()> {
    let items = list_available(paths, cwd, env_dir, bundled_dir);
    print!("{}", format_list(&items));
    Ok(())
}

/// Prints just the profile names, one per line — consumed by shell completion scripts
/// (see `commands::completions`) rather than by humans, so no source labels.
pub fn run_names(paths: &Paths, cwd: &Path, env_dir: Option<&Path>, bundled_dir: &Path) -> anyhow::Result<()> {
    for (name, _path, _source) in list_available(paths, cwd, env_dir, bundled_dir) {
        println!("{name}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::ProfileSource;
    use std::path::PathBuf;

    #[test]
    fn formats_name_and_source() {
        let items = vec![
            ("rust-developer".to_string(), PathBuf::from("/h/.claude-profiles/rust-developer.json"), ProfileSource::UserDir),
            ("demo".to_string(), PathBuf::from("/e/demo.json"), ProfileSource::BundledDir),
        ];
        let s = format_list(&items);
        assert!(s.contains("rust-developer"));
        assert!(s.contains("user"));
        assert!(s.contains("demo"));
        assert!(s.contains("bundled"));
    }

    #[test]
    fn empty_says_none() {
        assert!(format_list(&[]).to_lowercase().contains("no profiles"));
    }
}
