use crate::coord::state::WorkerState;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct LaunchSpec {
    pub exe: PathBuf,
    pub profiles: Vec<String>,
    pub permission_flags: Vec<String>,
    pub run_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Observed {
    Running,
    Blocked,
    Idle,
    Exited(i32),
    Gone,
}

pub trait Backend {
    fn start(&mut self, w: &mut WorkerState, spec: &LaunchSpec, prompt: &str) -> Result<()>;
    fn send(&mut self, w: &mut WorkerState, spec: &LaunchSpec, prompt: &str) -> Result<()>;
    fn observe(&mut self, w: &WorkerState) -> Result<Observed>;
    fn last_output(&mut self, w: &WorkerState, lines: usize) -> Result<String>;
    fn stop(&mut self, w: &WorkerState) -> Result<()>;
    fn close(&mut self, w: &WorkerState) -> Result<()>;
}

pub fn worker_prompt(task: &str, result_path: &Path, commit: bool) -> String {
    let commit = if commit {
        "Commit your changes on the current branch before you finish. "
    } else {
        ""
    };
    format!(
        "{task}\n\n{commit}When you finish, write {} with {{\"status\": \"done\" | \"failed\", \"summary\": \"...\", \"notes\": \"...\"}}. Keep the summary under 200 words.",
        result_path.display()
    )
}

pub fn launch_args(spec: &LaunchSpec, tail: &[String]) -> Vec<String> {
    let mut args = spec.profiles.clone();
    args.push("--yes".to_string());
    args.push("--".to_string());
    args.extend(tail.iter().cloned());
    args.push("--add-dir".to_string());
    args.push(spec.run_dir.display().to_string());
    args.extend(spec.permission_flags.iter().cloned());
    args
}

#[cfg(test)]
#[derive(Default)]
pub struct FakeBackend {
    pub observed: std::collections::HashMap<String, Observed>,
    pub output: std::collections::HashMap<String, String>,
    pub started: Vec<(String, String)>,
    pub sent: Vec<(String, String)>,
    pub stopped: Vec<String>,
    pub closed: Vec<String>,
    pub fail_start: bool,
    pub fail_output: bool,
}

#[cfg(test)]
impl Backend for FakeBackend {
    fn start(&mut self, w: &mut WorkerState, _spec: &LaunchSpec, prompt: &str) -> Result<()> {
        if self.fail_start {
            anyhow::bail!("start failed");
        }
        self.started.push((w.id.clone(), prompt.to_string()));
        Ok(())
    }

    fn send(&mut self, w: &mut WorkerState, _spec: &LaunchSpec, prompt: &str) -> Result<()> {
        if self.fail_start {
            anyhow::bail!("send failed");
        }
        self.sent.push((w.id.clone(), prompt.to_string()));
        Ok(())
    }

    fn observe(&mut self, w: &WorkerState) -> Result<Observed> {
        Ok(self.observed.get(&w.id).copied().unwrap_or(Observed::Running))
    }

    fn last_output(&mut self, w: &WorkerState, _lines: usize) -> Result<String> {
        if self.fail_output {
            anyhow::bail!("agent read failed");
        }
        Ok(self.output.get(&w.id).cloned().unwrap_or_default())
    }

    fn stop(&mut self, w: &WorkerState) -> Result<()> {
        self.stopped.push(w.id.clone());
        Ok(())
    }

    fn close(&mut self, w: &WorkerState) -> Result<()> {
        self.closed.push(w.id.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn worker_prompt_appends_contract() {
        let p = worker_prompt("do it", Path::new("/r/result.json"), false);
        assert_eq!(
            p,
            "do it\n\nWhen you finish, write /r/result.json with {\"status\": \"done\" | \"failed\", \"summary\": \"...\", \"notes\": \"...\"}. Keep the summary under 200 words."
        );
    }

    #[test]
    fn worker_prompt_asks_worktree_workers_to_commit() {
        let p = worker_prompt("do it", Path::new("/r/result.json"), true);
        assert_eq!(
            p,
            "do it\n\nCommit your changes on the current branch before you finish. When you finish, write /r/result.json with {\"status\": \"done\" | \"failed\", \"summary\": \"...\", \"notes\": \"...\"}. Keep the summary under 200 words."
        );
    }

    #[test]
    fn launch_args_order() {
        let spec = LaunchSpec {
            exe: "claude-profile".into(),
            profiles: vec!["rust".into(), "docs".into()],
            permission_flags: vec!["--permission-mode".into(), "plan".into()],
            run_dir: "/r".into(),
        };
        let tail = vec!["-p".to_string(), "x".to_string()];
        assert_eq!(
            launch_args(&spec, &tail),
            ["rust", "docs", "--yes", "--", "-p", "x", "--add-dir", "/r", "--permission-mode", "plan"]
        );
    }

    #[test]
    fn launch_args_never_carry_the_coordinator_policy() {
        let spec = LaunchSpec {
            exe: "claude-profile".into(),
            profiles: vec!["rust".into()],
            permission_flags: vec!["--permission-mode".into(), "plan".into()],
            run_dir: "/r".into(),
        };
        let args = launch_args(&spec, &["-p".to_string(), "x".to_string()]);
        assert!(!args.iter().any(|a| a.starts_with("--append-system-prompt")));
    }
}
