use crate::fs_paths::Paths;
use clap::ValueEnum;
use std::path::PathBuf;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

/// Where `install` writes the completion script, and (for shells without an
/// autoloaded completions directory) the line that must be sourced for it to
/// take effect.
pub struct InstallPlan {
    pub script_path: PathBuf,
    /// `(rc_file, line_to_ensure)` — `None` when the shell autoloads scripts from
    /// `script_path`'s directory (fish) and no rc edit is needed.
    pub rc_line: Option<(PathBuf, String)>,
}

pub fn script(shell: Shell) -> String {
    match shell {
        Shell::Bash => BASH.to_string(),
        Shell::Zsh => ZSH.to_string(),
        Shell::Fish => FISH.to_string(),
        Shell::Powershell => POWERSHELL.to_string(),
    }
}

pub fn install_plan(shell: Shell, paths: &Paths) -> InstallPlan {
    let completions_dir = paths.user_profiles_dir().join("completions");
    match shell {
        Shell::Bash => InstallPlan {
            script_path: completions_dir.join("claude-profile.bash"),
            rc_line: Some((
                paths.home.join(".bashrc"),
                format!("source {}", completions_dir.join("claude-profile.bash").display()),
            )),
        },
        Shell::Zsh => InstallPlan {
            script_path: completions_dir.join("claude-profile.zsh"),
            rc_line: Some((
                paths.home.join(".zshrc"),
                format!("source {}", completions_dir.join("claude-profile.zsh").display()),
            )),
        },
        Shell::Fish => InstallPlan {
            // fish autoloads any *.fish file placed here — no rc edit needed.
            script_path: paths.home.join(".config/fish/completions/claude-profile.fish"),
            rc_line: None,
        },
        Shell::Powershell => InstallPlan {
            script_path: completions_dir.join("claude-profile.ps1"),
            rc_line: Some((
                powershell_profile_path(paths),
                format!(". {}", completions_dir.join("claude-profile.ps1").display()),
            )),
        },
    }
}

fn powershell_profile_path(paths: &Paths) -> PathBuf {
    // Windows PowerShell / PowerShell 7 default profile location. HOME is set on
    // Windows too (git-bash/MSYS environments export it); this is a best-effort
    // default and the printed rc_line can always be added manually instead.
    paths.home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1")
}

const MARKER: &str = "# added by `claude-profile completions --install`";

/// Writes the completion script and, if the shell needs it, ensures the rc file
/// sources it (idempotent — skips if the line is already present).
pub fn install(shell: Shell, paths: &Paths) -> anyhow::Result<InstallPlan> {
    let plan = install_plan(shell, paths);
    if let Some(parent) = plan.script_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&plan.script_path, script(shell))?;

    if let Some((rc_file, line)) = &plan.rc_line {
        let existing = std::fs::read_to_string(rc_file).unwrap_or_default();
        if !existing.contains(line.as_str()) {
            if let Some(parent) = rc_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut updated = existing;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(MARKER);
            updated.push('\n');
            updated.push_str(line);
            updated.push('\n');
            std::fs::write(rc_file, updated)?;
        }
    }
    Ok(plan)
}

pub fn run(shell: Shell, install_flag: bool, paths: &Paths) -> anyhow::Result<()> {
    if !install_flag {
        print!("{}", script(shell));
        return Ok(());
    }
    let plan = install(shell, paths)?;
    println!("wrote completion script: {}", plan.script_path.display());
    match &plan.rc_line {
        Some((rc_file, line)) => {
            println!("ensured {} sources it (line: `{line}`)", rc_file.display());
            println!("restart your shell (or `source {}`) to pick it up", rc_file.display());
        }
        None => println!("fish loads completions from this directory automatically — restart fish to pick it up"),
    }
    Ok(())
}

const BASH: &str = r#"# claude-profile bash completion
_claude_profile_complete() {
    local cur subcommands
    cur="${COMP_WORDS[COMP_CWORD]}"
    subcommands="list show install update status remove new test find self-uninstall completions"
    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=( $(compgen -W "$subcommands $(claude-profile profile-names 2>/dev/null)" -- "$cur") )
        return
    fi
    case "${COMP_WORDS[1]}" in
        show|remove)
            COMPREPLY=( $(compgen -W "$(claude-profile profile-names 2>/dev/null)" -- "$cur") )
            ;;
    esac
}
complete -F _claude_profile_complete claude-profile
"#;

