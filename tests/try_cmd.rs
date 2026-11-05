use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    tmp: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let env = Env {
            tmp: tempfile::tempdir().unwrap(),
        };
        for d in ["home", "profiles", "cwd", "bin"] {
            std::fs::create_dir_all(env.tmp.path().join(d)).unwrap();
        }
        env
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let path = format!("{}:/usr/bin:/bin", self.path("bin").display());
        Command::new(env!("CARGO_BIN_EXE_claude-profile"))
            .args(args)
            .current_dir(self.path("cwd"))
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("home"))
            .env("CLAUDE_PROFILE_DIR", self.path("profiles"))
            .env("PATH", path)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn dry_run(&self, args: &[&str]) -> Value {
        let mut all = vec!["try", "--dry-run"];
        all.extend_from_slice(args);
        let out = self.run(&all);
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("stdout is profile JSON")
    }

    fn error(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            !out.status.success(),
            "expected failure, stdout: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8_lossy(&out.stderr).to_string()
    }

    fn assert_no_leftovers(&self) {
        let root = self.path("home").join(".claude-profiles");
        let sessions: Vec<_> = std::fs::read_dir(root.join("tmp"))
            .map(|r| r.flatten().collect())
            .unwrap_or_default();
        assert!(sessions.is_empty(), "session dirs left: {sessions:?}");
        let top_json: Vec<_> = std::fs::read_dir(&root)
            .map(|r| {
                r.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "json"))
                    .collect()
            })
            .unwrap_or_default();
        assert!(top_json.is_empty(), "profiles left: {top_json:?}");
        let store_try: Vec<_> = std::fs::read_dir(root.join("store"))
            .map(|r| {
                r.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("try-"))
                    .collect()
            })
            .unwrap_or_default();
        assert!(store_try.is_empty(), "store entries left: {store_try:?}");
        let locks: Vec<_> = std::fs::read_dir(root.join("locks"))
            .map(|r| r.flatten().collect())
            .unwrap_or_default();
        assert!(locks.is_empty(), "locks left: {locks:?}");
    }
}

fn canonical(p: &Path) -> String {
    p.canonicalize().unwrap().to_string_lossy().to_string()
}

#[test]
fn dry_run_npm_prints_stdio_server() {
    let env = Env::new();
    let p = env.dry_run(&["npm:@mobilenext/mobile-mcp"]);
    assert!(p["name"].as_str().unwrap().starts_with("try-"));
    assert_eq!(
        p["mcpServers"]["mobile-mcp"],
        json!({"command": "npx", "args": ["-y", "@mobilenext/mobile-mcp@latest"]})
    );
    env.assert_no_leftovers();
}

#[test]
fn dry_run_http_url_and_explicit_http_prefix() {
    let env = Env::new();
    let p = env.dry_run(&["https://mcp.example.com/mcp", "http:https://github.com/mcp"]);
    assert_eq!(
        p["mcpServers"]["mcp-example-com"],
        json!({"type": "http", "url": "https://mcp.example.com/mcp"})
    );
    assert_eq!(
        p["mcpServers"]["github-com"],
        json!({"type": "http", "url": "https://github.com/mcp"})
    );
}

