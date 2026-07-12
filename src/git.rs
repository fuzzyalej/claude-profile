use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
    pub git_ref: Option<String>,
    /// Explicit clone URL for ssh/https sources; `None` for `owner/repo` shorthand
    /// (which clones from github.com).
    pub url: Option<String>,
}

impl RepoRef {
    pub fn pack_dir_name(&self) -> String {
        format!("{}--{}", self.owner, self.repo)
    }
    pub fn clone_url(&self) -> String {
        self.url
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{}/{}.git", self.owner, self.repo))
    }
}

/// Parse a profile-repo source into a `RepoRef`. Accepts three forms, each with an
/// optional trailing `#ref`:
/// - `owner/repo` shorthand (cloned from github.com)
/// - `https://host/owner/repo(.git)` URL
/// - `git@host:path/owner/repo(.git)` SSH URL
pub fn parse_repo_ref(s: &str) -> anyhow::Result<RepoRef> {
    let (path, git_ref) = match s.split_once('#') {
        Some((p, r)) if !r.is_empty() => (p, Some(r.to_string())),
        Some((_, _)) => anyhow::bail!("empty ref after '#' in '{s}'"),
        None => (s, None),
    };

    if path.contains("://") || path.starts_with("git@") {
        let (owner, repo) = owner_repo_from_url(path)
            .ok_or_else(|| anyhow::anyhow!("could not parse owner/repo from URL '{s}'"))?;
        return Ok(RepoRef { owner, repo, git_ref, url: Some(path.to_string()) });
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        anyhow::bail!("expected owner/repo[#ref], https://… or git@…, got '{s}'");
    }
    Ok(RepoRef { owner: parts[0].to_string(), repo: parts[1].to_string(), git_ref, url: None })
}

/// Extract the trailing `owner`, `repo` segments from an ssh or https git URL.
fn owner_repo_from_url(url: &str) -> Option<(String, String)> {
    // Reduce to the path after the host: for scp-style ssh take everything after the
    // first ':'; for https take everything after "://host/".
    let path = if let Some(rest) = url.strip_prefix("git@") {
        rest.split_once(':').map(|(_host, p)| p)?
    } else {
        let after_scheme = url.split_once("://").map(|(_s, r)| r)?;
        after_scheme.split_once('/').map(|(_host, p)| p)?
    };
    let mut segs: Vec<&str> = path.trim_end_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let repo = segs.pop()?.trim_end_matches(".git");
    let owner = segs.pop()?;
    if repo.is_empty() || owner.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

pub trait GitCli {
    fn clone(&self, url: &str, dest: &Path) -> anyhow::Result<()>;
    fn pull(&self, repo: &Path) -> anyhow::Result<()>;
    fn head_sha(&self, repo: &Path) -> anyhow::Result<String>;
    fn checkout(&self, repo: &Path, git_ref: &str) -> anyhow::Result<()>;
    /// Whether `repo` is a git checkout. Some marketplaces (e.g. the official
    /// `anthropics/claude-plugins-official`) are installed without a `.git`, so
    /// SHA pinning must be skipped rather than failing on `rev-parse`.
    fn is_repo(&self, repo: &Path) -> bool;
    /// Shallow, blobless, sparse clone of only `subpath` from `url` into `dest`.
    /// Used to fetch a single `.claude-plugin/marketplace.json` cheaply.
    fn sparse_fetch(&self, url: &str, dest: &Path, subpath: &str) -> anyhow::Result<()>;
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
    fn is_repo(&self, repo: &Path) -> bool {
        // A `.git` entry is a dir for normal clones and a file for worktrees/submodules.
        repo.join(".git").exists()
    }
    fn sparse_fetch(&self, url: &str, dest: &Path, subpath: &str) -> anyhow::Result<()> {
        let dest_s = dest.to_string_lossy();
        self.run(
            &["clone", "--depth", "1", "--filter=blob:none", "--sparse", url, &dest_s],
            None,
        )?;
        self.run(&["-C", &dest_s, "sparse-checkout", "set", subpath], None)?;
        Ok(())
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

    #[test]
    fn parses_https_url() {
        let r = parse_repo_ref("https://github.com/fuzzyalej/diagon-alley.git").unwrap();
        assert_eq!(r.owner, "fuzzyalej");
        assert_eq!(r.repo, "diagon-alley");
        assert_eq!(r.pack_dir_name(), "fuzzyalej--diagon-alley");
        assert_eq!(r.clone_url(), "https://github.com/fuzzyalej/diagon-alley.git");
        assert_eq!(r.git_ref, None);
    }

    #[test]
    fn parses_https_url_with_ref() {
        let r = parse_repo_ref("https://gitlab.com/acme/tools#v2").unwrap();
        assert_eq!(r.owner, "acme");
        assert_eq!(r.repo, "tools");
        assert_eq!(r.git_ref.as_deref(), Some("v2"));
        assert_eq!(r.clone_url(), "https://gitlab.com/acme/tools"); // ref stripped from url
    }

    #[test]
    fn parses_ssh_scp_url_with_nested_path() {
        let r = parse_repo_ref("git@ssh.dev.azure.com:v3/MjolnerDEV/MIA-Tools/mia-marketplace").unwrap();
        assert_eq!(r.owner, "MIA-Tools");
        assert_eq!(r.repo, "mia-marketplace");
        assert_eq!(r.pack_dir_name(), "MIA-Tools--mia-marketplace");
        assert_eq!(r.clone_url(), "git@ssh.dev.azure.com:v3/MjolnerDEV/MIA-Tools/mia-marketplace");
    }

    #[test]
    fn owner_repo_shorthand_has_no_explicit_url() {
        let r = parse_repo_ref("o/r").unwrap();
        assert_eq!(r.url, None);
        assert_eq!(r.clone_url(), "https://github.com/o/r.git");
    }

    #[test]
    fn sparse_fetch_is_on_the_trait() {
        // Compile-time proof the method exists with the expected signature.
        fn _assert<G: GitCli>(g: &G, p: &std::path::Path) {
            let _ = g.sparse_fetch("https://x/y.git", p, ".claude-plugin");
        }
    }
}
