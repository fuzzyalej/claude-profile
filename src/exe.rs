use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

// Windows extensions tried in order: Rust's `Command` only appends `.exe` on its own,
// so an npm-installed `claude.cmd` would otherwise be reported as "program not found".
const WINDOWS_EXTS: &[&str] = &["exe", "cmd", "bat", "com"];

pub fn find_in_path(name: &str, path_var: &OsStr, exts: &[&str]) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        if exts.is_empty() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        for ext in exts {
            let candidate = dir.join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn program(name: &str) -> OsString {
    if cfg!(windows) {
        if let Some(found) = std::env::var_os("PATH").and_then(|p| find_in_path(name, &p, WINDOWS_EXTS)) {
            return found.into_os_string();
        }
    }
    OsString::from(name)
}

pub fn command(name: &str) -> Command {
    Command::new(program(name))
}

// Git for Windows puts only `<Git>\cmd` on PATH by default; its `sh.exe` lives in `<Git>\bin`.
fn sh_next_to_git(git: &Path) -> Option<PathBuf> {
    let root = git.parent()?.parent()?;
    ["bin/sh.exe", "usr/bin/sh.exe"]
        .iter()
        .map(|rel| root.join(rel))
        .find(|p| p.is_file())
}

pub fn find_posix_shell(git_bash_env: Option<OsString>, path_var: &OsStr) -> Option<PathBuf> {
    if let Some(explicit) = git_bash_env.filter(|v| !v.is_empty()).map(PathBuf::from) {
        if explicit.is_file() {
            return Some(explicit);
        }
    }
    if let Some(sh) = find_in_path("sh", path_var, &["exe"]) {
        return Some(sh);
    }
    find_in_path("git", path_var, &["exe"]).and_then(|git| sh_next_to_git(&git))
}

pub fn shell_command(script: &str) -> Command {
    if cfg!(windows) {
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        return match find_posix_shell(std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH"), &path_var) {
            Some(sh) => {
                let mut cmd = Command::new(sh);
                cmd.arg("-c").arg(script);
                cmd
            }
            None => {
                let mut cmd = Command::new("cmd");
                cmd.arg("/C").arg(script);
                cmd
            }
        };
    }
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn finds_cmd_shim_when_no_exe_exists() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("claude.cmd"));
        let found = find_in_path("claude", &path_of(&[t.path()]), WINDOWS_EXTS).unwrap();
        assert_eq!(found, t.path().join("claude.cmd"));
    }

    #[test]
    fn prefers_exe_over_cmd_in_the_same_dir() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("claude.cmd"));
        touch(&t.path().join("claude.exe"));
        let found = find_in_path("claude", &path_of(&[t.path()]), WINDOWS_EXTS).unwrap();
        assert_eq!(found, t.path().join("claude.exe"));
    }

    #[test]
    fn earlier_path_entry_wins() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        touch(&a.path().join("claude.cmd"));
        touch(&b.path().join("claude.exe"));
        let found = find_in_path("claude", &path_of(&[a.path(), b.path()]), WINDOWS_EXTS).unwrap();
        assert_eq!(found, a.path().join("claude.cmd"));
    }

    #[test]
    fn missing_program_is_none() {
        let t = tempfile::tempdir().unwrap();
        assert!(find_in_path("claude", &path_of(&[t.path()]), WINDOWS_EXTS).is_none());
    }

    #[test]
    fn explicit_git_bash_path_wins() {
        let t = tempfile::tempdir().unwrap();
        let bash = t.path().join("custom").join("bash.exe");
        touch(&bash);
        touch(&t.path().join("path").join("sh.exe"));
        let found = find_posix_shell(Some(bash.clone().into()), &path_of(&[&t.path().join("path")]));
        assert_eq!(found, Some(bash));
    }

    #[test]
    fn missing_explicit_git_bash_path_falls_back_to_path() {
        let t = tempfile::tempdir().unwrap();
        let sh = t.path().join("path").join("sh.exe");
        touch(&sh);
        let found = find_posix_shell(Some(t.path().join("nope.exe").into()), &path_of(&[&t.path().join("path")]));
        assert_eq!(found, Some(sh));
    }

    #[test]
    fn derives_sh_from_git_for_windows_layout() {
        let t = tempfile::tempdir().unwrap();
        let git_root = t.path().join("Git");
        touch(&git_root.join("cmd").join("git.exe"));
        touch(&git_root.join("bin").join("sh.exe"));
        let found = find_posix_shell(None, &path_of(&[&git_root.join("cmd")]));
        assert_eq!(found, Some(git_root.join("bin").join("sh.exe")));
    }

    #[test]
    fn no_shell_found_is_none() {
        let t = tempfile::tempdir().unwrap();
        assert!(find_posix_shell(None, &path_of(&[t.path()])).is_none());
    }
}
