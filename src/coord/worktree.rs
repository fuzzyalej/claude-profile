use crate::fs_paths::Paths;
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Created {
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
    pub cwd: PathBuf,
}

fn git(args: &[&str], cwd: &Path) -> Result<String> {
    let out = crate::exe::command("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    let out = git(&["rev-parse", "--show-toplevel"], cwd).ok()?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

pub fn is_dirty(repo: &Path) -> Result<bool> {
    has_uncommitted(repo)
}

pub fn has_uncommitted(worktree: &Path) -> Result<bool> {
    Ok(!git(&["status", "--porcelain"], worktree)?.trim().is_empty())
}

#[allow(dead_code)]
fn resolve(repo: &Path, rev: &str) -> Result<String> {
    let spec = format!("{rev}^{{commit}}");
    Ok(git(&["rev-parse", "--verify", &spec], repo)?.trim().to_string())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn worktree_path(paths: &Paths, repo: &Path, id: &str) -> PathBuf {
    let canonical = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let hash = format!("{:016x}", fnv1a(canonical.to_string_lossy().as_bytes()));
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    paths.worktrees_dir().join(format!("{name}-{}", &hash[..8])).join(id)
}

pub fn create(
    paths: &Paths,
    repo: &Path,
    coordinator_cwd: &Path,
    id: &str,
    base_ref: Option<&str>,
) -> Result<Created> {
    let base = resolve(repo, base_ref.unwrap_or("HEAD"))?;
    let path = worktree_path(paths, repo, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let branch = format!("cp/{id}");
    let path_str = path.to_string_lossy().into_owned();
    git(&["worktree", "add", "-b", &branch, &path_str, &base], repo)?;
    let canonical_repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let canonical_cwd = coordinator_cwd.canonicalize().unwrap_or_else(|_| coordinator_cwd.to_path_buf());
    let cwd = match canonical_cwd.strip_prefix(&canonical_repo) {
        Ok(rel) => path.join(rel),
        Err(_) => path.clone(),
    };
    Ok(Created { path, branch, base, cwd })
}

pub fn changed_files(worktree: &Path, base: &str) -> Result<Vec<String>> {
    let range = format!("{base}...HEAD");
    let mut files = BTreeSet::new();
    for line in git(&["diff", "--name-only", &range], worktree)?.lines() {
        if !line.is_empty() {
            files.insert(line.to_string());
        }
    }
    for line in git(&["status", "--porcelain", "--untracked-files=all"], worktree)?.lines() {
        let Some(rest) = line.get(3..) else { continue };
        let name = rest.rsplit(" -> ").next().unwrap_or(rest);
        files.insert(name.trim_matches('"').to_string());
    }
    Ok(files.into_iter().collect())
}

pub fn worker_branches(repo: &Path) -> Result<Vec<String>> {
    let out = git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/cp/"], repo)?;
    Ok(out
        .lines()
        .filter_map(|l| l.trim().strip_prefix("cp/"))
        .map(str::to_string)
        .collect())
}

pub fn remove_worktree(repo: &Path, worktree: &Path) -> Result<()> {
    let path_str = worktree.to_string_lossy().into_owned();
    git(&["worktree", "remove", "--force", &path_str], repo)?;
    Ok(())
}

pub fn remove(repo: &Path, worktree: &Path, branch: &str) -> Result<()> {
    remove_worktree(repo, worktree)?;
    git(&["branch", "-D", branch], repo)?;
    Ok(())
}

pub fn prune(repo: &Path) -> Result<()> {
    git(&["worktree", "prune"], repo)?;
    Ok(())
}

pub fn is_merged(repo: &Path, branch: &str) -> Result<bool> {
    let out = crate::exe::command("git")
        .args(["merge-base", "--is-ancestor", branch, "HEAD"])
        .current_dir(repo)
        .output()
        .with_context(|| "running git merge-base")?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(anyhow!(
            "git merge-base failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

pub fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    git(&["branch", "-D", branch], repo)?;
    Ok(())
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    let name = format!("refs/heads/{branch}");
    git(&["show-ref", "--verify", "--quiet", &name], repo).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = crate::exe::command("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_file(repo: &Path, name: &str) -> String {
        if let Some(parent) = Path::new(name).parent() {
            fs::create_dir_all(repo.join(parent)).unwrap();
        }
        fs::write(repo.join(name), name).unwrap();
        git_in(repo, &["add", "."]);
        git_in(repo, &["commit", "-m", name]);
        git_in(repo, &["rev-parse", "HEAD"])
    }

    fn setup() -> (TempDir, PathBuf, Paths) {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init"]);
        commit_file(&repo, "init.txt");
        let paths = Paths::from_home(tmp.path().join("home"));
        (tmp, repo, paths)
    }

    #[test]
    fn creates_branch_and_worktree() {
        let (_t, repo, paths) = setup();
        let c = create(&paths, &repo, &repo, "w1", None).unwrap();
        assert!(c.path.exists());
        assert_eq!(c.branch, "cp/w1");
        assert!(!git_in(&repo, &["branch", "--list", "cp/w1"]).is_empty());
        assert_eq!(c.base, resolve(&repo, "HEAD").unwrap());
    }

    #[test]
    fn cwd_preserves_subdirectory() {
        let (_t, repo, paths) = setup();
        let sub = repo.join("src/app");
        fs::create_dir_all(&sub).unwrap();
        let c = create(&paths, &repo, &sub, "w1", None).unwrap();
        assert_eq!(c.cwd, c.path.join("src/app"));
    }

    #[test]
    fn base_ref_is_resolved() {
        let (_t, repo, paths) = setup();
        let a = resolve(&repo, "HEAD").unwrap();
        commit_file(&repo, "b.txt");
        let c = create(&paths, &repo, &repo, "w1", Some("HEAD~1")).unwrap();
        assert_eq!(c.base, a);
    }

    #[test]
    fn changed_files_includes_committed_and_uncommitted() {
        let (_t, repo, paths) = setup();
        let c = create(&paths, &repo, &repo, "w1", None).unwrap();
        commit_file(&c.path, "a.txt");
        fs::write(c.path.join("b.txt"), "b").unwrap();
        assert_eq!(changed_files(&c.path, &c.base).unwrap(), vec!["a.txt", "b.txt"]);
        assert!(has_uncommitted(&c.path).unwrap());
    }

    #[test]
    fn remove_deletes_worktree_and_branch() {
        let (_t, repo, paths) = setup();
        let c = create(&paths, &repo, &repo, "w1", None).unwrap();
        remove(&repo, &c.path, &c.branch).unwrap();
        assert!(!c.path.exists());
        assert!(git_in(&repo, &["branch", "--list", "cp/w1"]).is_empty());
    }

    #[test]
    fn is_merged_reflects_commits_beyond_head() {
        let (_t, repo, paths) = setup();
        let c = create(&paths, &repo, &repo, "w1", None).unwrap();
        assert!(is_merged(&repo, &c.branch).unwrap());
        commit_file(&c.path, "extra.txt");
        assert!(!is_merged(&repo, &c.branch).unwrap());
    }

    #[test]
    fn worker_branches_lists_cp_ids() {
        let (_t, repo, paths) = setup();
        git_in(&repo, &["branch", "cp/a"]);
        git_in(&repo, &["branch", "other"]);
        create(&paths, &repo, &repo, "b", None).unwrap();
        assert_eq!(worker_branches(&repo).unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn repo_root_none_outside_git() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(repo_root(tmp.path()), None);
    }

    #[test]
    fn repo_root_finds_toplevel() {
        let (_t, repo, _p) = setup();
        let root = repo_root(&repo).unwrap();
        assert_eq!(root.canonicalize().unwrap(), repo.canonicalize().unwrap());
    }

    #[test]
    fn is_dirty_detects_untracked() {
        let (_t, repo, _p) = setup();
        assert!(!is_dirty(&repo).unwrap());
        fs::write(repo.join("new.txt"), "x").unwrap();
        assert!(is_dirty(&repo).unwrap());
    }

    #[test]
    fn worktree_path_is_stable_and_under_worktrees_dir() {
        let (_t, repo, paths) = setup();
        let p = worktree_path(&paths, &repo, "w1");
        assert_eq!(p, worktree_path(&paths, &repo, "w1"));
        assert!(p.starts_with(paths.worktrees_dir()));
        assert_eq!(p.file_name().unwrap(), "w1");
        let dir = p.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let suffix = dir.strip_prefix("repo-").unwrap();
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
