use crate::coord::state::{RunDir, RunState, Status, WorkerState};
use crate::coord::worktree;
use crate::fs_paths::Paths;
use anyhow::{bail, Result};
use std::fmt;
use std::path::Path;

#[derive(Debug, PartialEq)]
pub enum Kept {
    Uncommitted(String),
    Unmerged(String),
    InUse(String),
}

impl fmt::Display for Kept {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kept::Uncommitted(p) => write!(f, "kept (uncommitted changes): {p}"),
            Kept::Unmerged(b) => write!(f, "kept (unmerged branch): {b}"),
            Kept::InUse(id) => write!(f, "skipped (in use): {id}"),
        }
    }
}

const ORDER: [(Status, &str); 7] = [
    (Status::Queued, "queued"),
    (Status::Starting, "starting"),
    (Status::Working, "working"),
    (Status::Blocked, "blocked"),
    (Status::Done, "done"),
    (Status::Failed, "failed"),
    (Status::Cancelled, "cancelled"),
];

pub fn run(paths: &Paths, clean_runs: bool, run_id: Option<&str>) -> Result<i32> {
    if clean_runs {
        for item in clean(paths, run_id)? {
            println!("{item}");
        }
        return Ok(0);
    }
    if run_id.is_some() {
        bail!("a run id needs --clean");
    }
    let mut runs = Vec::new();
    for dir in list_dirs(paths) {
        let loaded = dir.load_run().and_then(|r| Ok((r, dir.workers()?, dir.is_live())));
        match loaded {
            Ok(pair) => runs.push(pair),
            Err(e) => eprintln!("warning: skipping {}: {e}", dir.root.display()),
        }
    }
    println!("{}", format_runs(&runs));
    Ok(0)
}

fn list_dirs(paths: &Paths) -> Vec<RunDir> {
    let Ok(entries) = std::fs::read_dir(paths.runs_dir()) else {
        return Vec::new();
    };
    let mut dirs: Vec<RunDir> = entries
        .flatten()
        .filter(|e| e.path().join("run.json").is_file())
        .map(|e| RunDir { root: e.path() })
        .collect();
    dirs.sort_by(|a, b| a.root.cmp(&b.root));
    dirs
}

fn counts(workers: &[WorkerState]) -> String {
    if workers.is_empty() {
        return "0 workers".to_string();
    }
    let parts: Vec<String> = ORDER
        .iter()
        .map(|(s, name)| (name, workers.iter().filter(|w| w.status == *s).count()))
        .filter(|(_, n)| *n > 0)
        .map(|(name, n)| format!("{name}: {n}"))
        .collect();
    format!("{} workers ({})", workers.len(), parts.join(", "))
}

