use crate::fs_paths::Paths;
use clap::ValueEnum;
use std::path::{Path, PathBuf};

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
    /// `(rc_file, line_to_ensure)` pairs — empty when the shell autoloads scripts from
    /// `script_path`'s directory (fish) and no rc edit is needed. PowerShell can have
    /// several entries: one per installed edition (PowerShell 7 and Windows PowerShell 5.1).
    pub rc_lines: Vec<(PathBuf, String)>,
}

pub fn script(shell: Shell) -> String {
    match shell {
        Shell::Bash => BASH.to_string(),
        Shell::Zsh => ZSH.to_string(),
        Shell::Fish => FISH.to_string(),
        Shell::Powershell => POWERSHELL.to_string(),
    }
}

fn install_plan_with(shell: Shell, paths: &Paths, powershell_profiles: &dyn Fn() -> Vec<PathBuf>) -> InstallPlan {
    let completions_dir = paths.user_profiles_dir().join("completions");
    match shell {
        Shell::Bash => InstallPlan {
            script_path: completions_dir.join("claude-profile.bash"),
            rc_lines: vec![(
                paths.home.join(".bashrc"),
                format!("source {}", completions_dir.join("claude-profile.bash").display()),
            )],
        },
        Shell::Zsh => InstallPlan {
            script_path: completions_dir.join("claude-profile.zsh"),
            rc_lines: vec![(
                paths.home.join(".zshrc"),
                format!("source {}", completions_dir.join("claude-profile.zsh").display()),
            )],
        },
        Shell::Fish => InstallPlan {
            // fish autoloads any *.fish file placed here — no rc edit needed.
            script_path: paths.home.join(".config/fish/completions/claude-profile.fish"),
            rc_lines: Vec::new(),
        },
        Shell::Powershell => {
            let script_path = completions_dir.join("claude-profile.ps1");
            let line = powershell_dot_source_line(&script_path);
            let mut profiles = powershell_profiles();
            if profiles.is_empty() {
                profiles.push(fallback_powershell_profile(paths));
            }
            InstallPlan {
                script_path,
                rc_lines: profiles.into_iter().map(|p| (p, line.clone())).collect(),
            }
        }
    }
}

fn powershell_dot_source_line(script_path: &Path) -> String {
    format!(". '{}'", script_path.display().to_string().replace('\'', "''"))
}

// Used only when no PowerShell executable answers: the PowerShell 7 default on Windows.
fn fallback_powershell_profile(paths: &Paths) -> PathBuf {
    paths.home.join("Documents").join("PowerShell").join("Microsoft.PowerShell_profile.ps1")
}

// Ask each installed edition for its own `$PROFILE`, which already accounts for a
// OneDrive-redirected Documents folder and for `pwsh` on macOS/Linux. Output is forced
// to UTF-8 because Windows PowerShell 5.1 otherwise prints in the OEM code page.
fn detect_powershell_profiles() -> Vec<PathBuf> {
    const QUERY: &str = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; $PROFILE";
    let mut found = Vec::new();
    for exe in ["pwsh", "powershell"] {
        let output = crate::exe::command(exe)
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", QUERY])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let Ok(output) = output else { continue };
        if !output.status.success() {
            continue;
        }
        if let Some(path) = parse_profile_output(&output.stdout) {
            if !found.contains(&path) {
                found.push(path);
            }
        }
    }
    found
}

fn parse_profile_output(stdout: &[u8]) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(stdout);
    let line = text.trim_start_matches('\u{feff}').lines().map(str::trim).find(|l| !l.is_empty())?;
    looks_absolute(line).then(|| PathBuf::from(line))
}

fn looks_absolute(line: &str) -> bool {
    let bytes = line.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/');
    drive || line.starts_with('/') || line.starts_with("\\\\")
}

const MARKER: &str = "# added by `claude-profile completions --install`";

/// Writes the completion script and, if the shell needs it, ensures the rc file
/// sources it (idempotent — skips if the line is already present).
pub fn install(shell: Shell, paths: &Paths) -> anyhow::Result<InstallPlan> {
    install_with(shell, paths, &detect_powershell_profiles)
}

