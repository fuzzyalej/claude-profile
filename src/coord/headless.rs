use crate::coord::backend::{launch_args, Backend, LaunchSpec, Observed};
use crate::coord::state::WorkerState;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

pub struct Headless {
    children: HashMap<String, Child>,
    dirs: HashMap<String, PathBuf>,
    #[cfg(windows)]
    jobs: HashMap<String, job::Job>,
}

impl Headless {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
            dirs: HashMap::new(),
            #[cfg(windows)]
            jobs: HashMap::new(),
        }
    }

    fn kill(&mut self, id: &str) {
        #[cfg(windows)]
        if let Some(job) = self.jobs.remove(id) {
            job.terminate();
        }
        if let Some(mut child) = self.children.remove(id) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn spawn(&mut self, w: &mut WorkerState, spec: &LaunchSpec, tail: &[String], prompt: &str) -> Result<()> {
        self.kill(&w.id);
        let dir = worker_dir(w, spec);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let out = File::create(dir.join("output.json")).context("creating output.json")?;
        let err = File::create(dir.join("stderr.log")).context("creating stderr.log")?;
        let mut child = Command::new(&spec.exe)
            .args(launch_args(spec, tail))
            .current_dir(&w.cwd)
            .stdin(Stdio::piped())
            .stdout(out)
            .stderr(err)
            .spawn()
            .with_context(|| format!("launching {}", spec.exe.display()))?;
        if let Some(mut stdin) = child.stdin.take() {
            let prompt = prompt.to_string();
            std::thread::spawn(move || {
                let _ = stdin.write_all(prompt.as_bytes());
            });
        }
        w.pid = Some(child.id());
        #[cfg(windows)]
        if let Some(job) = job::Job::assign(&child) {
            self.jobs.insert(w.id.clone(), job);
        }
        self.dirs.insert(w.id.clone(), dir);
        self.children.insert(w.id.clone(), child);
        Ok(())
    }
}

fn worker_dir(w: &WorkerState, spec: &LaunchSpec) -> PathBuf {
    spec.run_dir.join(&w.id)
}

pub fn start_tail(session_id: &str) -> Vec<String> {
    vec![
        "-p".into(),
        "--session-id".into(),
        session_id.into(),
        "--output-format".into(),
        "json".into(),
    ]
}

pub fn resume_tail(session_id: &str) -> Vec<String> {
    vec![
        "-p".into(),
        "--resume".into(),
        session_id.into(),
        "--output-format".into(),
        "json".into(),
    ]
}

pub fn final_message(output_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(output_json).ok()?;
    v.get("result")?.as_str().map(str::to_string)
}

impl Backend for Headless {
    fn start(&mut self, w: &mut WorkerState, spec: &LaunchSpec, prompt: &str) -> Result<()> {
        let tail = start_tail(&w.session_id);
        self.spawn(w, spec, &tail, prompt)
    }

    fn send(&mut self, w: &mut WorkerState, spec: &LaunchSpec, prompt: &str) -> Result<()> {
        let tail = resume_tail(&w.session_id);
        self.spawn(w, spec, &tail, prompt)
    }

    fn observe(&mut self, w: &WorkerState) -> Result<Observed> {
        let Some(child) = self.children.get_mut(&w.id) else {
            return Ok(Observed::Gone);
        };
        Ok(match child.try_wait()? {
            None => Observed::Running,
            Some(status) => Observed::Exited(status.code().unwrap_or(-1)),
        })
    }

    fn last_output(&mut self, w: &WorkerState, lines: usize) -> Result<String> {
        let Some(dir) = self.dirs.get(&w.id) else {
            return Ok(String::new());
        };
        if let Ok(raw) = std::fs::read_to_string(dir.join("output.json")) {
            if let Some(msg) = final_message(&raw) {
                return Ok(msg);
            }
        }
        let log = std::fs::read_to_string(dir.join("stderr.log")).unwrap_or_default();
        let all: Vec<&str> = log.lines().collect();
        let start = all.len().saturating_sub(lines);
        Ok(all[start..].join("\n"))
    }

    fn stop(&mut self, w: &WorkerState) -> Result<()> {
        self.kill(&w.id);
        Ok(())
    }

    fn close(&mut self, _w: &WorkerState) -> Result<()> {
        Ok(())
    }
}

impl Drop for Headless {
    fn drop(&mut self) {
        let ids: Vec<String> = self.children.keys().cloned().collect();
        for id in ids {
            self.kill(&id);
        }
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);

    impl Job {
        pub fn assign(child: &Child) -> Option<Job> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Job(handle);
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 || AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) == 0 {
                    return None;
                }
                Some(job)
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::backend::{Backend, LaunchSpec, Observed};
    use crate::coord::state::{Status, WorkerState};

    fn worker(cwd: &std::path::Path) -> WorkerState {
        WorkerState {
            id: "w1".into(),
            name: "w1".into(),
            profiles: vec![],
            task: "t".into(),
            status: Status::Starting,
            status_detail: None,
            cwd: cwd.to_path_buf(),
            worktree: None,
            branch: None,
            base: None,
            session_id: "sid".into(),
            pane_id: None,
            pid: None,
            created_at: 0,
            updated_at: 0,
            idle_since: None,
        }
    }

