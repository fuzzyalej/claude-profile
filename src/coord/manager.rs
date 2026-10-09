use crate::coord::backend::{worker_prompt, Backend, LaunchSpec, Observed};
use crate::coord::state::{new_worker_id, now, BackendKind, RunDir, RunState, Status, WorkerState};
use crate::coord::worktree;
use crate::fs_paths::Paths;
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub type PrepareFn = dyn Fn(&[String]) -> Result<()>;

const IDLE_GRACE_SECS: u64 = 30;
const FALLBACK_LINES: usize = 50;

pub struct Manager<B: Backend> {
    pub run: RunState,
    pub dir: RunDir,
    pub backend: B,
    pub spec_base: (PathBuf, PathBuf),
    pub profile_exists: Box<dyn Fn(&str) -> bool>,
    pub list: Box<dyn Fn() -> Vec<(String, Option<String>)>>,
    pub paths: Paths,
    pub fallback: Box<dyn FnMut() -> Box<dyn Backend>>,
    pub fallback_backend: Option<Box<dyn Backend>>,
    pub prepare: Box<PrepareFn>,
}

pub struct SpawnArgs {
    pub profiles: Vec<String>,
    pub task: String,
    pub worktree: bool,
    pub base_ref: Option<String>,
    pub name: Option<String>,
}

fn file_status(v: &Value) -> Option<Status> {
    match v.get("status")?.as_str()? {
        "done" => Some(Status::Done),
        "failed" => Some(Status::Failed),
        _ => None,
    }
}

fn status_str(s: Status) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

fn path_str(p: &std::path::Path) -> String {
    p.display().to_string()
}

impl<B: Backend> Manager<B> {
    pub fn list_profiles(&mut self) -> Result<Value> {
        self.refresh()?;
        let items: Vec<Value> = (self.list)()
            .into_iter()
            .map(|(name, description)| json!({"name": name, "description": description}))
            .collect();
        Ok(Value::Array(items))
    }

    pub fn spawn(&mut self, args: SpawnArgs) -> Result<Value> {
        if args.profiles.is_empty() {
            bail!("profiles must name at least one profile");
        }
        self.refresh()?;
        for p in &args.profiles {
            if !(self.profile_exists)(p) {
                bail!("unknown profile '{p}'");
            }
        }
        if args.worktree && self.run.repo_root.is_none() {
            bail!("not a git repository; call spawn with worktree=false");
        }
        let queue = self.active_count()? >= self.run.max_workers;
        if !queue {
            (self.prepare)(&args.profiles)?;
        }
        let existing = self.taken_ids()?;
        let label = args.name.clone().unwrap_or_else(|| args.profiles.join("-"));
        let id = new_worker_id(&label, &existing);
        let mut warning = None;
        let (cwd, worktree, branch, base) = match (&self.run.repo_root, args.worktree) {
            (Some(repo), true) => {
                if worktree::is_dirty(repo)? {
                    warning = Some(format!(
                        "uncommitted changes in {} are not in the worktree",
                        repo.display()
                    ));
                }
                let c = worktree::create(&self.paths, repo, &self.run.cwd, &id, args.base_ref.as_deref())?;
                (c.cwd, Some(c.path), Some(c.branch), Some(c.base))
            }
            _ => (self.run.cwd.clone(), None, None, None),
        };
        let t = now();
        let mut w = WorkerState {
            id: id.clone(),
            name: args.name.unwrap_or_else(|| id.clone()),
            profiles: args.profiles,
            task: args.task,
            status: Status::Queued,
            status_detail: None,
            cwd,
            worktree,
            branch,
            base,
            session_id: uuid::Uuid::new_v4().to_string(),
            pane_id: None,
            pid: None,
            created_at: t,
            updated_at: t,
            idle_since: None,
        };
        if queue {
            self.dir.save_worker(&w)?;
        } else {
            self.launch(&mut w)?;
        }
        let mut out = Map::new();
        out.insert("id".into(), json!(w.id));
        out.insert("status".into(), status_str(w.status));
        if let Some(b) = &w.branch {
            out.insert("branch".into(), json!(b));
        }
        if let Some(p) = &w.worktree {
            out.insert("worktree".into(), json!(path_str(p)));
        }
        if let Some(msg) = warning {
            out.insert("warning".into(), json!(msg));
        }
        Ok(Value::Object(out))
    }

