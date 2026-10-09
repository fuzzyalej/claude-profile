use crate::coord::backend::Backend;
use crate::coord::headless::Headless;
use crate::coord::herdr::{Herdr, RealHerdr};
use crate::coord::manager::Manager;
use crate::coord::perm::permission_flags;
use crate::coord::plugin::seed_coordinator_plugin;
use crate::coord::state::{new_run_id, now, BackendKind, RunDir, RunState};
use crate::coord::worktree;
use crate::fs_paths::Paths;
use crate::profile::Profile;
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const SERVER_NAME: &str = "claude-profile-workers";
const APPEND_FLAG: &str = "--append-system-prompt";

const POLICY: &str = "You are a claude-profile coordinator. Policy for this session:
- Delegate first. Hand implementation, research, reviews and other multi-step work to workers with the claude-profile-workers tools. Do work yourself only for answers, plans, merging, or a trivial edit, and say briefly why when you don't delegate.
- If no profile fits a task, delegate it to a worker with the clean profile instead of doing it yourself.
- Load the coordinating-workers skill before you start any task.
- Keep the work invisible. Report results and decisions to the user, not worker status chatter.
- Clean up after yourself. After you merge a worker's branch, call cleanup for it (this closes its tab and removes its worktree). Delete temporary files you created. Close tabs, panes and browser tabs you opened. Before you report a task done, check that nothing you created is left behind.";

pub struct Options {
    pub profiles: Vec<String>,
    pub yes: bool,
    pub max_workers: usize,
    pub headless: bool,
    pub extra: Vec<String>,
}

pub fn inject(profile: &mut Profile, exe: &Path, run_id: &str, plugin_dir: &Path) {
    if !profile.mcp_servers.is_object() {
        profile.mcp_servers = Value::Object(Map::new());
    }
    if let Some(servers) = profile.mcp_servers.as_object_mut() {
        servers.insert(
            SERVER_NAME.to_string(),
            json!({"command": exe.display().to_string(), "args": ["workers-mcp", "--run", run_id]}),
        );
    }
    profile.plugin_dirs.push(plugin_dir.display().to_string());
}

pub fn coordinator_extra(extra: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(extra.len() + 2);
    let mut user: Vec<String> = Vec::new();
    let mut iter = extra.iter();
    while let Some(arg) = iter.next() {
        if arg == APPEND_FLAG {
            if let Some(value) = iter.next() {
                user.push(value.clone());
            }
        } else if let Some(value) = arg.strip_prefix("--append-system-prompt=") {
            user.push(value.to_string());
        } else {
            out.push(arg.clone());
        }
    }
    user.push(POLICY.to_string());
    out.push(APPEND_FLAG.to_string());
    out.push(user.join("\n\n"));
    out
}

pub fn choose_backend(herdr_env: Option<&str>, pane: Option<&str>, headless: bool) -> BackendKind {
    let in_herdr = herdr_env == Some("1") && pane.is_some_and(|p| !p.is_empty());
    if in_herdr && !headless {
        BackendKind::Herdr
    } else {
        BackendKind::Headless
    }
}

pub fn headless_warning(backend: BackendKind, permission_flags: &[String]) -> Option<&'static str> {
    if backend == BackendKind::Headless && permission_flags.is_empty() {
        Some("warning: headless workers can't ask for approval; pass a permission mode after --, for example -- --permission-mode acceptEdits")
    } else {
        None
    }
}

pub fn run(opts: &Options, paths: &Paths, cwd: &Path, env: Option<&Path>, bundled: &Path) -> Result<i32> {
    if opts.profiles.is_empty() {
        bail!("coordinate needs at least one profile");
    }
    if opts.max_workers == 0 {
        bail!("--max-workers must be at least 1");
    }
    let target = crate::resolve_launch(&opts.profiles, paths, cwd, env, bundled)?;
    let herdr_env = std::env::var("HERDR_ENV").ok();
    let pane = std::env::var("HERDR_PANE_ID").ok().filter(|p| !p.is_empty());
    let t = now();
    let run = RunState {
        run_id: new_run_id(t),
        repo_root: worktree::repo_root(cwd),
        cwd: cwd.to_path_buf(),
        permission_flags: permission_flags(&opts.extra),
        backend: choose_backend(herdr_env.as_deref(), pane.as_deref(), opts.headless),
        max_workers: opts.max_workers,
        coordinator_pane: pane,
        created_at: t,
    };
    if let Some(warning) = headless_warning(run.backend, &run.permission_flags) {
        eprintln!("{warning}");
    }
    RunDir::create(paths, &run)?;
    let plugin_dir = seed_coordinator_plugin(paths, env!("CARGO_PKG_VERSION"))
        .context("writing the coordinator plugin")?;
    let exe = std::env::current_exe().context("finding the claude-profile executable")?;
    let mut profile = target.profile;
    inject(&mut profile, &exe, &run.run_id, &plugin_dir);
    let extra = coordinator_extra(&opts.extra);
    crate::provision_pin_launch(&profile, &target.key, &target.lock_file, opts.yes, &extra, cwd, paths)
}

pub fn serve_workers(run_id: &str, paths: &Paths, env: Option<&Path>, bundled: &Path) -> Result<i32> {
    let dir = RunDir::open(paths, run_id)?;
    let run = dir.load_run()?;
    let _lock = dir
        .hold_lock()
        .map_err(|e| eprintln!("warning: {e:#}; runs --clean may treat this run as inactive"))
        .ok();
    match run.backend {
        BackendKind::Headless => serve_with(Headless::new(), run, dir, paths, env, bundled),
        BackendKind::Herdr => {
            let pane = run
                .coordinator_pane
                .clone()
                .with_context(|| format!("run '{run_id}' uses Herdr but has no coordinator pane"))?;
            serve_with(Herdr::new(RealHerdr, pane), run, dir, paths, env, bundled)
        }
    }
}

