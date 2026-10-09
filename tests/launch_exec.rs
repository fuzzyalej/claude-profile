#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn launch_replaces_wrapper_with_claude() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let profiles = tmp.path().join("profiles");
    let bin = tmp.path().join("bin");
    let cwd = tmp.path().join("cwd");
    for d in [&home, &profiles, &bin, &cwd] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(profiles.join("p.json"), r#"{"name":"p","mcpServers":{}}"#).unwrap();
    let pid_file = tmp.path().join("claude.pid");
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n", pid_file.display()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());

    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-profile"))
        .args(["p", "--yes"])
        .current_dir(&cwd)
        .env("HOME", &home)
        .env("CLAUDE_PROFILE_DIR", &profiles)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut recorded = None;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                recorded = Some(pid);
                break;
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("claude-profile exited early: {status}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(recorded, Some(child.id()));
}