    pub fn status(&mut self, id: Option<&str>) -> Result<Value> {
        self.refresh()?;
        match id {
            Some(id) => Ok(summary(&self.load(id)?)),
            None => Ok(Value::Array(self.dir.workers()?.iter().map(summary).collect())),
        }
    }

    pub fn result(&mut self, id: &str) -> Result<Value> {
        self.refresh()?;
        let w = self.load(id)?;
        let parsed = std::fs::read(self.dir.result_path(id))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .filter(|v| v.get("summary").is_some_and(Value::is_string));
        let (summary_text, notes) = match parsed {
            Some(v) => (
                v["summary"].as_str().unwrap_or_default().to_string(),
                v.get("notes").and_then(Value::as_str).unwrap_or_default().to_string(),
            ),
            None => (
                self.backend_for(&w)
                    .last_output(&w, FALLBACK_LINES)
                    .unwrap_or_else(|e| format!("output unavailable: {e:#}")),
                String::new(),
            ),
        };
        let changed = match (&w.worktree, &w.base) {
            (Some(wt), Some(base)) if wt.exists() => worktree::changed_files(wt, base),
            _ => Ok(Vec::new()),
        };
        let mut out = Map::new();
        out.insert("id".into(), json!(w.id));
        out.insert("status".into(), status_str(w.status));
        out.insert("summary".into(), json!(summary_text));
        out.insert("notes".into(), json!(notes));
        out.insert("branch".into(), json!(w.branch));
        out.insert("worktree".into(), json!(w.worktree.as_deref().map(path_str)));
        out.insert("base".into(), json!(w.base));
        match changed {
            Ok(files) => {
                out.insert("changed_files".into(), json!(files));
            }
            Err(e) => {
                out.insert("changed_files".into(), json!([]));
                out.insert("changed_files_error".into(), json!(format!("{e:#}")));
            }
        }
        out.insert("session_id".into(), json!(w.session_id));
        if let Some(p) = &w.pane_id {
            out.insert("pane_id".into(), json!(p));
        }
        Ok(Value::Object(out))
    }

    pub fn send(&mut self, id: &str, message: &str) -> Result<Value> {
        self.refresh()?;
        let mut w = self.load(id)?;
        if w.status == Status::Queued {
            bail!("worker '{id}' is queued and has not started yet");
        }
        let result_path = self.dir.result_path(id);
        if result_path.exists() {
            std::fs::remove_file(&result_path)?;
        }
        let prompt = worker_prompt(message, &result_path, w.worktree.is_some());
        let spec = self.spec_for(&w);
        let sent = self.backend_for(&w).send(&mut w, &spec, &prompt);
        if let Err(e) = sent {
            if !format!("{e:#}").contains("pane closed") {
                return Err(e);
            }
            w.pane_id = None;
            self.backend_for(&w).send(&mut w, &spec, &prompt)?;
        }
        w.status = Status::Starting;
        w.status_detail = None;
        w.idle_since = None;
        w.updated_at = now();
        self.dir.save_worker(&w)?;
        Ok(json!({"id": w.id, "status": status_str(w.status)}))
    }

    pub fn cancel(&mut self, id: &str) -> Result<Value> {
        self.refresh()?;
        let mut w = self.load(id)?;
        if w.status.is_finished() {
            bail!("worker '{id}' is already {}", status_str(w.status).as_str().unwrap_or_default());
        }
        if w.status != Status::Queued {
            self.backend_for(&w).stop(&w)?;
        }
        w.status = Status::Cancelled;
        w.status_detail = None;
        w.idle_since = None;
        w.updated_at = now();
        self.dir.save_worker(&w)?;
        Ok(json!({"id": w.id, "status": "cancelled"}))
    }

    pub fn cleanup(&mut self, id: &str, remove_worktree: bool) -> Result<Value> {
        self.refresh()?;
        let mut w = self.load(id)?;
        if !w.status.is_finished() {
            bail!(
                "worker '{id}' is {}; cancel it or wait until it finishes",
                status_str(w.status).as_str().unwrap_or_default()
            );
        }
        self.backend_for(&w).close(&w)?;
        let mut out = Map::new();
        out.insert("id".into(), json!(w.id));
        let mut removed = false;
        if remove_worktree {
            if let (Some(wt), Some(branch), Some(repo)) = (&w.worktree, &w.branch, &self.run.repo_root) {
                if wt.exists() && worktree::has_uncommitted(wt)? {
                    out.insert("kept_reason".into(), json!("uncommitted changes"));
                } else {
                    worktree::remove(repo, wt, branch)?;
                    removed = true;
                }
            }
        }
        if removed {
            w.worktree = None;
            w.branch = None;
            w.updated_at = now();
            self.dir.save_worker(&w)?;
        }
        out.insert("removed_worktree".into(), json!(removed));
        Ok(Value::Object(out))
    }

