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

pub const RENDER_COMMAND: &str = "claude-profile statusline-render";

#[derive(Debug)]
pub enum InstallOutcome {
    AlreadyInstalled,
    Installed { settings_path: PathBuf, backup_path: PathBuf },
}

pub fn install(scope: Scope, paths: &Paths, cwd: &Path) -> anyhow::Result<InstallOutcome> {
    let target = settings_path(scope, paths, cwd);
    let mut settings = read_json_object(&target)?;
    let current = settings.get("statusLine").cloned().unwrap_or(serde_json::Value::Null);
    if current.get("command").and_then(serde_json::Value::as_str) == Some(RENDER_COMMAND) {
        return Ok(InstallOutcome::AlreadyInstalled);
    }

    let backup_file = backup_path(scope, paths, cwd);
    let mut backup = serde_json::Map::new();
    backup.insert("prior_status_line".to_string(), current);
    write_json_object(&backup_file, &backup)?;

    settings.insert(
        "statusLine".to_string(),
        serde_json::json!({"type": "command", "command": RENDER_COMMAND}),
    );
    write_json_object(&target, &settings)?;

    Ok(InstallOutcome::Installed { settings_path: target, backup_path: backup_file })
}

#[derive(Debug)]
pub enum UninstallOutcome {
    NothingToDo,
    Restored { settings_path: PathBuf },
    ChangedSinceInstall { settings_path: PathBuf },
}

pub fn uninstall(scope: Scope, paths: &Paths, cwd: &Path) -> anyhow::Result<UninstallOutcome> {
    let backup_file = backup_path(scope, paths, cwd);
    if !backup_file.exists() {
        return Ok(UninstallOutcome::NothingToDo);
    }

    let target = settings_path(scope, paths, cwd);
    let mut settings = read_json_object(&target)?;
    let current_cmd = settings
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(serde_json::Value::as_str);
    if current_cmd != Some(RENDER_COMMAND) {
        return Ok(UninstallOutcome::ChangedSinceInstall { settings_path: target });
    }

    let backup = read_json_object(&backup_file)?;
    let prior = backup.get("prior_status_line").cloned().unwrap_or(serde_json::Value::Null);
    if prior.is_null() {
        settings.remove("statusLine");
    } else {
        settings.insert("statusLine".to_string(), prior);
    }
    write_json_object(&target, &settings)?;
    std::fs::remove_file(&backup_file)?;

    Ok(UninstallOutcome::Restored { settings_path: target })
}

const PALETTE: [&str; 6] = [
    "\x1b[31m", // red
    "\x1b[32m", // green
    "\x1b[33m", // yellow
    "\x1b[34m", // blue
    "\x1b[35m", // magenta
    "\x1b[36m", // cyan
];

fn color_index(name: &str) -> usize {
    (name.bytes().map(|b| b as usize).sum::<usize>()) % PALETTE.len()
}

pub fn format_tag(name: &str, color_enabled: bool) -> String {
    if color_enabled {
        format!("{}[{name}]\x1b[0m", PALETTE[color_index(name)])
    } else {
        format!("[{name}]")
    }
}

pub fn compose(tag: Option<&str>, wrapped: Option<&str>) -> String {
    let wrapped = wrapped.filter(|w| !w.is_empty());
    match (tag, wrapped) {
        (Some(t), Some(w)) => format!("{t} {w}"),
        (Some(t), None) => t.to_string(),
        (None, Some(w)) => w.to_string(),
        (None, None) => String::new(),
    }
}

pub fn render_line(profile_name: Option<&str>, color_enabled: bool, wrapped_output: Option<&str>) -> String {
    let tag = profile_name.filter(|n| !n.is_empty()).map(|n| format_tag(n, color_enabled));
    compose(tag.as_deref(), wrapped_output)
}

pub fn run_wrapped_command(command: &str, stdin_bytes: &[u8]) -> anyhow::Result<Option<String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort: a wrapped command that doesn't read stdin (e.g. `echo`) closes its
        // read end early, which would make this write fail with EPIPE. That's expected,
        // not a real error, so it's ignored rather than propagated.
        let _ = stdin.write_all(stdin_bytes);
    }

    let output = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if text.is_empty() { None } else { Some(text) })
}

fn resolve_wrapped_command(paths: &Paths, cwd: &Path) -> Option<String> {
    for scope in [Scope::Project, Scope::Global] {
        let backup_file = backup_path(scope, paths, cwd);
        if !backup_file.exists() {
            continue;
        }
        let Ok(backup) = read_json_object(&backup_file) else { continue };
        let Some(prior) = backup.get("prior_status_line") else { continue };
        if let Some(cmd) = prior.get("command").and_then(serde_json::Value::as_str) {
            return Some(cmd.to_string());
        }
    }
    None
}

