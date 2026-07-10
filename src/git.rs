use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
    pub git_ref: Option<String>,
}

impl RepoRef {
    pub fn pack_dir_name(&self) -> String {
        format!("{}--{}", self.owner, self.repo)
    }
    pub fn clone_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.repo)
    }
}

pub fn parse_repo_ref(s: &str) -> anyhow::Result<RepoRef> {
    let (path, git_ref) = match s.split_once('#') {
        Some((p, r)) if !r.is_empty() => (p, Some(r.to_string())),
        Some((_, _)) => anyhow::bail!("empty ref after '#' in '{s}'"),
        None => (s, None),
    };
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        anyhow::bail!("expected owner/repo[#ref], got '{s}'");
    }
    Ok(RepoRef { owner: parts[0].to_string(), repo: parts[1].to_string(), git_ref })
}

pub trait GitCli {
    fn clone(&self, url: &str, dest: &Path) -> anyhow::Result<()>;
    fn pull(&self, repo: &Path) -> anyhow::Result<()>;
    fn head_sha(&self, repo: &Path) -> anyhow::Result<String>;
    fn checkout(&self, repo: &Path, git_ref: &str) -> anyhow::Result<()>;
}

pub struct RealGit;

impl RealGit {
    fn run(&self, args: &[&str], cwd: Option<&Path>) -> anyhow::Result<std::process::Output> {
        let mut cmd = Command::new("git");
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let out = cmd.output().map_err(|e| anyhow::anyhow!("failed to run git {}: {e}", args.join(" ")))?;
        if !out.status.success() {
            anyhow::bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
        }
        Ok(out)
    }
}

impl GitCli for RealGit {
    fn clone(&self, url: &str, dest: &Path) -> anyhow::Result<()> {
        self.run(&["clone", url, &dest.to_string_lossy()], None).map(|_| ())
    }
    fn pull(&self, repo: &Path) -> anyhow::Result<()> {
        self.run(&["pull", "--ff-only"], Some(repo)).map(|_| ())
    }
    fn head_sha(&self, repo: &Path) -> anyhow::Result<String> {
        let out = self.run(&["rev-parse", "HEAD"], Some(repo))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
    fn checkout(&self, repo: &Path, git_ref: &str) -> anyhow::Result<()> {
        self.run(&["checkout", git_ref], Some(repo)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_repo_without_ref() {
        let r = parse_repo_ref("fuzzyalej/rust-profile").unwrap();
        assert_eq!(r.owner, "fuzzyalej");
        assert_eq!(r.repo, "rust-profile");
        assert_eq!(r.git_ref, None);
        assert_eq!(r.pack_dir_name(), "fuzzyalej--rust-profile");
        assert_eq!(r.clone_url(), "https://github.com/fuzzyalej/rust-profile.git");
    }

    #[test]
    fn parses_owner_repo_with_ref() {
        let r = parse_repo_ref("obra/superpowers-marketplace#v1.2.3").unwrap();
        assert_eq!(r.repo, "superpowers-marketplace");
        assert_eq!(r.git_ref.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_repo_ref("noslash").is_err());
        assert!(parse_repo_ref("a/b/c").is_err());
        assert!(parse_repo_ref("/empty").is_err());
    }
}