    fn refresh(&mut self) -> Result<()> {
        for mut w in self.dir.workers()? {
            if w.status.is_finished() || w.status == Status::Queued {
                continue;
            }
            let before = w.clone();
            match self.backend_for(&w).observe(&w) {
                Ok(observed) => self.apply(&mut w, observed),
                Err(e) => w.status_detail = Some(format!("observe failed: {e:#}")),
            }
            if w != before {
                w.updated_at = now();
                self.dir.save_worker(&w)?;
            }
        }
        let mut active = self.active_count()?;
        for mut w in self.dir.workers()? {
            if active >= self.run.max_workers {
                break;
            }
            if w.status == Status::Queued {
                self.promote(&mut w)?;
                if w.status.is_active() {
                    active += 1;
                }
            }
        }
        Ok(())
    }

    fn apply(&self, w: &mut WorkerState, observed: Observed) {
        if observed != Observed::Idle {
            w.idle_since = None;
        }
        w.status_detail = None;
        match observed {
            Observed::Running => w.status = Status::Working,
            Observed::Blocked => {
                w.status = Status::Blocked;
                w.status_detail = Some(format!(
                    "waiting for input in pane {}",
                    w.pane_id.as_deref().unwrap_or("?")
                ));
            }
            Observed::Exited(0) => w.status = self.file_status(&w.id).unwrap_or(Status::Done),
            Observed::Exited(_) => w.status = Status::Failed,
            Observed::Gone => {
                w.status = Status::Failed;
                let in_pane = self.run.backend == BackendKind::Herdr && w.pane_id.is_some();
                let detail = if in_pane { "pane closed" } else { "process not found" };
                w.status_detail = Some(detail.into());
            }
            Observed::Idle => match self.file_status(&w.id) {
                Some(s) => {
                    w.status = s;
                    w.idle_since = None;
                }
                None => {
                    let t = now();
                    let since = *w.idle_since.get_or_insert(t);
                    w.status = if t.saturating_sub(since) >= IDLE_GRACE_SECS {
                        Status::Done
                    } else {
                        Status::Working
                    };
                }
            },
        }
    }

    fn file_status(&self, id: &str) -> Option<Status> {
        let bytes = std::fs::read(self.dir.result_path(id)).ok()?;
        file_status(&serde_json::from_slice(&bytes).ok()?)
    }

    fn promote(&mut self, w: &mut WorkerState) -> Result<()> {
        if let Err(e) = (self.prepare)(&w.profiles) {
            w.status = Status::Failed;
            w.status_detail = Some(format!("{e:#}"));
            w.updated_at = now();
            return self.dir.save_worker(w);
        }
        self.launch(w)
    }

    fn launch(&mut self, w: &mut WorkerState) -> Result<()> {
        let spec = self.spec_for(w);
        let prompt = worker_prompt(&w.task, &self.dir.result_path(&w.id), w.worktree.is_some());
        match self.backend.start(w, &spec, &prompt) {
            Ok(()) => {
                w.status = Status::Starting;
                w.status_detail = None;
            }
            Err(e) => {
                w.status = Status::Failed;
                w.status_detail = Some(format!("{e:#}"));
            }
        }
        w.updated_at = now();
        self.dir.save_worker(w)
    }

