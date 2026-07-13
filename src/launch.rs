use crate::enablement::Enablement;
use crate::profile::Profile;
use std::process::Command;

pub fn build_args(profile: &Profile, enablement: &Enablement, extra: &[String]) -> Vec<String> {
    let settings = serde_json::json!({ "enabledPlugins": enablement.enabled_plugins });
    let mut args = vec![
        "--settings".to_string(),
        serde_json::to_string(&settings).unwrap(),
        "--strict-mcp-config".to_string(),
        "--mcp-config".to_string(),
        serde_json::to_string(&serde_json::json!({ "mcpServers": profile.mcp_servers })).unwrap(),
    ];
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
    args
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
    use std::collections::BTreeMap;

    fn enablement() -> Enablement {
        let mut m = BTreeMap::new();
        m.insert("keep@m".to_string(), true);
        m.insert("drop@m".to_string(), false);
        Enablement { enabled_plugins: m, leaking_skills: vec![], suppressed_mcp: vec![] }
    }

    #[test]
    fn assembles_core_args_with_settings_and_strict_mcp() {
        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","pluginDirs":["vendor/x"],"mcpServers":{}}"#).unwrap();
        let args = build_args(&p, &enablement(), &[]);
        assert_eq!(args[0], "--settings");
        let settings: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        assert_eq!(settings["enabledPlugins"]["keep@m"], serde_json::json!(true));
        assert_eq!(settings["enabledPlugins"]["drop@m"], serde_json::json!(false));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        let i = args.iter().position(|a| a == "--mcp-config").unwrap();
        let mcp_config: serde_json::Value = serde_json::from_str(&args[i + 1]).unwrap();
        assert_eq!(mcp_config, serde_json::json!({"mcpServers": {}}));
        let d = args.iter().position(|a| a == "--plugin-dir").unwrap();
        assert_eq!(args[d + 1], "vendor/x");
        assert!(!args.contains(&"--bare".to_string()));
    }

    #[test]
    fn mcp_config_is_wrapped_in_mcpservers() {
        let p = crate::profile::Profile::from_json_str(
            r#"{"name":"p","mcpServers":{"srv":{"command":"echo"}}}"#).unwrap();
        let args = build_args(&p, &enablement(), &[]);
        let i = args.iter().position(|a| a == "--mcp-config").unwrap();
        let mcp_config: serde_json::Value = serde_json::from_str(&args[i + 1]).unwrap();
        assert_eq!(
            mcp_config,
            serde_json::json!({"mcpServers": {"srv": {"command": "echo"}}})
        );
    }

    #[test]
    fn forwards_extra_as_flags_without_double_dash() {
        let p = crate::profile::Profile::from_json_str(r#"{"name":"p","bare":true}"#).unwrap();
        let args = build_args(&p, &enablement(), &["--continue".to_string()]);
        assert!(args.contains(&"--bare".to_string()));
        // No `--` separator: claude would otherwise read the flag as a prompt.
        assert!(!args.contains(&"--".to_string()));
        assert_eq!(args.last().unwrap(), "--continue");
    }
}