pub fn format_runs(runs: &[(RunState, Vec<WorkerState>, bool)]) -> String {
    if runs.is_empty() {
        return "No runs.".to_string();
    }
    let mut sorted: Vec<&(RunState, Vec<WorkerState>, bool)> = runs.iter().collect();
    sorted.sort_by(|a, b| b.0.created_at.cmp(&a.0.created_at).then_with(|| b.0.run_id.cmp(&a.0.run_id)));
    sorted
        .iter()
        .map(|(r, w, live)| {
            let active = if *live { " (active)" } else { "" };
            format!("{}  {}  {}{active}", r.run_id, r.cwd.display(), counts(w))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn drop_branch(repo: &Path, branch: &str, kept: &mut Vec<Kept>) -> Result<bool> {
    if !worktree::branch_exists(repo, branch) {
        return Ok(true);
    }
    if worktree::is_merged(repo, branch)? {
        worktree::delete_branch(repo, branch)?;
        Ok(true)
    } else {
        kept.push(Kept::Unmerged(branch.to_string()));
        Ok(false)
    }
}

fn clean_run(dir: &RunDir, run: &RunState, kept: &mut Vec<Kept>) -> Result<()> {
    let workers = match dir.workers() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("warning: skipping {}: {e}", dir.root.display());
            return Ok(());
        }
    };
    let mut run_kept = false;
    for mut w in workers {
        let Some(repo) = run.repo_root.as_deref() else {
            continue;
        };
        let (Some(wt), Some(branch)) = (w.worktree.clone(), w.branch.clone()) else {
            continue;
        };
        if wt.exists() {
            if worktree::has_uncommitted(&wt)? {
                kept.push(Kept::Uncommitted(wt.display().to_string()));
                run_kept = true;
                continue;
            }
            worktree::remove_worktree(repo, &wt)?;
        }
        worktree::prune(repo)?;
        w.worktree = None;
        if drop_branch(repo, &branch, kept)? {
            w.branch = None;
        } else {
            run_kept = true;
        }
        dir.save_worker(&w)?;
    }
    if !run_kept {
        std::fs::remove_dir_all(&dir.root)?;
    }
    Ok(())
}

pub fn clean(paths: &Paths, run_id: Option<&str>) -> Result<Vec<Kept>> {
    let dirs = match run_id {
        Some(id) => match RunDir::open(paths, id) {
            Ok(d) => vec![d],
            Err(_) => bail!("unknown run '{id}'"),
        },
        None => list_dirs(paths),
    };
    let mut kept = Vec::new();
    for dir in dirs {
        match dir.load_run() {
            Ok(run) if dir.is_live() => kept.push(Kept::InUse(run.run_id)),
            Ok(run) => clean_run(&dir, &run, &mut kept)?,
            Err(e) => eprintln!("warning: skipping {}: {e}", dir.root.display()),
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::state::BackendKind;
    use std::path::{Path, PathBuf};

    fn git_in(dir: &Path, args: &[&str]) {
        let out = crate::exe::command("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    fn run_state(id: &str, repo: &Path, created_at: u64) -> RunState {
        RunState {
            run_id: id.into(),
            repo_root: Some(repo.to_path_buf()),
            cwd: repo.to_path_buf(),
            permission_flags: vec![],
            backend: BackendKind::Headless,
            max_workers: 4,
            coordinator_pane: None,
            created_at,
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
        }
    }

    #[test]
    fn format_runs_counts_statuses() {
        let r = run_state("100-abc", Path::new("/repo"), 100);
        let ws = vec![worker("a", Status::Done), worker("b", Status::Working), worker("c", Status::Done)];
        let out = format_runs(&[(r.clone(), ws, true), (run_state("200-def", Path::new("/other"), 200), vec![], false)]);
        assert_eq!(
            out,
            "200-def  /other  0 workers\n100-abc  /repo  3 workers (working: 1, done: 2) (active)"
        );
    }

    #[test]
    fn format_runs_empty() {
        assert_eq!(format_runs(&[]), "No runs.");
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        paths: Paths,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-m", "init"]);
        let paths = Paths::from_home(tmp.path().join("home"));
        Fixture { _tmp: tmp, repo, paths }
    }

    fn add_worker(f: &Fixture, dir: &RunDir, id: &str, status: Status) -> WorkerState {
        let c = worktree::create(&f.paths, &f.repo, &f.repo, id, None).unwrap();
        let mut w = worker(id, status);
        w.worktree = Some(c.path);
        w.branch = Some(c.branch);
        dir.save_worker(&w).unwrap();
        w
    }

    #[test]
    fn clean_removes_finished_and_keeps_dirty() {
        let f = fixture();
        let dir = RunDir::create(&f.paths, &run_state("1-aaa", &f.repo, 1)).unwrap();
        let clean_w = add_worker(&f, &dir, "clean", Status::Done);
        let dirty_w = add_worker(&f, &dir, "dirty", Status::Failed);
        let dirty_path = dirty_w.worktree.clone().unwrap();
        std::fs::write(dirty_path.join("new.txt"), "x").unwrap();
        let kept = clean(&f.paths, None).unwrap();
        assert_eq!(kept, vec![Kept::Uncommitted(dirty_path.display().to_string())]);
        assert!(!clean_w.worktree.unwrap().exists());
        assert!(!worktree::branch_exists(&f.repo, "cp/clean"));
        assert!(dirty_path.exists());
        assert!(dir.root.exists());
        let reloaded = dir.load_worker("clean").unwrap();
        assert_eq!(reloaded.worktree, None);
        assert_eq!(reloaded.branch, None);
        assert!(dir.load_worker("dirty").unwrap().worktree.is_some());
    }

    #[test]
    fn clean_removes_run_dir_when_all_finished() {
        let f = fixture();
        let dir = RunDir::create(&f.paths, &run_state("1-aaa", &f.repo, 1)).unwrap();
        add_worker(&f, &dir, "w", Status::Done);
        assert!(clean(&f.paths, Some("1-aaa")).unwrap().is_empty());
        assert!(!dir.root.exists());
        assert!(!worktree::branch_exists(&f.repo, "cp/w"));
    }

    #[test]
    fn clean_skips_live_run() {
        let f = fixture();
        let dir = RunDir::create(&f.paths, &run_state("1-aaa", &f.repo, 1)).unwrap();
        let active = add_worker(&f, &dir, "act", Status::Working);
        let fin = add_worker(&f, &dir, "fin", Status::Done);
        let _lock = dir.hold_lock().unwrap();
        let kept = clean(&f.paths, None).unwrap();
        assert_eq!(kept, vec![Kept::InUse("1-aaa".into())]);
        assert_eq!(kept[0].to_string(), "skipped (in use): 1-aaa");
        assert!(active.worktree.unwrap().exists());
        assert!(fin.worktree.unwrap().exists());
        assert!(worktree::branch_exists(&f.repo, "cp/fin"));
        assert!(dir.root.exists());
    }

    #[test]
    fn clean_treats_unfinished_workers_of_dead_run_as_finished() {
        let f = fixture();
        let dir = RunDir::create(&f.paths, &run_state("1-aaa", &f.repo, 1)).unwrap();
        let active = add_worker(&f, &dir, "act", Status::Working);
        add_worker(&f, &dir, "fin", Status::Done);
        assert!(clean(&f.paths, None).unwrap().is_empty());
        assert!(!active.worktree.unwrap().exists());
        assert!(!worktree::branch_exists(&f.repo, "cp/act"));
        assert!(!worktree::branch_exists(&f.repo, "cp/fin"));
        assert!(!dir.root.exists());
    }

    #[test]
    fn clean_deletes_branch_when_worktree_missing() {
        let f = fixture();
        let dir = RunDir::create(&f.paths, &run_state("1-aaa", &f.repo, 1)).unwrap();
        let w = add_worker(&f, &dir, "gone", Status::Done);
        std::fs::remove_dir_all(w.worktree.unwrap()).unwrap();
        clean(&f.paths, None).unwrap();
        assert!(!worktree::branch_exists(&f.repo, "cp/gone"));
    }

    #[test]
    fn clean_keeps_unmerged_branch() {
        let f = fixture();
        let dir = RunDir::create(&f.paths, &run_state("1-aaa", &f.repo, 1)).unwrap();
        let w = add_worker(&f, &dir, "ahead", Status::Done);
        let wt = w.worktree.unwrap();
        std::fs::write(wt.join("x.txt"), "x").unwrap();
        git_in(&wt, &["add", "."]);
        git_in(&wt, &["commit", "-m", "x"]);
        let kept = clean(&f.paths, None).unwrap();
        assert_eq!(kept, vec![Kept::Unmerged("cp/ahead".into())]);
        assert_eq!(kept[0].to_string(), "kept (unmerged branch): cp/ahead");
        assert!(!wt.exists());
        assert!(worktree::branch_exists(&f.repo, "cp/ahead"));
        assert!(dir.root.exists());
        let reloaded = dir.load_worker("ahead").unwrap();
        assert_eq!(reloaded.worktree, None);
        assert_eq!(reloaded.branch.as_deref(), Some("cp/ahead"));
    }

    #[test]
    fn clean_skips_run_with_corrupt_worker() {
        let f = fixture();
        let bad = RunDir::create(&f.paths, &run_state("1-bad", &f.repo, 1)).unwrap();
        add_worker(&f, &bad, "w", Status::Done);
        std::fs::write(bad.worker_dir("w").join("worker.json"), "{").unwrap();
        let good = RunDir::create(&f.paths, &run_state("2-good", &f.repo, 2)).unwrap();
        add_worker(&f, &good, "g", Status::Done);
        assert!(clean(&f.paths, None).unwrap().is_empty());
        assert!(bad.root.exists());
        assert!(!good.root.exists());
    }

    #[test]
    fn clean_unknown_run_errors() {
        let f = fixture();
        let err = clean(&f.paths, Some("nope")).unwrap_err();
        assert_eq!(err.to_string(), "unknown run 'nope'");
    }
}