    fn taken_ids(&self) -> Result<Vec<String>> {
        let mut ids: Vec<String> = self.dir.workers()?.into_iter().map(|w| w.id).collect();
        if let Some(repo) = &self.run.repo_root {
            ids.extend(worktree::worker_branches(repo)?);
            let parent = worktree::worktree_path(&self.paths, repo, "x");
            if let Some(Ok(entries)) = parent.parent().map(std::fs::read_dir) {
                for entry in entries.flatten() {
                    ids.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        Ok(ids)
    }

    fn active_count(&self) -> Result<usize> {
        Ok(self.dir.workers()?.iter().filter(|w| w.status.is_active()).count())
    }

    fn spec_for(&self, w: &WorkerState) -> LaunchSpec {
        LaunchSpec {
            exe: self.spec_base.0.clone(),
            profiles: w.profiles.clone(),
            permission_flags: self.run.permission_flags.clone(),
            run_dir: self.spec_base.1.clone(),
        }
    }

    fn load(&self, id: &str) -> Result<WorkerState> {
        match self.dir.workers()?.into_iter().find(|w| w.id == id) {
            Some(w) => Ok(w),
            None => bail!("unknown worker id '{id}'"),
        }
    }

    fn backend_for(&mut self, w: &WorkerState) -> &mut dyn Backend {
        let rerouted = self.run.backend == BackendKind::Herdr
            && w.pane_id.is_none()
            && w.status != Status::Queued;
        if !rerouted {
            return &mut self.backend;
        }
        let fallback = &mut self.fallback;
        self.fallback_backend.get_or_insert_with(|| fallback()).as_mut()
    }
}

fn summary(w: &WorkerState) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(w.id));
    out.insert("name".into(), json!(w.name));
    out.insert("profiles".into(), json!(w.profiles));
    out.insert("status".into(), status_str(w.status));
    if let Some(d) = &w.status_detail {
        out.insert("detail".into(), json!(d));
    }
    if let Some(p) = &w.pane_id {
        out.insert("pane_id".into(), json!(p));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::backend::{FakeBackend, Observed};
    use crate::coord::state::{BackendKind, RunDir, RunState, Status};
    use crate::fs_paths::Paths;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use tempfile::TempDir;

    fn git_in(dir: &Path, args: &[&str]) {
        let out = crate::exe::command("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    struct Env {
        _tmp: TempDir,
        repo: PathBuf,
        paths: Paths,
    }

    fn env(with_repo: bool) -> Env {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        if with_repo {
            git_in(&repo, &["init"]);
            fs::write(repo.join("init.txt"), "x").unwrap();
            git_in(&repo, &["add", "."]);
            git_in(&repo, &["commit", "-m", "init"]);
        }
        let paths = Paths::from_home(tmp.path().join("home"));
        Env { _tmp: tmp, repo, paths }
    }

    #[derive(Clone, Default)]
    struct Shared {
        inner: Rc<RefCell<FakeBackend>>,
        pane_closed: bool,
        observe_fail: Option<String>,
    }

    impl Backend for Shared {
        fn start(&mut self, w: &mut WorkerState, spec: &LaunchSpec, prompt: &str) -> Result<()> {
            self.inner.borrow_mut().start(w, spec, prompt)
        }

        fn send(&mut self, w: &mut WorkerState, spec: &LaunchSpec, prompt: &str) -> Result<()> {
            if self.pane_closed {
                anyhow::bail!("pane closed: agent_not_found");
            }
            self.inner.borrow_mut().send(w, spec, prompt)
        }

        fn observe(&mut self, w: &WorkerState) -> Result<Observed> {
            if self.observe_fail.as_deref() == Some(w.id.as_str()) {
                anyhow::bail!("herdr down");
            }
            self.inner.borrow_mut().observe(w)
        }

        fn last_output(&mut self, w: &WorkerState, lines: usize) -> Result<String> {
            self.inner.borrow_mut().last_output(w, lines)
        }

        fn stop(&mut self, w: &WorkerState) -> Result<()> {
            self.inner.borrow_mut().stop(w)
        }

        fn close(&mut self, w: &WorkerState) -> Result<()> {
            self.inner.borrow_mut().close(w)
        }
    }

    fn manager<B: Backend>(e: &Env, backend: B, git: bool, max_workers: usize) -> Manager<B> {
        manager_with(e, backend, git, max_workers, BackendKind::Headless, Shared::default())
    }

    fn manager_with<B: Backend>(
        e: &Env,
        backend: B,
        git: bool,
        max_workers: usize,
        kind: BackendKind,
        fallback: Shared,
    ) -> Manager<B> {
        let run = RunState {
            run_id: "1-abcdef".into(),
            repo_root: if git { Some(e.repo.clone()) } else { None },
            cwd: e.repo.clone(),
            permission_flags: vec![],
            backend: kind,
            max_workers,
            coordinator_pane: None,
            created_at: 1,
        };
        let dir = RunDir::create(&e.paths, &run).unwrap();
        let run_dir = dir.root.clone();
        Manager {
            run,
            dir,
            backend,
            spec_base: (PathBuf::from("claude-profile"), run_dir),
            profile_exists: Box::new(|n| n == "rust" || n == "docs"),
            list: Box::new(|| vec![("rust".into(), Some("Rust work".into())), ("docs".into(), None)]),
            paths: Paths::from_home(e.paths.home.clone()),
            fallback: Box::new(move || Box::new(fallback.clone())),
            fallback_backend: None,
            prepare: Box::new(|_| Ok(())),
        }
    }

    fn args(task: &str, worktree: bool) -> SpawnArgs {
        SpawnArgs {
            profiles: vec!["rust".into()],
            task: task.into(),
            worktree,
            base_ref: None,
            name: Some(task.into()),
        }
    }

    #[test]
    fn list_profiles_returns_names_and_descriptions() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        let v = m.list_profiles().unwrap();
        assert_eq!(v[0]["name"], "rust");
        assert_eq!(v[0]["description"], "Rust work");
        assert!(v[1]["description"].is_null());
    }

    #[test]
    fn spawn_starts_and_reports_branch() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        let v = m.spawn(args("fix", true)).unwrap();
        assert_eq!(v["id"], "fix");
        assert_eq!(v["status"], "starting");
        assert_eq!(v["branch"], "cp/fix");
        assert!(Path::new(v["worktree"].as_str().unwrap()).exists());
        assert!(v.get("warning").is_none());
        assert_eq!(m.backend.started.len(), 1);
        let (id, prompt) = &m.backend.started[0];
        assert_eq!(id, "fix");
        assert!(prompt.starts_with("fix\n\nCommit your changes on the current branch before you finish. When you finish, write "));
        assert!(prompt.contains(&m.dir.result_path("fix").display().to_string()));
        let w = m.dir.load_worker("fix").unwrap();
        assert_eq!(w.status, Status::Starting);
        assert_eq!(w.cwd, PathBuf::from(v["worktree"].as_str().unwrap()));
        assert!(uuid::Uuid::parse_str(&w.session_id).is_ok());
    }

    #[test]
    fn spawn_without_worktree_uses_run_cwd() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        let v = m.spawn(args("fix", false)).unwrap();
        assert!(v.get("branch").is_none());
        let w = m.dir.load_worker("fix").unwrap();
        assert_eq!(w.cwd, e.repo);
        assert!(w.worktree.is_none());
        assert!(m.backend.started[0].1.starts_with("fix\n\nWhen you finish, write "));
    }

