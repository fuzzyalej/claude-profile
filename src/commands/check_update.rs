/// Fetches release metadata. Abstracted so tests can supply canned JSON instead of
/// hitting the network (mirrors `git::GitCli`'s Real/Mock split).
pub trait ReleaseSource {
    /// Body of the GitHub "latest release" API response for `owner/repo`.
    fn latest_release_json(&self, owner: &str, repo: &str) -> anyhow::Result<String>;
}

pub struct GitHubReleaseSource;

impl ReleaseSource for GitHubReleaseSource {
    fn latest_release_json(&self, owner: &str, repo: &str) -> anyhow::Result<String> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
        let body = ureq::get(&url)
            .header("User-Agent", "claude-profile-update-check")
            .call()
            .map_err(|e| anyhow::anyhow!("checking for updates: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| anyhow::anyhow!("reading release response: {e}"))?;
        Ok(body)
    }
}

/// Result of comparing the running version against the latest published release.
pub struct VersionCheck {
    pub current: String,
    pub latest: String,
    pub up_to_date: bool,
}

fn latest_tag(release_json: &str) -> anyhow::Result<String> {
    let v: serde_json::Value = serde_json::from_str(release_json)?;
    let tag = v.get("tag_name").and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("release response has no `tag_name`"))?;
    Ok(tag.trim_start_matches('v').to_string())
}

pub fn check(source: &dyn ReleaseSource, owner: &str, repo: &str, current: &str) -> anyhow::Result<VersionCheck> {
    let json = source.latest_release_json(owner, repo)?;
    let latest = latest_tag(&json)?;
    Ok(VersionCheck { current: current.to_string(), up_to_date: latest == current, latest })
}

/// How the running binary was likely installed — determines the upgrade command printed.
/// Best-effort: inspects the running executable's path rather than tracking install
/// provenance, so it's a guess, not a guarantee.
fn upgrade_hint(current_exe: &std::path::Path) -> &'static str {
    let path = current_exe.to_string_lossy();
    if path.contains("/.cargo/bin/") {
        "cargo install claude-profile --force"
    } else if path.contains("Cellar") || path.contains("homebrew") {
        "brew upgrade claude-profile"
    } else {
        "see https://github.com/fuzzyalej/claude-profile#installing for your install method"
    }
}

pub fn run(current_exe: &std::path::Path) -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    match check(&GitHubReleaseSource, "fuzzyalej", "claude-profile", current) {
        Ok(v) if v.up_to_date => {
            println!("claude-profile {} is up to date", v.current);
        }
        Ok(v) => {
            println!("claude-profile {} is available (you have {})", v.latest, v.current);
            println!("upgrade with: {}", upgrade_hint(current_exe));
        }
        Err(e) => {
            println!("could not check for updates: {e}");
        }
    }
    println!();
    println!("to update installed profile repos and marketplaces instead, run: claude-profile update profiles");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct FakeSource(String);
    impl ReleaseSource for FakeSource {
        fn latest_release_json(&self, _owner: &str, _repo: &str) -> anyhow::Result<String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn reports_up_to_date_when_tags_match() {
        let src = FakeSource(r#"{"tag_name":"v0.3.0"}"#.to_string());
        let v = check(&src, "o", "r", "0.3.0").unwrap();
        assert!(v.up_to_date);
        assert_eq!(v.latest, "0.3.0");
    }

    #[test]
    fn reports_newer_release_available() {
        let src = FakeSource(r#"{"tag_name":"v0.4.0"}"#.to_string());
        let v = check(&src, "o", "r", "0.3.0").unwrap();
        assert!(!v.up_to_date);
        assert_eq!(v.latest, "0.4.0");
    }

    #[test]
    fn errors_on_missing_tag_name() {
        let src = FakeSource(r#"{}"#.to_string());
        assert!(check(&src, "o", "r", "0.3.0").is_err());
    }

    #[test]
    fn upgrade_hint_detects_cargo_install() {
        assert!(upgrade_hint(&PathBuf::from("/Users/x/.cargo/bin/claude-profile")).contains("cargo install"));
    }

    #[test]
    fn upgrade_hint_detects_homebrew() {
        assert!(upgrade_hint(&PathBuf::from("/opt/homebrew/Cellar/claude-profile/0.3.0/bin/claude-profile"))
            .contains("brew upgrade"));
    }

    #[test]
    fn upgrade_hint_falls_back_to_readme() {
        assert!(upgrade_hint(&PathBuf::from("/usr/local/bin/claude-profile")).contains("github.com"));
    }
}
