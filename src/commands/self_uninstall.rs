use crate::fs_paths::Paths;
use std::path::PathBuf;

pub struct UninstallPlan {
    pub binary: PathBuf,
    pub purge_dir: Option<PathBuf>,
}

pub fn plan(current_exe: PathBuf, paths: &Paths, purge: bool) -> UninstallPlan {
    UninstallPlan {
        binary: current_exe,
        purge_dir: if purge { Some(paths.user_profiles_dir()) } else { None },
    }
}

pub fn apply(plan: &UninstallPlan) -> anyhow::Result<()> {
    if plan.binary.exists() {
        std::fs::remove_file(&plan.binary)?;
    }
    if let Some(dir) = &plan.purge_dir {
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}

pub fn run(paths: &Paths, purge: bool) -> anyhow::Result<()> {
    let current = std::env::current_exe()?;
    let pl = plan(current, paths, purge);
    println!("removing binary: {}", pl.binary.display());
    if let Some(dir) = &pl.purge_dir {
        println!("purging profile data (including all vendored plugins/skills): {}", dir.display());
    }
    apply(&pl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_without_purge_targets_only_binary() {
        let paths = crate::fs_paths::Paths::from_home(std::path::PathBuf::from("/h"));
        let pl = plan(std::path::PathBuf::from("/usr/local/bin/claude-profile"), &paths, false);
        assert_eq!(pl.binary, std::path::PathBuf::from("/usr/local/bin/claude-profile"));
        assert!(pl.purge_dir.is_none());
    }

    #[test]
    fn plan_with_purge_includes_profiles_dir_not_dotclaude() {
        let paths = crate::fs_paths::Paths::from_home(std::path::PathBuf::from("/h"));
        let pl = plan(std::path::PathBuf::from("/b/claude-profile"), &paths, true);
        assert_eq!(pl.purge_dir, Some(std::path::PathBuf::from("/h/.claude-profiles")));
        // never ~/.claude:
        assert_ne!(pl.purge_dir, Some(std::path::PathBuf::from("/h/.claude")));
    }

    #[test]
    fn apply_removes_binary_and_purge_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("claude-profile");
        std::fs::write(&bin, "x").unwrap();
        let purge = tmp.path().join(".claude-profiles");
        std::fs::create_dir_all(purge.join("packs")).unwrap();
        apply(&UninstallPlan { binary: bin.clone(), purge_dir: Some(purge.clone()) }).unwrap();
        assert!(!bin.exists());
        assert!(!purge.exists());
    }
}