    #[test]
    fn spawn_unknown_profile_errors() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        let mut a = args("fix", true);
        a.profiles = vec!["rust".into(), "nope".into()];
        let err = m.spawn(a).unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(m.backend.started.is_empty());
        assert!(m.dir.workers().unwrap().is_empty());
    }

    #[test]
    fn spawn_rejects_empty_profiles() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        let mut a = args("fix", false);
        a.profiles = vec![];
        let err = m.spawn(a).unwrap_err().to_string();
        assert_eq!(err, "profiles must name at least one profile");
        assert!(m.dir.workers().unwrap().is_empty());
    }

    #[test]
    fn spawn_prepares_profiles_before_start() {
        let e = env(false);
        let seen = Rc::new(RefCell::new(Vec::<Vec<String>>::new()));
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        let log = seen.clone();
        m.prepare = Box::new(move |p| {
            log.borrow_mut().push(p.to_vec());
            Ok(())
        });
        m.spawn(args("fix", false)).unwrap();
        assert_eq!(*seen.borrow(), vec![vec!["rust".to_string()]]);
    }

    #[test]
    fn spawn_prepare_failure_errors_and_starts_nothing() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        m.prepare = Box::new(|_| anyhow::bail!("clone failed"));
        let err = format!("{:#}", m.spawn(args("fix", true)).unwrap_err());
        assert!(err.contains("clone failed"), "{err}");
        assert!(m.backend.started.is_empty());
        assert!(m.dir.workers().unwrap().is_empty());
        assert!(!worktree::branch_exists(&e.repo, "cp/fix"));
    }

    #[test]
    fn promotion_prepare_failure_marks_worker_failed() {
        let e = env(false);
        let fail = Rc::new(std::cell::Cell::new(false));
        let mut m = manager(&e, FakeBackend::default(), false, 1);
        let flag = fail.clone();
        m.prepare = Box::new(move |_| {
            if flag.get() {
                anyhow::bail!("clone failed");
            }
            Ok(())
        });
        m.spawn(args("one", false)).unwrap();
        assert_eq!(m.spawn(args("two", false)).unwrap()["status"], "queued");
        fail.set(true);
        m.backend.observed.insert("one".into(), Observed::Exited(0));
        let two = m.status(Some("two")).unwrap();
        assert_eq!(two["status"], "failed");
        assert_eq!(two["detail"], "clone failed");
        assert_eq!(m.backend.started.len(), 1);
    }

    #[test]
    fn spawn_outside_repo_with_worktree_errors_with_hint() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        let err = m.spawn(args("fix", true)).unwrap_err().to_string();
        assert_eq!(err, "not a git repository; call spawn with worktree=false");
    }

    #[test]
    fn spawn_start_failure_marks_failed() {
        let e = env(false);
        let backend = FakeBackend {
            fail_start: true,
            ..Default::default()
        };
        let mut m = manager(&e, backend, false, 4);
        let v = m.spawn(args("fix", false)).unwrap();
        assert_eq!(v["status"], "failed");
        let w = m.dir.load_worker("fix").unwrap();
        assert_eq!(w.status_detail.as_deref(), Some("start failed"));
    }

    #[test]
    fn spawn_warns_on_dirty_repo() {
        let e = env(true);
        fs::write(e.repo.join("dirty.txt"), "x").unwrap();
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        let v = m.spawn(args("fix", true)).unwrap();
        assert_eq!(
            v["warning"],
            format!("uncommitted changes in {} are not in the worktree", e.repo.display())
        );
    }

    #[test]
    fn spawn_queues_over_limit_and_promotes_when_slot_frees() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 1);
        assert_eq!(m.spawn(args("one", false)).unwrap()["status"], "starting");
        assert_eq!(m.spawn(args("two", false)).unwrap()["status"], "queued");
        assert_eq!(m.backend.started.len(), 1);
        m.backend.observed.insert("one".into(), Observed::Exited(0));
        let all = m.status(None).unwrap();
        assert_eq!(all[0]["id"], "one");
        assert_eq!(all[0]["status"], "done");
        assert_eq!(all[1]["id"], "two");
        assert_eq!(all[1]["status"], "starting");
        assert_eq!(m.backend.started.len(), 2);
        assert!(m.dir.load_worker("one").unwrap().status_detail.is_none());
    }

    #[test]
    fn exited_zero_uses_result_file_status() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        fs::write(m.dir.result_path("one"), r#"{"status":"failed","summary":"no"}"#).unwrap();
        m.backend.observed.insert("one".into(), Observed::Exited(0));
        assert_eq!(m.status(Some("one")).unwrap()["status"], "failed");
    }

    #[test]
    fn exited_nonzero_and_gone_fail() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        m.spawn(args("two", false)).unwrap();
        m.backend.observed.insert("one".into(), Observed::Exited(2));
        m.backend.observed.insert("two".into(), Observed::Gone);
        assert_eq!(m.status(Some("one")).unwrap()["status"], "failed");
        let two = m.status(Some("two")).unwrap();
        assert_eq!(two["status"], "failed");
        assert_eq!(two["detail"], "process not found");
    }

    #[test]
    fn gone_herdr_pane_reports_pane_closed() {
        let e = env(false);
        let mut m = manager_with(&e, Shared::default(), false, 4, BackendKind::Herdr, Shared::default());
        m.spawn(args("one", false)).unwrap();
        let mut w = m.dir.load_worker("one").unwrap();
        w.pane_id = Some("p2".into());
        m.dir.save_worker(&w).unwrap();
        m.backend.inner.borrow_mut().observed.insert("one".into(), Observed::Gone);
        let v = m.status(Some("one")).unwrap();
        assert_eq!(v["status"], "failed");
        assert_eq!(v["detail"], "pane closed");
    }

    #[test]
    fn blocked_reports_pane() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        let mut w = m.dir.load_worker("one").unwrap();
        w.pane_id = Some("p7".into());
        m.dir.save_worker(&w).unwrap();
        m.backend.observed.insert("one".into(), Observed::Blocked);
        let v = m.status(Some("one")).unwrap();
        assert_eq!(v["status"], "blocked");
        assert_eq!(v["detail"], "waiting for input in pane p7");
        assert_eq!(v["pane_id"], "p7");
        m.backend.observed.insert("one".into(), Observed::Running);
        let v = m.status(Some("one")).unwrap();
        assert_eq!(v["status"], "working");
        assert!(v.get("detail").is_none());
    }

    #[test]
    fn idle_without_result_waits_grace_then_done() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        m.backend.observed.insert("one".into(), Observed::Idle);
        assert_eq!(m.status(Some("one")).unwrap()["status"], "working");
        let mut w = m.dir.load_worker("one").unwrap();
        assert!(w.idle_since.is_some());
        w.idle_since = Some(now() - 31);
        m.dir.save_worker(&w).unwrap();
        assert_eq!(m.status(Some("one")).unwrap()["status"], "done");
    }

    #[test]
    fn idle_clears_on_running() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        m.backend.observed.insert("one".into(), Observed::Idle);
        m.status(None).unwrap();
        m.backend.observed.insert("one".into(), Observed::Running);
        m.status(None).unwrap();
        assert!(m.dir.load_worker("one").unwrap().idle_since.is_none());
    }

    #[test]
    fn idle_with_result_file_is_done() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        fs::write(m.dir.result_path("one"), r#"{"status":"done","summary":"ok"}"#).unwrap();
        m.backend.observed.insert("one".into(), Observed::Idle);
        assert_eq!(m.status(Some("one")).unwrap()["status"], "done");
    }

    #[test]
    fn result_uses_file_summary() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        m.spawn(args("one", true)).unwrap();
        let wt = m.dir.load_worker("one").unwrap().worktree.unwrap();
        fs::write(wt.join("new.txt"), "x").unwrap();
        fs::write(
            m.dir.result_path("one"),
            r#"{"status":"done","summary":"did it","notes":"n"}"#,
        )
        .unwrap();
        m.backend.observed.insert("one".into(), Observed::Exited(0));
        let v = m.result("one").unwrap();
        assert_eq!(v["id"], "one");
        assert_eq!(v["status"], "done");
        assert_eq!(v["summary"], "did it");
        assert_eq!(v["notes"], "n");
        assert_eq!(v["branch"], "cp/one");
        assert_eq!(v["worktree"], wt.display().to_string());
        assert_eq!(v["changed_files"], serde_json::json!(["new.txt"]));
        assert_eq!(v["base"], m.dir.load_worker("one").unwrap().base.unwrap());
        assert!(v.get("changed_files_error").is_none());
        assert!(v["session_id"].is_string());
    }

    #[test]
    fn result_reports_changed_files_error_without_failing() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        m.spawn(args("one", true)).unwrap();
        let mut w = m.dir.load_worker("one").unwrap();
        w.base = Some("no-such-ref".into());
        m.dir.save_worker(&w).unwrap();
        let v = m.result("one").unwrap();
        assert_eq!(v["changed_files"], serde_json::json!([]));
        assert!(v["changed_files_error"].as_str().unwrap().contains("no-such-ref"), "{v}");
        assert_eq!(v["base"], "no-such-ref");
    }

    #[test]
    fn send_to_worktree_worker_asks_for_commit() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        m.spawn(args("one", true)).unwrap();
        m.send("one", "more").unwrap();
        assert!(m.backend.sent[0].1.starts_with("more\n\nCommit your changes"));
    }

    #[test]
    fn result_falls_back_on_malformed_file() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        fs::write(m.dir.result_path("one"), r#"{"summary":"#).unwrap();
        m.backend.output.insert("one".into(), "last words".into());
        let v = m.result("one").unwrap();
        assert_eq!(v["summary"], "last words");
        assert_eq!(v["notes"], "");
        assert_eq!(v["changed_files"], serde_json::json!([]));
        assert!(v["branch"].is_null());
        assert!(v["base"].is_null());
    }

    #[test]
    fn send_resumes_and_clears_old_result() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        fs::write(m.dir.result_path("one"), r#"{"status":"done","summary":"ok"}"#).unwrap();
        m.backend.observed.insert("one".into(), Observed::Exited(0));
        m.status(None).unwrap();
        m.backend.observed.insert("one".into(), Observed::Running);
        let v = m.send("one", "more").unwrap();
        assert_eq!(v["status"], "starting");
        assert!(!m.dir.result_path("one").exists());
        assert!(m.backend.sent[0].1.starts_with("more\n\nWhen you finish"));
    }

    #[test]
    fn send_errors_on_queued() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 1);
        m.spawn(args("one", false)).unwrap();
        m.spawn(args("two", false)).unwrap();
        assert!(m.send("two", "x").is_err());
    }

    #[test]
    fn send_falls_back_to_headless_when_pane_closed() {
        let e = env(false);
        let primary = Shared {
            pane_closed: true,
            ..Default::default()
        };
        let fallback = Shared::default();
        let mut m = manager_with(&e, primary, false, 4, BackendKind::Herdr, fallback.clone());
        m.spawn(args("one", false)).unwrap();
        let mut w = m.dir.load_worker("one").unwrap();
        w.pane_id = Some("p2".into());
        m.dir.save_worker(&w).unwrap();
        let v = m.send("one", "again").unwrap();
        assert_eq!(v["status"], "starting");
        assert_eq!(fallback.inner.borrow().sent.len(), 1);
        assert!(m.dir.load_worker("one").unwrap().pane_id.is_none());
        fallback.inner.borrow_mut().observed.insert("one".into(), Observed::Exited(0));
        assert_eq!(m.status(Some("one")).unwrap()["status"], "done");
        m.cancel("one").unwrap_err();
        m.cleanup("one", false).unwrap();
        assert_eq!(fallback.inner.borrow().closed, ["one"]);
        assert!(m.backend.inner.borrow().closed.is_empty());
    }

    #[test]
    fn cancel_stops_worker() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        let v = m.cancel("one").unwrap();
        assert_eq!(v, serde_json::json!({"id": "one", "status": "cancelled"}));
        assert_eq!(m.backend.stopped, ["one"]);
        assert_eq!(m.dir.load_worker("one").unwrap().status, Status::Cancelled);
    }

    #[test]
    fn cleanup_keeps_dirty_worktree() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        m.spawn(args("one", true)).unwrap();
        let wt = m.dir.load_worker("one").unwrap().worktree.unwrap();
        fs::write(wt.join("new.txt"), "x").unwrap();
        m.cancel("one").unwrap();
        let v = m.cleanup("one", true).unwrap();
        assert_eq!(v["removed_worktree"], false);
        assert_eq!(v["kept_reason"], "uncommitted changes");
        assert!(wt.exists());
        assert_eq!(m.backend.closed, ["one"]);
    }

    #[test]
    fn cleanup_removes_clean_worktree() {
        let e = env(true);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        m.spawn(args("one", true)).unwrap();
        let wt = m.dir.load_worker("one").unwrap().worktree.unwrap();
        m.cancel("one").unwrap();
        let v = m.cleanup("one", true).unwrap();
        assert_eq!(v, serde_json::json!({"id": "one", "removed_worktree": true}));
        assert!(!wt.exists());
    }

    #[test]
    fn cleanup_refuses_active_worker() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        m.spawn(args("one", false)).unwrap();
        assert!(m.cleanup("one", true).is_err());
        assert!(m.backend.closed.is_empty());
    }

    #[test]
    fn unknown_id_errors() {
        let e = env(false);
        let mut m = manager(&e, FakeBackend::default(), false, 4);
        for id in ["../x", "nope"] {
            assert_eq!(
                m.status(Some(id)).unwrap_err().to_string(),
                format!("unknown worker id '{id}'")
            );
            assert!(m.result(id).is_err());
            assert!(m.send(id, "x").is_err());
            assert!(m.cancel(id).is_err());
            assert!(m.cleanup(id, true).is_err());
        }
    }

    #[test]
    fn spawn_skips_ids_of_existing_branches() {
        let e = env(true);
        git_in(&e.repo, &["branch", "cp/fix"]);
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        let v = m.spawn(args("fix", true)).unwrap();
        assert_eq!(v["id"], "fix-2");
        assert_eq!(v["branch"], "cp/fix-2");
    }

    #[test]
    fn spawn_skips_ids_of_existing_worktree_dirs() {
        let e = env(true);
        let taken = worktree::worktree_path(&e.paths, &e.repo, "fix");
        fs::create_dir_all(&taken).unwrap();
        let mut m = manager(&e, FakeBackend::default(), true, 4);
        assert_eq!(m.spawn(args("fix", true)).unwrap()["id"], "fix-2");
    }

    #[test]
    fn observe_error_is_recorded_and_others_continue() {
        let e = env(false);
        let backend = Shared {
            observe_fail: Some("one".into()),
            ..Default::default()
        };
        let mut m = manager(&e, backend, false, 4);
        m.spawn(args("one", false)).unwrap();
        m.spawn(args("two", false)).unwrap();
        m.backend.inner.borrow_mut().observed.insert("two".into(), Observed::Exited(0));
        let all = m.status(None).unwrap();
        assert_eq!(all[0]["status"], "starting");
        assert_eq!(all[0]["detail"], "observe failed: herdr down");
        assert_eq!(all[1]["status"], "done");
    }

    #[test]
    fn result_reports_output_error() {
        let e = env(false);
        let backend = FakeBackend {
            fail_output: true,
            ..Default::default()
        };
        let mut m = manager(&e, backend, false, 4);
        m.spawn(args("one", false)).unwrap();
        let v = m.result("one").unwrap();
        assert_eq!(v["summary"], "output unavailable: agent read failed");
    }
}