    #[test]
    fn tails_leave_prompt_out_of_argv() {
        assert_eq!(start_tail("sid"), ["-p", "--session-id", "sid", "--output-format", "json"]);
        assert_eq!(resume_tail("sid"), ["-p", "--resume", "sid", "--output-format", "json"]);
    }

    #[cfg(unix)]
    #[test]
    fn prompt_reaches_child_verbatim_on_stdin() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude-profile");
        let seen = dir.path().join("stdin.txt");
        std::fs::write(&script, format!("#!/bin/sh\ncat > '{}'\n", seen.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cwd = dir.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let spec = LaunchSpec {
            exe: script,
            profiles: vec![],
            permission_flags: vec![],
            run_dir: dir.path().join("run"),
        };
        let prompt = "say \"hi\"\nit's $HOME ñ\n-- --flag";
        let mut w = worker(&cwd);
        let mut h = Headless::new();
        h.start(&mut w, &spec, prompt).unwrap();
        for _ in 0..200 {
            if h.observe(&w).unwrap() != Observed::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), prompt);
    }

    #[test]
    fn final_message_reads_result() {
        assert_eq!(
            final_message("{\"type\":\"result\",\"result\":\"ok\"}"),
            Some("ok".to_string())
        );
        assert_eq!(final_message("not json"), None);
    }

    #[cfg(unix)]
    fn sleeper(dir: &std::path::Path) -> (LaunchSpec, std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("fake-claude-profile");
        let pid_file = dir.join("sleep.pid");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n", pid_file.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cwd = dir.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let spec = LaunchSpec {
            exe: script,
            profiles: vec![],
            permission_flags: vec![],
            run_dir: dir.join("run"),
        };
        (spec, cwd, pid_file)
    }

    #[cfg(unix)]
    fn wait_pid(pid_file: &std::path::Path) -> u32 {
        for _ in 0..400 {
            if let Ok(text) = std::fs::read_to_string(pid_file) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("no pid written");
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[cfg(unix)]
    #[test]
    fn stop_kills_process_exec_from_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, cwd, pid_file) = sleeper(dir.path());
        let mut w = worker(&cwd);
        let mut h = Headless::new();
        h.start(&mut w, &spec, "go").unwrap();
        let pid = wait_pid(&pid_file);
        assert!(alive(pid));
        h.stop(&w).unwrap();
        assert!(!alive(pid));
        assert_eq!(h.observe(&w).unwrap(), Observed::Gone);
    }

    #[cfg(unix)]
    #[test]
    fn drop_kills_running_child() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, cwd, pid_file) = sleeper(dir.path());
        let mut w = worker(&cwd);
        let mut h = Headless::new();
        h.start(&mut w, &spec, "go").unwrap();
        let pid = wait_pid(&pid_file);
        assert!(alive(pid));
        drop(h);
        assert!(!alive(pid));
    }

    #[cfg(unix)]
    #[test]
    fn runs_child_and_observes_exit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude-profile");
        std::fs::write(&script, "#!/bin/sh\necho '{\"result\":\"done\"}'\nexit 0\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run_dir = dir.path().join("run");
        let cwd = dir.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let spec = LaunchSpec {
            exe: script,
            profiles: vec!["rust".into()],
            permission_flags: vec![],
            run_dir,
        };
        let mut w = worker(&cwd);
        let mut h = Headless::new();
        h.start(&mut w, &spec, "go").unwrap();
        assert!(w.pid.is_some());
        let mut seen = Observed::Running;
        for _ in 0..200 {
            seen = h.observe(&w).unwrap();
            if seen != Observed::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(seen, Observed::Exited(0));
        assert_eq!(h.last_output(&w, 5).unwrap(), "done");
    }

    #[cfg(unix)]
    #[test]
    fn send_replaces_running_child() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude-profile");
        std::fs::write(
            &script,
            "#!/bin/sh\ncase \"$*\" in\n*--resume*) echo '{\"result\":\"second\"}';;\n*) sleep 5;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cwd = dir.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let spec = LaunchSpec {
            exe: script,
            profiles: vec![],
            permission_flags: vec![],
            run_dir: dir.path().join("run"),
        };
        let mut w = worker(&cwd);
        let mut h = Headless::new();
        h.start(&mut w, &spec, "first").unwrap();
        let first = w.pid.unwrap();
        h.send(&mut w, &spec, "second").unwrap();
        assert_ne!(w.pid.unwrap(), first);
        let alive = std::process::Command::new("kill")
            .args(["-0", &first.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive);
        let mut seen = Observed::Running;
        for _ in 0..200 {
            seen = h.observe(&w).unwrap();
            if seen != Observed::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(seen, Observed::Exited(0));
        assert_eq!(h.last_output(&w, 5).unwrap(), "second");
    }
}
