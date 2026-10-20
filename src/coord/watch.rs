use crate::coord::backend::{Backend, Observed};
use crate::coord::state::{write_atomic, BackendKind, RunDir, RunState, Status, WorkerState};
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

const IDLE_GRACE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Done,
    Failed,
    Blocked,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Done => "done",
            Kind::Failed => "failed",
            Kind::Blocked => "blocked",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Events(Vec<String>),
    Timeout,
    NoActive,
}

struct Seen {
    kind: Kind,
    line: String,
}

fn cursor_path(dir: &RunDir) -> std::path::PathBuf {
    dir.root.join("notified.json")
}

fn load_cursor(dir: &RunDir) -> BTreeMap<String, String> {
    std::fs::read(cursor_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_cursor(dir: &RunDir, cursor: &BTreeMap<String, String>) -> Result<()> {
    write_atomic(&cursor_path(dir), &serde_json::to_vec_pretty(cursor)?)
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    match line.char_indices().nth(160) {
        Some((i, _)) => format!("{}...", &line[..i]),
        None => line.to_string(),
    }
}

fn read_result(dir: &RunDir, id: &str) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(dir.result_path(id)).ok()?).ok()
}

fn failed(reason: &str) -> Seen {
    let line = if reason.is_empty() { "failed".to_string() } else { format!("failed: {reason}") };
    Seen { kind: Kind::Failed, line }
}

