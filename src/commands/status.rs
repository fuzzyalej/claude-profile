use crate::fs_paths::Paths;
use crate::profile::Profile;

pub fn format_status(paths: &Paths, profiles: &[(String, Profile)]) -> anyhow::Result<String> {
    let mut out = String::from("Vendored profiles:\n");
    for (name, _profile) in profiles {
        let vendor_dir = paths.profile_vendor_dir(name);
        if !vendor_dir.is_dir() {
            out.push_str(&format!("  {name}  (not yet provisioned)\n"));
            continue;
        }
        let mut entries: Vec<String> = std::fs::read_dir(&vendor_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        out.push_str(&format!("  {name}  ({} vendored)\n", entries.len()));
        for id in entries {
            out.push_str(&format!("    - {id}\n"));
        }
    }
    Ok(out)
}

pub fn run(paths: &Paths, profiles: &[(String, Profile)]) -> anyhow::Result<()> {
    print!("{}", format_status(paths, profiles)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_vendored_entries_per_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        fs::create_dir_all(paths.profile_vendor_dir("p").join("foo@m")).unwrap();
        fs::create_dir_all(paths.profile_vendor_dir("p").join("bar@m")).unwrap();

        let profiles = vec![("p".to_string(), Profile::from_json_str(r#"{"name":"p"}"#).unwrap())];
        let s = format_status(&paths, &profiles).unwrap();
        assert!(s.contains("p  (2 vendored)"));
        assert!(s.contains("foo@m"));
        assert!(s.contains("bar@m"));
    }

    #[test]
    fn reports_not_yet_provisioned_when_no_vendor_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let profiles = vec![("p".to_string(), Profile::from_json_str(r#"{"name":"p"}"#).unwrap())];
        let s = format_status(&paths, &profiles).unwrap();
        assert!(s.contains("p  (not yet provisioned)"));
    }
}