#[test]
fn dry_run_local_plugin_uses_dir_in_place() {
    let env = Env::new();
    env.write("cwd/plug/.claude-plugin/plugin.json", r#"{"name":"plug"}"#);
    let p = env.dry_run(&["./plug"]);
    assert_eq!(p["pluginDirs"], json!([canonical(&env.path("cwd/plug"))]));
    env.assert_no_leftovers();
}

#[test]
fn dry_run_local_skill_does_not_touch_original() {
    let env = Env::new();
    env.write("cwd/skill/SKILL.md", "---\nname: tidy\n---\nbody");
    let p = env.dry_run(&["./skill"]);
    assert_eq!(p["pluginDirs"].as_array().unwrap().len(), 1);
    assert!(!env.path("cwd/skill/.claude-plugin").exists());
    env.assert_no_leftovers();
}

#[test]
fn dry_run_local_npm_package_with_bin() {
    let env = Env::new();
    env.write(
        "cwd/srv/package.json",
        r#"{"name":"@acme/srv","bin":{"srv":"cli.js"}}"#,
    );
    let p = env.dry_run(&["./srv"]);
    assert_eq!(
        p["mcpServers"]["srv"]["args"],
        json!(["-y", "@acme/srv@latest"])
    );
}

#[test]
fn dry_run_command_with_env() {
    let env = Env::new();
    let p = env.dry_run(&[
        "--command",
        "uvx my-server --stdio",
        "--env",
        "TOKEN=abc",
        "--env",
        "MODE=x",
    ]);
    assert_eq!(
        p["mcpServers"]["my-server"],
        json!({"command": "uvx", "args": ["my-server", "--stdio"], "env": {"TOKEN": "abc", "MODE": "x"}})
    );
}

#[test]
fn dry_run_with_base_profile_merges() {
    let env = Env::new();
    env.write(
        "profiles/base.json",
        r#"{"name":"base","marketplaces":{"m":"o/m"},"plugins":["a@m"],"mcpServers":{"b":{"command":"b"}}}"#,
    );
    let p = env.dry_run(&["--with", "base", "npm:x", "z@m"]);
    assert_eq!(p["plugins"], json!(["a@m", "z@m"]));
    assert_eq!(p["marketplaces"], json!({"m": "o/m"}));
    assert!(p["mcpServers"]["b"].is_object() && p["mcpServers"]["x"].is_object());
    assert!(p.get("extends").is_none());
}

#[test]
fn unknown_marketplace_is_a_clear_error() {
    let env = Env::new();
    let err = env.error(&["try", "--dry-run", "a@nowhere"]);
    assert!(err.contains("unknown marketplace 'nowhere'"), "{err}");
    env.assert_no_leftovers();
}

#[test]
fn undetectable_dir_suggests_npm_or_command() {
    let env = Env::new();
    env.write("cwd/lib/README.md", "hi");
    let err = env.error(&["try", "--dry-run", "./lib"]);
    assert!(
        err.contains("npm:<package>") && err.contains("--command"),
        "{err}"
    );
    env.assert_no_leftovers();
}

#[test]
fn profile_repo_suggests_launching_directly() {
    let env = Env::new();
    env.write("cwd/prof/profile.json", r#"{"name":"p"}"#);
    let err = env.error(&["try", "--dry-run", "./prof"]);
    assert!(err.contains("profile repo"), "{err}");
}

#[test]
fn save_refuses_to_overwrite() {
    let env = Env::new();
    env.write("home/.claude-profiles/taken.json", r#"{"name":"taken"}"#);
    let err = env.error(&["try", "--save", "taken", "--yes", "npm:x"]);
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn save_conflicts_with_dry_run() {
    let env = Env::new();
    env.error(&["try", "--save", "x", "--dry-run", "npm:x"]);
}

#[test]
fn needs_a_target() {
    let env = Env::new();
    let err = env.error(&["try", "--dry-run"]);
    assert!(err.contains("nothing to try"), "{err}");
}

#[cfg(unix)]
fn fake_claude(env: &Env) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let args_file = env.path("claude.args");
    let fake = env.path("bin/claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho \"$CLAUDE_PROFILE\" > '{0}'\nfor a in \"$@\"; do echo \"$a\" >> '{0}'; done\nexit 7\n",
            args_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    args_file
}

#[cfg(unix)]
#[test]
fn launch_runs_claude_then_removes_everything() {
    let env = Env::new();
    let args_file = fake_claude(&env);
    env.write("cwd/skill/SKILL.md", "# s");
    let out = env.run(&[
        "try", "--yes", "npm:pkg", "./skill", "--", "--model", "opus",
    ]);
    assert_eq!(
        out.status.code(),
        Some(7),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let recorded = std::fs::read_to_string(&args_file).unwrap();
    let lines: Vec<&str> = recorded.lines().collect();
    assert!(lines[0].starts_with("try-"));
    assert!(recorded.contains("\"command\":\"npx\""));
    assert!(recorded.contains("--plugin-dir"));
    assert_eq!(&lines[lines.len() - 2..], &["--model", "opus"]);
    env.assert_no_leftovers();
}

#[cfg(unix)]
#[test]
fn save_keeps_profile_and_launches_under_its_name() {
    let env = Env::new();
    let args_file = fake_claude(&env);
    env.write("cwd/skill/SKILL.md", "# s");
    let out = env.run(&["try", "--yes", "--save", "kept", "./skill"]);
    assert_eq!(
        out.status.code(),
        Some(7),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&args_file).unwrap().lines().next(),
        Some("kept")
    );
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(env.path("home/.claude-profiles/kept.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["name"], "kept");
    let dir = PathBuf::from(saved["pluginDirs"][0].as_str().unwrap());
    assert!(dir.join(".claude-plugin/plugin.json").is_file());
    assert!(dir.starts_with(env.path("home/.claude-profiles/store/kept")));
}

#[test]
fn confirmation_decline_aborts_without_launch() {
    let env = Env::new();
    let out = env.run(&["try", "npm:pkg"]);
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pkg: npx -y pkg@latest"), "{stdout}");
    env.assert_no_leftovers();
}