fn blocked(w: &WorkerState) -> Seen {
    Seen {
        kind: Kind::Blocked,
        line: format!("blocked pane {}", w.pane_id.as_deref().unwrap_or("?")),
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(out) = crate::exe::command("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output() else {
        return true;
    };
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

fn classify<B: Backend>(
    dir: &RunDir,
    run: &RunState,
    w: &WorkerState,
    herdr: &mut Option<B>,
    idle: &mut HashMap<String, Instant>,
) -> Option<Seen> {
    if matches!(w.status, Status::Queued | Status::Cancelled) {
        return None;
    }
    if let Some(result) = read_result(dir, &w.id) {
        match result.get("status").and_then(Value::as_str) {
            Some("done") => return Some(Seen { kind: Kind::Done, line: "done".into() }),
            Some("failed") => {
                let summary = result.get("summary").and_then(Value::as_str).unwrap_or("");
                return Some(failed(&first_line(summary)));
            }
            _ => {}
        }
    }
    match w.status {
        Status::Done => return Some(Seen { kind: Kind::Done, line: "done".into() }),
        Status::Failed => return Some(failed(&first_line(w.status_detail.as_deref().unwrap_or("")))),
        _ => {}
    }
    if run.backend == BackendKind::Herdr && w.pane_id.is_some() {
        if let Some(backend) = herdr.as_mut() {
            match backend.observe(w) {
                Ok(Observed::Blocked) => {
                    idle.remove(&w.id);
                    return Some(blocked(w));
                }
                Ok(Observed::Gone) => return Some(failed("pane closed")),
                Ok(Observed::Exited(_)) => return Some(failed("process exited without a result")),
                Ok(Observed::Idle) => {
                    let since = *idle.entry(w.id.clone()).or_insert_with(Instant::now);
                    if since.elapsed() >= IDLE_GRACE {
                        return Some(Seen { kind: Kind::Done, line: "done".into() });
                    }
                    return None;
                }
                Ok(Observed::Running) => {
                    idle.remove(&w.id);
                    return None;
                }
                Err(_) => {}
            }
        }
    } else if let Some(pid) = w.pid {
        if !process_alive(pid) {
            return Some(failed("process exited without a result"));
        }
    }
    if w.status == Status::Blocked {
        return Some(blocked(w));
    }
    None
}

fn is_pending(run_live: bool, w: &WorkerState, seen: &Option<Seen>) -> bool {
    match seen {
        Some(s) => s.kind == Kind::Blocked,
        None => w.status.is_active() || (w.status == Status::Queued && run_live),
    }
}

pub fn poll_once<B: Backend>(
    dir: &RunDir,
    run: &RunState,
    herdr: &mut Option<B>,
    idle: &mut HashMap<String, Instant>,
) -> Result<Outcome> {
    let live = dir.is_live();
    let mut cursor = load_cursor(dir);
    let before = cursor.clone();
    let mut events = Vec::new();
    let mut pending = 0;
    for w in dir.workers()? {
        let seen = classify(dir, run, &w, herdr, idle);
        if is_pending(live, &w, &seen) {
            pending += 1;
        }
        match seen {
            Some(s) => {
                if cursor.get(&w.id).map(String::as_str) != Some(s.kind.name()) {
                    cursor.insert(w.id.clone(), s.kind.name().to_string());
                    events.push(format!("{} {}", w.id, s.line));
                }
            }
            None => {
                cursor.remove(&w.id);
            }
        }
    }
    if cursor != before {
        save_cursor(dir, &cursor)?;
    }
    if !events.is_empty() {
        return Ok(Outcome::Events(events));
    }
    if pending == 0 {
        return Ok(Outcome::NoActive);
    }
    Ok(Outcome::Timeout)
}

pub fn wait<B: Backend>(
    dir: &RunDir,
    run: &RunState,
    mut herdr: Option<B>,
    timeout: Option<Duration>,
    poll: Duration,
) -> Result<Outcome> {
    let start = Instant::now();
    let mut idle = HashMap::new();
    loop {
        match poll_once(dir, run, &mut herdr, &mut idle)? {
            Outcome::Timeout => {}
            other => return Ok(other),
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            return Ok(Outcome::Timeout);
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::backend::FakeBackend;
    use crate::fs_paths::Paths;
    use std::path::PathBuf;

    fn run_state(backend: BackendKind) -> RunState {
        RunState {
            run_id: "1-aaa".into(),
            repo_root: None,
            cwd: PathBuf::from("/x"),
            permission_flags: vec![],
            backend,
            max_workers: 4,
            coordinator_pane: Some("w1:p1".into()),
            created_at: 1,
        }
    }

    fn worker(id: &str, status: Status) -> WorkerState {
        WorkerState {
            id: id.into(),
            name: id.into(),
            profiles: vec![],
            task: "t".into(),
            status,
            status_detail: None,
            cwd: PathBuf::from("/x"),
            worktree: None,
            branch: None,
            base: None,
            session_id: "s".into(),
            pane_id: None,
            pid: None,
            created_at: 1,
            updated_at: 1,
            idle_since: None,
            model: None,
        }
    }

    fn setup(backend: BackendKind) -> (tempfile::TempDir, RunDir, RunState) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let run = run_state(backend);
        let dir = RunDir::create(&paths, &run).unwrap();
        (tmp, dir, run)
    }

    fn write_result(dir: &RunDir, id: &str, body: &str) {
        std::fs::create_dir_all(dir.worker_dir(id)).unwrap();
        std::fs::write(dir.result_path(id), body).unwrap();
    }

    fn quick(dir: &RunDir, run: &RunState, herdr: Option<FakeBackend>, timeout: Duration) -> Outcome {
        wait(dir, run, herdr, Some(timeout), Duration::from_millis(10)).unwrap()
    }

    #[test]
    fn reports_finished_worker_once() {
        let (_t, dir, run) = setup(BackendKind::Headless);
        let mut w = worker("a", Status::Working);
        w.pid = Some(std::process::id());
        dir.save_worker(&w).unwrap();
        write_result(&dir, "a", r#"{"status":"done","summary":"ok"}"#);
        assert_eq!(quick(&dir, &run, None, Duration::ZERO), Outcome::Events(vec!["a done".into()]));
        assert_eq!(quick(&dir, &run, None, Duration::ZERO), Outcome::NoActive);
        let cursor: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(dir.root.join("notified.json")).unwrap()).unwrap();
        assert_eq!(cursor.get("a").map(String::as_str), Some("done"));
    }

    #[test]
    fn reports_event_from_before_the_command_started() {
        let (_t, dir, run) = setup(BackendKind::Headless);
        dir.save_worker(&worker("early", Status::Done)).unwrap();
        let mut f = worker("fail", Status::Failed);
        f.status_detail = Some("pane closed".into());
        dir.save_worker(&f).unwrap();
        assert_eq!(
            quick(&dir, &run, None, Duration::ZERO),
            Outcome::Events(vec!["early done".into(), "fail failed: pane closed".into()])
        );
    }

    #[test]
    fn failed_result_carries_summary() {
        let (_t, dir, run) = setup(BackendKind::Headless);
        let mut w = worker("a", Status::Working);
        w.pid = Some(std::process::id());
        dir.save_worker(&w).unwrap();
        write_result(&dir, "a", r#"{"status":"failed","summary":"build broke\nmore"}"#);
        assert_eq!(
            quick(&dir, &run, None, Duration::ZERO),
            Outcome::Events(vec!["a failed: build broke".into()])
        );
    }

    #[test]
    fn times_out_while_workers_run() {
        let (_t, dir, run) = setup(BackendKind::Headless);
        let mut w = worker("a", Status::Working);
        w.pid = Some(std::process::id());
        dir.save_worker(&w).unwrap();
        assert_eq!(quick(&dir, &run, None, Duration::from_millis(30)), Outcome::Timeout);
        assert!(!dir.root.join("notified.json").exists());
    }

    #[test]
    fn no_active_workers_returns_immediately() {
        let (_t, dir, run) = setup(BackendKind::Headless);
        assert_eq!(wait::<FakeBackend>(&dir, &run, None, None, Duration::from_secs(60)).unwrap(), Outcome::NoActive);
        dir.save_worker(&worker("q", Status::Queued)).unwrap();
        assert_eq!(wait::<FakeBackend>(&dir, &run, None, None, Duration::from_secs(60)).unwrap(), Outcome::NoActive);
    }

    #[test]
    fn blocked_is_reported_again_after_unblocking() {
        let (_t, dir, run) = setup(BackendKind::Herdr);
        let mut w = worker("a", Status::Working);
        w.pane_id = Some("w1:p2".into());
        dir.save_worker(&w).unwrap();
        let mut fake = FakeBackend::default();
        fake.observed.insert("a".into(), Observed::Blocked);
        let mut herdr = Some(fake);
        let mut idle = HashMap::new();
        let first = poll_once(&dir, &run, &mut herdr, &mut idle).unwrap();
        assert_eq!(first, Outcome::Events(vec!["a blocked pane w1:p2".into()]));
        assert_eq!(poll_once(&dir, &run, &mut herdr, &mut idle).unwrap(), Outcome::Timeout);
        herdr.as_mut().unwrap().observed.insert("a".into(), Observed::Running);
        assert_eq!(poll_once(&dir, &run, &mut herdr, &mut idle).unwrap(), Outcome::Timeout);
        herdr.as_mut().unwrap().observed.insert("a".into(), Observed::Blocked);
        assert_eq!(poll_once(&dir, &run, &mut herdr, &mut idle).unwrap(), first);
    }

    #[test]
    fn gone_pane_is_failed() {
        let (_t, dir, run) = setup(BackendKind::Herdr);
        let mut w = worker("a", Status::Working);
        w.pane_id = Some("w1:p2".into());
        dir.save_worker(&w).unwrap();
        let mut fake = FakeBackend::default();
        fake.observed.insert("a".into(), Observed::Gone);
        assert_eq!(
            quick(&dir, &run, Some(fake), Duration::ZERO),
            Outcome::Events(vec!["a failed: pane closed".into()])
        );
    }

    #[cfg(unix)]
    #[test]
    fn exited_headless_process_is_failed() {
        let (_t, dir, run) = setup(BackendKind::Headless);
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let mut w = worker("a", Status::Working);
        w.pid = Some(pid);
        dir.save_worker(&w).unwrap();
        assert_eq!(
            quick(&dir, &run, None, Duration::ZERO),
            Outcome::Events(vec!["a failed: process exited without a result".into()])
        );
    }

    #[test]
    fn never_writes_worker_state() {
        let (_t, dir, run) = setup(BackendKind::Headless);
        let mut w = worker("a", Status::Working);
        w.pid = Some(std::process::id());
        dir.save_worker(&w).unwrap();
        write_result(&dir, "a", r#"{"status":"done"}"#);
        let before = std::fs::read(dir.worker_dir("a").join("worker.json")).unwrap();
        quick(&dir, &run, None, Duration::ZERO);
        assert_eq!(std::fs::read(dir.worker_dir("a").join("worker.json")).unwrap(), before);
    }
}
