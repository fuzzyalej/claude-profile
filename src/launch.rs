use crate::fs_paths::Paths;
use crate::profile::Profile;
use std::process::Command;

pub fn build_args(
    profile: &Profile,
    profile_key: &str,
    paths: &Paths,
    extra: &[String],
) -> anyhow::Result<Vec<String>> {
    let mut args = vec![
        "--strict-mcp-config".to_string(),
        "--mcp-config".to_string(),
        serde_json::to_string(&serde_json::json!({ "mcpServers": profile.mcp_servers }))?,
    ];

    let vendor_dir = paths.profile_vendor_dir(profile_key);
    if vendor_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&vendor_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for dir in entries {
            args.push("--plugin-dir".to_string());
            args.push(dir.to_string_lossy().to_string());
        }
    }

    for dir in &profile.plugin_dirs {
        args.push("--plugin-dir".to_string());
        args.push(dir.clone());
    }

    if profile.bare {
        args.push("--bare".to_string());
    }
    // Append forwarded args directly (no `--`): claude treats everything after a
    // `--` as the positional prompt, so a separator would turn flags like
    // `--model opus` into prompt text. As options they parse correctly, and a
    // trailing prompt still lands as the positional arg.
    args.extend(extra.iter().cloned());
    Ok(args)
}

pub fn spawn(profile_name: &str, args: &[String]) -> anyhow::Result<i32> {
    let status = Command::new("claude")
        .args(args)
        .env("CLAUDE_PROFILE", profile_name)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to spawn claude: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_paths::Paths;
    use std::fs;

    #[test]
    fn assembles_plugin_dir_flags_for_vendored_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let vendor = paths.profile_vendor_dir("p");
        fs::create_dir_all(vendor.join("foo@m")).unwrap();
        fs::create_dir_all(vendor.join("bar@m")).unwrap();

        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","plugins":["foo@m","bar@m"],"mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();

        let flags: Vec<&String> = args.iter().collect();
        assert_eq!(flags.iter().filter(|a| a.as_str() == "--plugin-dir").count(), 2);
        assert!(args.contains(&vendor.join("foo@m").to_string_lossy().to_string()));
        assert!(args.contains(&vendor.join("bar@m").to_string_lossy().to_string()));
    }

    #[test]
    fn includes_profile_plugin_dirs_alongside_vendored_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        fs::create_dir_all(paths.profile_vendor_dir("p")).unwrap();

        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","pluginDirs":["vendor/x"],"mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();
        let d = args.iter().position(|a| a == "vendor/x").unwrap();
        assert_eq!(args[d - 1], "--plugin-dir");
    }

    #[test]
    fn no_settings_flag_is_emitted() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let p = crate::profile::Profile::from_json_str(r#"{"name":"p","mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();
        assert!(!args.contains(&"--settings".to_string()));
    }

    #[test]
    fn mcp_config_strict_and_bare_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","mcpServers":{"srv":{"command":"echo"}},"bare":true}"#).unwrap();
        let args = build_args(&p, "p", &paths, &[]).unwrap();
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        let i = args.iter().position(|a| a == "--mcp-config").unwrap();
        let mcp_config: serde_json::Value = serde_json::from_str(&args[i + 1]).unwrap();
        assert_eq!(mcp_config, serde_json::json!({"mcpServers": {"srv": {"command": "echo"}}}));
        assert!(args.contains(&"--bare".to_string()));
    }

    #[test]
    fn forwards_extra_args() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let p = crate::profile::Profile::from_json_str(r#"{"name":"p","mcpServers":{}}"#).unwrap();
        let args = build_args(&p, "p", &paths, &["--model".into(), "opus".into()]).unwrap();
        assert_eq!(&args[args.len() - 2..], &["--model".to_string(), "opus".to_string()]);
    }
}