fn serve_with<B: Backend>(
    backend: B,
    run: RunState,
    dir: RunDir,
    paths: &Paths,
    env: Option<&Path>,
    bundled: &Path,
) -> Result<i32> {
    let exe = std::env::current_exe().context("finding the claude-profile executable")?;
    let run_dir = dir.root.clone();
    let home = paths.home.clone();
    let cwd = run.cwd.clone();
    let env: Option<PathBuf> = env.map(Path::to_path_buf);
    let bundled = bundled.to_path_buf();
    let profile_exists = {
        let (home, cwd, env, bundled) = (home.clone(), cwd.clone(), env.clone(), bundled.clone());
        move |name: &str| {
            crate::resolve::resolve(name, &Paths::from_home(home.clone()), &cwd, env.as_deref(), &bundled).is_ok()
        }
    };
    let prepare = {
        let (home, cwd, env, bundled) = (home.clone(), cwd.clone(), env.clone(), bundled.clone());
        move |names: &[String]| {
            crate::prepare_profiles(names, &cwd, &Paths::from_home(home.clone()), env.as_deref(), &bundled)
        }
    };
    let list = {
        let home = home.clone();
        move || {
            let (profiles, _) =
                crate::load_all_profiles(&Paths::from_home(home.clone()), &cwd, env.as_deref(), &bundled);
            profiles.into_iter().map(|(name, p)| (name, p.description)).collect()
        }
    };
    let mut mgr = Manager {
        run,
        dir,
        backend,
        spec_base: (exe, run_dir),
        profile_exists: Box::new(profile_exists),
        list: Box::new(list),
        paths: Paths::from_home(home),
        fallback: Box::new(|| Box::new(Headless::new()) as Box<dyn Backend>),
        fallback_backend: None,
        prepare: Box::new(prepare),
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    crate::coord::mcp::serve(&mut mgr, stdin.lock(), stdout.lock())?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_adds_server_and_plugin_dir() {
        let mut p = Profile::from_json_str(r#"{"name":"p"}"#).unwrap();
        inject(&mut p, Path::new("/bin/claude-profile"), "1-abcdef", Path::new("/plugins/coordinator"));
        assert_eq!(
            p.mcp_servers,
            json!({"claude-profile-workers": {"command": "/bin/claude-profile", "args": ["workers-mcp", "--run", "1-abcdef"]}})
        );
        assert_eq!(p.plugin_dirs, vec!["/plugins/coordinator"]);
    }

    #[test]
    fn inject_keeps_existing_servers() {
        let mut p = Profile::from_json_str(
            r#"{"name":"p","pluginDirs":["vendor/x"],"mcpServers":{"s":{"command":"echo"}}}"#,
        )
        .unwrap();
        inject(&mut p, Path::new("/bin/cp"), "r", Path::new("/plugins/coordinator"));
        assert_eq!(p.mcp_servers["s"], json!({"command": "echo"}));
        assert_eq!(p.mcp_servers["claude-profile-workers"]["command"], "/bin/cp");
        assert_eq!(p.plugin_dirs, vec!["vendor/x", "/plugins/coordinator"]);
    }

    #[test]
    fn headless_without_permission_flags_warns() {
        assert_eq!(
            headless_warning(BackendKind::Headless, &[]),
            Some("warning: headless workers can't ask for approval; pass a permission mode after --, for example -- --permission-mode acceptEdits")
        );
        assert_eq!(headless_warning(BackendKind::Headless, &["--permission-mode".into(), "plan".into()]), None);
        assert_eq!(headless_warning(BackendKind::Herdr, &[]), None);
    }

    #[test]
    fn choose_backend_cases() {
        assert_eq!(choose_backend(Some("1"), Some("p1"), false), BackendKind::Herdr);
        assert_eq!(choose_backend(Some("1"), Some("p1"), true), BackendKind::Headless);
        assert_eq!(choose_backend(Some("1"), None, false), BackendKind::Headless);
        assert_eq!(choose_backend(Some("0"), Some("p1"), false), BackendKind::Headless);
        assert_eq!(choose_backend(None, Some("p1"), false), BackendKind::Headless);
        assert_eq!(choose_backend(Some("1"), Some(""), false), BackendKind::Headless);
    }

    #[test]
    fn coordinator_extra_appends_policy() {
        assert!(POLICY.contains("Delegate first"));
        assert!(POLICY.contains("the clean profile"));
        let out = coordinator_extra(&["--model".to_string(), "opus".to_string()]);
        assert_eq!(out, ["--model", "opus", "--append-system-prompt", POLICY]);
    }

    #[test]
    fn coordinator_extra_joins_user_prompt_after_theirs() {
        let out = coordinator_extra(&[
            "--append-system-prompt".to_string(),
            "mine".to_string(),
            "--model".to_string(),
            "opus".to_string(),
        ]);
        assert_eq!(
            out,
            ["--model".to_string(), "opus".to_string(), "--append-system-prompt".to_string(), format!("mine\n\n{POLICY}")]
        );
    }

    #[test]
    fn coordinator_extra_handles_equals_form() {
        let out = coordinator_extra(&["--append-system-prompt=mine".to_string()]);
        assert_eq!(out, ["--append-system-prompt".to_string(), format!("mine\n\n{POLICY}")]);
    }

    #[test]
    fn coordinator_extra_adds_the_flag_once() {
        let out = coordinator_extra(&[]);
        assert_eq!(out.iter().filter(|a| *a == "--append-system-prompt").count(), 1);
    }
}