/// Top-level entry point invoked by Claude Code as the `statusLine` command. Never
/// errors out: any failure anywhere in this path falls back to an empty string rather
/// than surfacing a Rust error, since a broken statusline command breaks Claude Code's
/// UI chrome.
pub fn render(paths: &Paths, cwd: &Path) -> String {
    let mut input = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut input);

    let wrapped_output = resolve_wrapped_command(paths, cwd)
        .and_then(|cmd| run_wrapped_command(&cmd, &input).ok().flatten());

    let profile = std::env::var("CLAUDE_PROFILE").ok();
    let color_enabled = std::env::var_os("NO_COLOR").is_none();
    render_line(profile.as_deref(), color_enabled, wrapped_output.as_deref())
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

    #[test]
    fn install_creates_settings_file_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();

        let outcome = install(Scope::Global, &paths, &cwd).unwrap();
        assert!(matches!(outcome, InstallOutcome::Installed { .. }));

        let written = read_json_object(&settings_path(Scope::Global, &paths, &cwd)).unwrap();
        assert_eq!(
            written.get("statusLine").unwrap().get("command").unwrap(),
            RENDER_COMMAND
        );
    }

    #[test]
    fn install_preserves_unrelated_settings_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();
        let target = settings_path(Scope::Global, &paths, &cwd);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"model": "opus", "other": {"nested": true}}"#).unwrap();

        install(Scope::Global, &paths, &cwd).unwrap();

        let written = read_json_object(&target).unwrap();
        assert_eq!(written.get("model").unwrap(), "opus");
        assert_eq!(written.get("other").unwrap(), &serde_json::json!({"nested": true}));
        assert_eq!(written.get("statusLine").unwrap().get("command").unwrap(), RENDER_COMMAND);
    }

    #[test]
    fn install_backs_up_prior_status_line() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();
        let target = settings_path(Scope::Global, &paths, &cwd);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"statusLine": {"type": "command", "command": "my-old-script"}}"#).unwrap();

        let outcome = install(Scope::Global, &paths, &cwd).unwrap();
        let backup_file = match outcome {
            InstallOutcome::Installed { backup_path, .. } => backup_path,
            _ => panic!("expected Installed"),
        };
        let backup = read_json_object(&backup_file).unwrap();
        assert_eq!(
            backup.get("prior_status_line").unwrap().get("command").unwrap(),
            "my-old-script"
        );
    }

    #[test]
    fn install_backs_up_null_when_no_prior_status_line() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();

        let outcome = install(Scope::Global, &paths, &cwd).unwrap();
        let backup_file = match outcome {
            InstallOutcome::Installed { backup_path, .. } => backup_path,
            _ => panic!("expected Installed"),
        };
        let backup = read_json_object(&backup_file).unwrap();
        assert!(backup.get("prior_status_line").unwrap().is_null());
    }

    #[test]
    fn install_is_a_noop_when_already_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();

        install(Scope::Global, &paths, &cwd).unwrap();
        let outcome = install(Scope::Global, &paths, &cwd).unwrap();
        assert!(matches!(outcome, InstallOutcome::AlreadyInstalled));
    }

    #[test]
    fn uninstall_is_a_noop_when_nothing_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();

        let outcome = uninstall(Scope::Global, &paths, &cwd).unwrap();
        assert!(matches!(outcome, UninstallOutcome::NothingToDo));
    }

    #[test]
    fn uninstall_restores_prior_command_and_removes_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();
        let target = settings_path(Scope::Global, &paths, &cwd);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"statusLine": {"type": "command", "command": "my-old-script"}}"#).unwrap();

        install(Scope::Global, &paths, &cwd).unwrap();
        let outcome = uninstall(Scope::Global, &paths, &cwd).unwrap();
        assert!(matches!(outcome, UninstallOutcome::Restored { .. }));

        let restored = read_json_object(&target).unwrap();
        assert_eq!(restored.get("statusLine").unwrap().get("command").unwrap(), "my-old-script");
        assert!(!backup_path(Scope::Global, &paths, &cwd).exists());
    }

    #[test]
    fn uninstall_removes_status_line_key_when_there_was_no_prior_one() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();

        install(Scope::Global, &paths, &cwd).unwrap();
        uninstall(Scope::Global, &paths, &cwd).unwrap();

        let restored = read_json_object(&settings_path(Scope::Global, &paths, &cwd)).unwrap();
        assert!(!restored.contains_key("statusLine"));
    }

    #[test]
    fn uninstall_aborts_without_deleting_backup_if_command_changed_since_install() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().to_path_buf();
        let target = settings_path(Scope::Global, &paths, &cwd);

        install(Scope::Global, &paths, &cwd).unwrap();
        // Simulate the user hand-editing settings.json since install.
        let mut settings = read_json_object(&target).unwrap();
        settings.insert(
            "statusLine".to_string(),
            serde_json::json!({"type": "command", "command": "something-else"}),
        );
        write_json_object(&target, &settings).unwrap();

        let outcome = uninstall(Scope::Global, &paths, &cwd).unwrap();
        assert!(matches!(outcome, UninstallOutcome::ChangedSinceInstall { .. }));
        assert!(backup_path(Scope::Global, &paths, &cwd).exists());
        let unchanged = read_json_object(&target).unwrap();
        assert_eq!(unchanged.get("statusLine").unwrap().get("command").unwrap(), "something-else");
    }

    #[test]
    fn format_tag_wraps_name_in_brackets() {
        assert!(format_tag("web-dev", false).contains("[web-dev]"));
        assert_eq!(format_tag("web-dev", false), "[web-dev]");
    }

    #[test]
    fn format_tag_adds_ansi_color_when_enabled() {
        let tag = format_tag("web-dev", true);
        assert!(tag.starts_with("\x1b["));
        assert!(tag.ends_with("\x1b[0m"));
        assert!(tag.contains("[web-dev]"));
    }

    #[test]
    fn format_tag_is_deterministic_per_name() {
        assert_eq!(format_tag("web-dev", true), format_tag("web-dev", true));
    }

    #[test]
    fn format_tag_differs_by_name_color() {
        // byte-sum("a") % 6 = 1, byte-sum("b") % 6 = 2 -- guaranteed distinct palette slots.
        let tag_a = format_tag("a", true);
        let tag_b = format_tag("b", true);
        let color_of = |s: &str| s.split(']').next().unwrap().to_string();
        assert_ne!(color_of(&tag_a), color_of(&tag_b));
    }

    #[test]
    fn compose_handles_all_four_combinations() {
        assert_eq!(compose(None, None), "");
        assert_eq!(compose(Some("[p]"), None), "[p]");
        assert_eq!(compose(None, Some("model info")), "model info");
        assert_eq!(compose(Some("[p]"), Some("model info")), "[p] model info");
    }

    #[test]
    fn compose_treats_empty_wrapped_output_as_absent() {
        assert_eq!(compose(Some("[p]"), Some("")), "[p]");
    }

    #[test]
    fn render_line_omits_tag_when_profile_name_is_none_or_empty() {
        assert_eq!(render_line(None, false, Some("model info")), "model info");
        assert_eq!(render_line(Some(""), false, Some("model info")), "model info");
    }

    #[test]
    fn render_line_combines_tag_and_wrapped_output() {
        assert_eq!(render_line(Some("web-dev"), false, Some("model info")), "[web-dev] model info");
    }

    #[test]
    fn run_wrapped_command_captures_trimmed_stdout() {
        let out = run_wrapped_command("echo hello", &[]).unwrap();
        assert_eq!(out.as_deref(), Some("hello"));
    }

    #[test]
    fn run_wrapped_command_forwards_stdin() {
        let out = run_wrapped_command("cat", b"piped-in").unwrap();
        assert_eq!(out.as_deref(), Some("piped-in"));
    }

    #[test]
    fn run_wrapped_command_returns_none_for_empty_output() {
        let out = run_wrapped_command("true", &[]).unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_wrapped_command_prefers_project_backup_over_global() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut global_backup = serde_json::Map::new();
        global_backup.insert(
            "prior_status_line".to_string(),
            serde_json::json!({"command": "echo global"}),
        );
        write_json_object(&backup_path(Scope::Global, &paths, &cwd), &global_backup).unwrap();

        let mut project_backup = serde_json::Map::new();
        project_backup.insert(
            "prior_status_line".to_string(),
            serde_json::json!({"command": "echo project"}),
        );
        write_json_object(&backup_path(Scope::Project, &paths, &cwd), &project_backup).unwrap();

        assert_eq!(resolve_wrapped_command(&paths, &cwd), Some("echo project".to_string()));
    }

    #[test]
    fn resolve_wrapped_command_falls_back_to_global() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut global_backup = serde_json::Map::new();
        global_backup.insert(
            "prior_status_line".to_string(),
            serde_json::json!({"command": "echo global"}),
        );
        write_json_object(&backup_path(Scope::Global, &paths, &cwd), &global_backup).unwrap();

        assert_eq!(resolve_wrapped_command(&paths, &cwd), Some("echo global".to_string()));
    }

    #[test]
    fn resolve_wrapped_command_is_none_when_no_backups_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        assert_eq!(resolve_wrapped_command(&paths, &cwd), None);
    }

    #[test]
    fn resolve_wrapped_command_is_none_when_prior_status_line_was_null() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();

        let mut backup = serde_json::Map::new();
        backup.insert("prior_status_line".to_string(), serde_json::Value::Null);
        write_json_object(&backup_path(Scope::Global, &paths, &cwd), &backup).unwrap();

        assert_eq!(resolve_wrapped_command(&paths, &cwd), None);
    }
}