fn install_with(shell: Shell, paths: &Paths, powershell_profiles: &dyn Fn() -> Vec<PathBuf>) -> anyhow::Result<InstallPlan> {
    let plan = install_plan_with(shell, paths, powershell_profiles);
    if let Some(parent) = plan.script_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&plan.script_path, script(shell))?;

    for (rc_file, line) in &plan.rc_lines {
        let existing = std::fs::read_to_string(rc_file).ok();
        let is_new = existing.is_none();
        let existing = existing.unwrap_or_default();
        if !existing.contains(line.as_str()) {
            if let Some(parent) = rc_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut updated = existing;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            // Windows PowerShell 5.1 reads a BOM-less script as ANSI, which garbles a
            // non-ASCII user name in the dot-sourced path.
            if is_new && shell == Shell::Powershell {
                updated.push('\u{feff}');
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
    if plan.rc_lines.is_empty() {
        println!("fish loads completions from this directory automatically — restart fish to pick it up");
    }
    for (rc_file, line) in &plan.rc_lines {
        println!("ensured {} sources it (line: `{line}`)", rc_file.display());
    }
    if shell == Shell::Powershell {
        println!("restart PowerShell (or run `. $PROFILE`) to pick it up");
    } else if let Some((rc_file, _)) = plan.rc_lines.first() {
        println!("restart your shell (or `source {}`) to pick it up", rc_file.display());
    }
    Ok(())
}

const BASH: &str = r#"# claude-profile bash completion
_claude_profile_complete() {
    local cur subcommands
    cur="${COMP_WORDS[COMP_CWORD]}"
    subcommands="list show install update status remove new test find self-uninstall completions statusline"
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
    subcommands=(list show install update status remove new test find self-uninstall completions statusline)
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
complete -c claude-profile -n "__fish_use_subcommand" -a "list show install update status remove new test find self-uninstall completions statusline"
complete -c claude-profile -n "__fish_use_subcommand" -a "(__claude_profile_names)"
complete -c claude-profile -n "__fish_seen_subcommand_from show remove" -a "(__claude_profile_names)"
"#;

const POWERSHELL: &str = r#"# claude-profile PowerShell completion
Register-ArgumentCompleter -Native -CommandName claude-profile -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $subcommands = 'list','show','install','update','status','remove','new','test','find','self-uninstall','completions','statusline'
    $tokens = $commandAst.CommandElements | ForEach-Object { $_.ToString() }

    $position = if ($wordToComplete -eq '') { $tokens.Count } else { $tokens.Count - 1 }

    $candidates = if ($position -le 1) {
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
        assert_eq!(install_plan_with(Shell::Bash, &paths, &Vec::new).rc_lines.len(), 1);
        assert_eq!(install_plan_with(Shell::Zsh, &paths, &Vec::new).rc_lines.len(), 1);
        assert!(!install_plan_with(Shell::Powershell, &paths, &Vec::new).rc_lines.is_empty());
        assert!(install_plan_with(Shell::Fish, &paths, &Vec::new).rc_lines.is_empty());
    }

    #[test]
    fn powershell_wires_every_detected_profile() {
        let paths = Paths::from_home(PathBuf::from("/h"));
        let seven = PathBuf::from("/docs/PowerShell/Microsoft.PowerShell_profile.ps1");
        let five = PathBuf::from("/docs/WindowsPowerShell/Microsoft.PowerShell_profile.ps1");
        let detected = vec![seven.clone(), five.clone()];
        let plan = install_plan_with(Shell::Powershell, &paths, &|| detected.clone());
        let files: Vec<_> = plan.rc_lines.iter().map(|(f, _)| f.clone()).collect();
        assert_eq!(files, vec![seven, five]);
    }

    #[test]
    fn powershell_falls_back_to_documents_when_nothing_detected() {
        let paths = Paths::from_home(PathBuf::from("/h"));
        let plan = install_plan_with(Shell::Powershell, &paths, &Vec::new);
        assert_eq!(
            plan.rc_lines[0].0,
            PathBuf::from("/h/Documents/PowerShell/Microsoft.PowerShell_profile.ps1")
        );
    }

    #[test]
    fn powershell_line_quotes_the_script_path() {
        assert_eq!(
            powershell_dot_source_line(Path::new("/Users/Ann O'Neil/c.ps1")),
            ". '/Users/Ann O''Neil/c.ps1'"
        );
    }

    #[test]
    fn parses_profile_path_from_powershell_output() {
        assert_eq!(
            parse_profile_output("\u{feff}C:\\Users\\Åse\\OneDrive\\Dokumenter\\PowerShell\\p.ps1\r\n".as_bytes()),
            Some(PathBuf::from("C:\\Users\\Åse\\OneDrive\\Dokumenter\\PowerShell\\p.ps1"))
        );
        assert_eq!(
            parse_profile_output(b"/home/a/.config/powershell/Microsoft.PowerShell_profile.ps1\n"),
            Some(PathBuf::from("/home/a/.config/powershell/Microsoft.PowerShell_profile.ps1"))
        );
        assert_eq!(parse_profile_output(b""), None);
        assert_eq!(parse_profile_output(b"not a path\n"), None);
    }

    #[test]
    fn powershell_install_writes_bom_only_for_a_new_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        let fresh = tmp.path().join("fresh").join("profile.ps1");
        let existing = tmp.path().join("existing.ps1");
        std::fs::write(&existing, "Set-Alias g git\n").unwrap();
        let detected = vec![fresh.clone(), existing.clone()];

        install_with(Shell::Powershell, &paths, &|| detected.clone()).unwrap();
        let fresh_text = std::fs::read_to_string(&fresh).unwrap();
        let existing_text = std::fs::read_to_string(&existing).unwrap();
        assert!(fresh_text.starts_with("\u{feff}# added by"));
        assert!(existing_text.starts_with("Set-Alias g git\n"));
        assert!(existing_text.contains("claude-profile.ps1"));

        install_with(Shell::Powershell, &paths, &|| detected.clone()).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap().matches("claude-profile.ps1").count(), 1);
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
        assert!(plan.rc_lines.is_empty());
    }
}
