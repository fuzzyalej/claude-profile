use crate::fs_paths::Paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Starting,
    Working,
    Blocked,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn is_active(self) -> bool {
        matches!(self, Status::Starting | Status::Working | Status::Blocked)
    }

    pub fn is_finished(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Herdr,
    Headless,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct RunState {
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_root: Option<PathBuf>,
    pub cwd: PathBuf,
    pub permission_flags: Vec<String>,
    pub backend: BackendKind,
    pub max_workers: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub coordinator_pane: Option<String>,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct WorkerState {
    pub id: String,
    pub name: String,
    pub profiles: Vec<String>,
    pub task: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub status_detail: Option<String>,
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worktree: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub base: Option<String>,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pid: Option<u32>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub idle_since: Option<u64>,
}

pub struct RunDir {
    pub root: PathBuf,
}

impl RunDir {
    pub fn create(paths: &Paths, run: &RunState) -> Result<RunDir> {
        let root = paths.runs_dir().join(&run.run_id);
        std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        let dir = RunDir { root };
        write_atomic(&dir.root.join("run.json"), &serde_json::to_vec_pretty(run)?)?;
        Ok(dir)
    }

    pub fn open(paths: &Paths, run_id: &str) -> Result<RunDir> {
        let root = paths.runs_dir().join(run_id);
        if !root.join("run.json").is_file() {
            anyhow::bail!("run '{run_id}' not found");
        }
        Ok(RunDir { root })
    }

    pub fn load_run(&self) -> Result<RunState> {
        let path = self.root.join("run.json");
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn worker_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn save_worker(&self, w: &WorkerState) -> Result<()> {
        let dir = self.worker_dir(&w.id);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        write_atomic(&dir.join("worker.json"), &serde_json::to_vec_pretty(w)?)
    }

    pub fn load_worker(&self, id: &str) -> Result<WorkerState> {
        let path = self.worker_dir(id).join("worker.json");
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn workers(&self) -> Result<Vec<WorkerState>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root).with_context(|| format!("reading {}", self.root.display()))? {
            let entry = entry?;
            if entry.path().join("worker.json").is_file() {
                out.push(self.load_worker(&entry.file_name().to_string_lossy())?);
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    pub fn result_path(&self, id: &str) -> PathBuf {
        self.worker_dir(id).join("result.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join("server.lock")
    }

    pub fn hold_lock(&self) -> Result<std::fs::File> {
        let path = self.lock_path();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.try_lock()
            .map_err(|e| anyhow::anyhow!("locking {}: {e}", path.display()))?;
        Ok(file)
    }

    pub fn is_live(&self) -> bool {
        let Ok(file) = std::fs::OpenOptions::new().write(true).open(self.lock_path()) else {
            return false;
        };
        matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
    }
}

pub fn new_run_id(now: u64) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{now}-{}", &hex[..6])
}

pub fn new_worker_id(name: &str, existing: &[String]) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if c == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(c);
    }
    let slug = slug.trim_matches('-');
    let slug = &slug[..slug.len().min(32)];
    let base = slug.trim_matches('-');
    let base = if base.is_empty() { "worker" } else { base };
    if !existing.iter().any(|e| e == base) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !existing.iter().any(|e| e == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("writing {}: {}", path.display(), e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_lock_marks_run_live_while_held() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let dir = RunDir::create(&paths, &run()).unwrap();
        assert!(!dir.is_live());
        let lock = dir.hold_lock().unwrap();
        assert!(dir.is_live());
        drop(lock);
        assert!(!dir.is_live());
        assert!(dir.workers().unwrap().is_empty());
    }

    fn run() -> RunState {
        RunState {
            run_id: "100-abcdef".into(),
            repo_root: Some(PathBuf::from("/repo")),
            cwd: PathBuf::from("/repo"),
            permission_flags: vec!["--permission-mode".into(), "acceptEdits".into()],
            backend: BackendKind::Headless,
            max_workers: 4,
            coordinator_pane: None,
            created_at: 100,
        }
    }

    fn worker(id: &str, created_at: u64) -> WorkerState {
        WorkerState {
            id: id.into(),
            name: id.into(),
            profiles: vec!["rust".into()],
            task: "do it".into(),
            status: Status::Working,
            status_detail: None,
            cwd: PathBuf::from("/repo"),
            worktree: Some(PathBuf::from("/wt")),
            branch: Some("b".into()),
            base: None,
            session_id: "sid".into(),
            pane_id: None,
            pid: Some(42),
            created_at,
            updated_at: created_at,
            idle_since: None,
        }
    }

    #[test]
    fn run_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path().to_path_buf());
        let r = run();
        RunDir::create(&paths, &r).unwrap();
        let dir = RunDir::open(&paths, &r.run_id).unwrap();
        assert_eq!(dir.load_run().unwrap(), r);
        assert!(RunDir::open(&paths, "nope").is_err());
    }

    #[test]
    fn worker_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path().to_path_buf());
        let dir = RunDir::create(&paths, &run()).unwrap();
        let w = worker("a", 1);
        dir.save_worker(&w).unwrap();
        assert_eq!(dir.load_worker("a").unwrap(), w);
        let json = std::fs::read_to_string(dir.worker_dir("a").join("worker.json")).unwrap();
        assert!(!json.contains("pane_id"));
        assert_eq!(dir.result_path("a"), dir.worker_dir("a").join("result.json"));
    }

    #[test]
    fn workers_sorted_by_created_at() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path().to_path_buf());
        let dir = RunDir::create(&paths, &run()).unwrap();
        dir.save_worker(&worker("late", 30)).unwrap();
        dir.save_worker(&worker("early", 10)).unwrap();
        dir.save_worker(&worker("mid", 20)).unwrap();
        let ids: Vec<String> = dir.workers().unwrap().into_iter().map(|w| w.id).collect();
        assert_eq!(ids, ["early", "mid", "late"]);
    }

    #[test]
    fn status_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Status::Blocked).unwrap(), "\"blocked\"");
        assert!(Status::Working.is_active());
        assert!(!Status::Queued.is_active());
        assert!(Status::Cancelled.is_finished());
        assert!(!Status::Blocked.is_finished());
    }

    #[test]
    fn worker_id_slugs_and_dedupes() {
        assert_eq!(new_worker_id("Fix Auth/Login!", &[]), "fix-auth-login");
        assert_eq!(new_worker_id("fix", &["fix".into()]), "fix-2");
        assert_eq!(new_worker_id("fix", &["fix".into(), "fix-2".into()]), "fix-3");
        assert_eq!(new_worker_id("", &[]), "worker");
        assert_eq!(new_worker_id("ñandú", &[]), "and");
        assert_eq!(new_worker_id(&"a".repeat(50), &[]).len(), 32);
    }

    #[test]
    fn run_id_format() {
        let id = new_run_id(123);
        let (ts, hex) = id.split_once('-').unwrap();
        assert_eq!(ts, "123");
        assert_eq!(hex.len(), 6);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn write_atomic_leaves_no_temp_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }
}
