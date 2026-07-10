use std::path::PathBuf;

pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    pub fn from_home(home: PathBuf) -> Paths {
        Paths { home }
    }

    pub fn detect() -> anyhow::Result<Paths> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        Ok(Paths::from_home(home))
    }

    pub fn user_profiles_dir(&self) -> PathBuf {
        self.home.join(".claude-profiles")
    }

    pub fn claude_skills_dir(&self) -> PathBuf {
        self.home.join(".claude").join("skills")
    }

    // Phase 2/3: profile locking (concurrent-launch guard) will use this.
    #[allow(dead_code)]
    pub fn locks_dir(&self) -> PathBuf {
        self.user_profiles_dir().join("locks")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn derives_paths_from_home() {
        let p = Paths::from_home(PathBuf::from("/h"));
        assert_eq!(p.user_profiles_dir(), PathBuf::from("/h/.claude-profiles"));
        assert_eq!(p.claude_skills_dir(), PathBuf::from("/h/.claude/skills"));
        assert_eq!(p.locks_dir(), PathBuf::from("/h/.claude-profiles/locks"));
    }
}