const ZSH: &str = r#"#compdef claude-profile
# claude-profile zsh completion

_claude_profile() {
    local -a subcommands profiles
    subcommands=(list show install update status remove new test find self-uninstall completions)
    profiles=(${(f)"$(claude-profile profile-names 2>/dev/null)"})

    if (( CURRENT == 2 )); then
        compadd -a subcommands
        compadd -a profiles
        return
    fi

    case "${words[2]}" in
        show|remove)
            compadd -a profiles
            ;;
    esac
}

# This script is sourced from .zshrc rather than autoloaded from $fpath, so the
# `#compdef` tag above is inert and we must register the function ourselves. That
# needs the completion system loaded; initialise it if a framework hasn't already.
if ! command -v compdef >/dev/null 2>&1; then
    autoload -Uz compinit && compinit -u
fi
compdef _claude_profile claude-profile
"#;

const FISH: &str = r#"# claude-profile fish completion
function __claude_profile_names
    claude-profile profile-names 2>/dev/null
end

complete -c claude-profile -f
complete -c claude-profile -n "__fish_use_subcommand" -a "list show install update status remove new test find self-uninstall completions"
complete -c claude-profile -n "__fish_use_subcommand" -a "(__claude_profile_names)"
complete -c claude-profile -n "__fish_seen_subcommand_from show remove" -a "(__claude_profile_names)"
"#;

const POWERSHELL: &str = r#"# claude-profile PowerShell completion
Register-ArgumentCompleter -Native -CommandName claude-profile -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $subcommands = 'list','show','install','update','status','remove','new','test','find','self-uninstall','completions'
    $tokens = $commandAst.CommandElements | ForEach-Object { $_.ToString() }

    $candidates = if ($tokens.Count -le 2) {
        $subcommands + (& claude-profile profile-names 2>$null)
    } elseif ($tokens[1] -in @('show', 'remove')) {
        & claude-profile profile-names 2>$null
    } else {
        @()
    }

    $candidates | Where-Object { $_ -like "$wordToComplete*" } |
        ForEach-Object { [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_script_references_profile_names() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Powershell] {
            assert!(script(shell).contains("profile-names"));
        }
    }

    #[test]
    fn zsh_script_registers_via_compdef_since_it_is_sourced() {
        // Sourced (not autoloaded) scripts must call `compdef` to bind the
        // completion; the `#compdef` tag alone is inert when sourced.
        let zsh = script(Shell::Zsh);
        assert!(zsh.contains("compdef _claude_profile claude-profile"));
        assert!(zsh.contains("compinit"));
    }

    #[test]
    fn bash_and_zsh_and_powershell_need_an_rc_line_fish_does_not() {
        let paths = Paths::from_home(PathBuf::from("/h"));
        assert!(install_plan(Shell::Bash, &paths).rc_line.is_some());
        assert!(install_plan(Shell::Zsh, &paths).rc_line.is_some());
        assert!(install_plan(Shell::Powershell, &paths).rc_line.is_some());
        assert!(install_plan(Shell::Fish, &paths).rc_line.is_none());
    }

    #[test]
    fn install_writes_script_and_appends_rc_line_once() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());

        let plan = install(Shell::Zsh, &paths).unwrap();
        assert!(plan.script_path.exists());
        let rc = std::fs::read_to_string(paths.home.join(".zshrc")).unwrap();
        assert_eq!(rc.matches("claude-profile.zsh").count(), 1);

        // idempotent: installing again does not duplicate the rc line
        install(Shell::Zsh, &paths).unwrap();
        let rc2 = std::fs::read_to_string(paths.home.join(".zshrc")).unwrap();
        assert_eq!(rc2.matches("claude-profile.zsh").count(), 1);
    }

    #[test]
    fn install_preserves_existing_rc_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        std::fs::write(paths.home.join(".bashrc"), "export FOO=bar\n").unwrap();

        install(Shell::Bash, &paths).unwrap();
        let rc = std::fs::read_to_string(paths.home.join(".bashrc")).unwrap();
        assert!(rc.starts_with("export FOO=bar\n"));
        assert!(rc.contains("claude-profile.bash"));
    }

    #[test]
    fn fish_install_writes_no_rc_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let plan = install(Shell::Fish, &paths).unwrap();
        assert!(plan.script_path.exists());
        assert!(plan.rc_line.is_none());
    }
}
