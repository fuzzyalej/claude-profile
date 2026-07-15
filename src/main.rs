use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod combine;
mod commands;
mod extends;
mod fs_paths;
mod git;
mod index;
mod launch;
mod lock;
mod pack;
mod profile;
mod provision;
mod resolve;
mod spinner;
mod vendor;
mod vendor_fs;

#[derive(Parser)]
#[command(name = "claude-profile", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Profile name(s) to launch (when no subcommand is given). Give several to launch a
    /// combined session; each may be a profile name or a repo reference (owner/repo, URL).
    profiles: Vec<String>,
    /// Skip the provisioning confirmation prompt.
    #[arg(long)]
    yes: bool,
    /// Extra args forwarded to claude after `--`.
    #[arg(last = true)]
    extra: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// List available profiles and their sources.
    List,
    /// Show a profile's details and what it would install. TARGET is a profile
    /// name or a repo reference (owner/repo[#ref], https://…, or git@…).
    Show { target: String },
    /// Install or refresh a profile repo (owner/repo[#ref]) without launching.
    Install { spec: String },
    /// Git-pull profile repos and re-resolve floating marketplaces.
    Update {
        /// Fail if the lockfile is out of date instead of updating.
        #[arg(long)]
        frozen: bool,
    },
    /// Show each profile's vendored plugins/skills under ~/.claude-profiles/store/.
    Status,
    /// Delete a personal profile or cloned pack.
    Remove { target: String },
    /// Scaffold a new profile in ~/.claude-profiles/.
    New { name: String },
    /// Run `claude plugin eval` against a plugin/skill target.
    Test {
        target: String,
        #[arg(long)]
        json: bool,
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Search a local index of plugins across marketplaces. Returns profile-ready
    /// `plugin@marketplace` ids. First run auto-syncs; use --sync to rebuild.
    Find {
        query: Vec<String>,
        #[arg(long)]
        sync: bool,
        #[arg(long)]
        refresh_seeds: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        marketplace: Option<String>,
    },
    /// Remove the claude-profile binary (and optionally profile data).
    SelfUninstall {
        /// Also remove ~/.claude-profiles (all personal profiles, packs, locks).
        #[arg(long)]
        purge: bool,
    },
}

type LoadedProfiles = (Vec<(String, profile::Profile)>, Vec<(PathBuf, String)>);

fn load_all_profiles(
    paths: &fs_paths::Paths, cwd: &std::path::Path,
    env: Option<&std::path::Path>, bundled: &std::path::Path,
) -> LoadedProfiles {
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for (name, path, _src) in resolve::list_available(paths, cwd, env, bundled) {
        match std::fs::read_to_string(&path) {
            Ok(body) => match profile::Profile::from_json_str(&body) {
                Ok(p) => out.push((name, p)),
                Err(e) => failed.push((path, e.to_string())),
            },
            Err(e) => failed.push((path, e.to_string())),
        }
    }
    (out, failed)
}

fn bundled_dir() -> PathBuf {
    // profiles/ holds the reference profiles shipped with the engine; resolved relative to
    // CARGO_MANIFEST_DIR at build time.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("profiles")
}

fn env_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_PROFILE_DIR").map(PathBuf::from)
}

fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    let paths = fs_paths::Paths::detect()?;
    let cwd = std::env::current_dir()?;
    let bundled = bundled_dir();
    let env = env_dir();

    match cli.command {
        Some(Command::List) => {
            commands::list::run(&paths, &cwd, env.as_deref(), &bundled)?;
            Ok(0)
        }
        Some(Command::Show { target }) => {
            commands::show::run(&target, &paths, &cwd, env.as_deref(), &bundled)?;
            Ok(0)
        }
        Some(Command::Install { spec }) => {
            let dir = pack::install_pack(&git::RealGit, &spec, &paths)?;
            println!("installed pack at {}", dir.display());
            Ok(0)
        }
        Some(Command::Update { frozen }) => {
            handle_update(frozen, &paths, &cwd, env.as_deref(), &bundled)?;
            Ok(0)
        }
        Some(Command::New { name }) => {
            commands::new::run(&name, &paths)?;
            Ok(0)
        }
        Some(Command::Test { target, json, extra }) => commands::test::run(&target, json, &extra),
        Some(Command::Find { query, sync, refresh_seeds, json, limit, marketplace }) => {
            commands::find::run(&paths, &query, sync, refresh_seeds, json, limit, marketplace.as_deref())
        }
        Some(Command::SelfUninstall { purge }) => {
            commands::self_uninstall::run(&paths, purge)?;
            Ok(0)
        }
        Some(Command::Status) => {
            let (profiles, failed) = load_all_profiles(&paths, &cwd, env.as_deref(), &bundled);
            for (path, err) in &failed {
                eprintln!("warning: could not parse profile {}: {err}", path.display());
            }
            commands::status::run(&paths, &profiles)?;
            Ok(0)
        }
        Some(Command::Remove { target }) => {
            let plan = commands::remove::remove_target(&target, &paths, &cwd, env.as_deref(), &bundled)?;
            commands::remove::apply(&plan)?;
            println!("removed {target}");
            Ok(0)
        }
        None => handle_launch(&cli.profiles, cli.yes, &cli.extra, &paths, &cwd, env.as_deref(), &bundled),
    }
}

fn handle_update(
    frozen: bool,
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    bundled: &std::path::Path,
) -> anyhow::Result<()> {
    let updated = pack::update_all_packs(&git::RealGit, paths)?;
    for name in &updated { println!("updated pack {name}"); }
    let (profiles, failed) = load_all_profiles(paths, cwd, env, bundled);
    for (path, err) in &failed {
        eprintln!("warning: skipping unparseable profile {}: {err}", path.display());
    }
    let dir_lookup = |n: &str| paths.marketplace_clone_dir(n);
    if frozen {
        let mut triples = Vec::new();
        for (name, profile) in &profiles {
            let Some(profile) = resolve_extends_or_warn(name, profile, paths, cwd, env, bundled) else { continue };
            let resolved = resolve::resolve(name, paths, cwd, env, bundled)?;
            let lp = lock::lock_path(name, &resolved.path, &resolved.source, paths);
            let lf = lock::Lockfile::load(&lp)?.unwrap_or_else(|| lock::Lockfile::new(name));
            triples.push((name.clone(), profile, lf));
        }
        commands::update::frozen_check(&triples)?;
        println!("--frozen: all locks up to date");
    } else {
        for (name, profile) in &profiles {
            let Some(profile) = resolve_extends_or_warn(name, profile, paths, cwd, env, bundled) else { continue };
            let missing = profile.marketplaces.keys().find(|mkt| !paths.marketplace_clone_dir(mkt).is_dir());
            if let Some(mkt) = missing {
                eprintln!(
                    "skipping '{name}': marketplace '{mkt}' not cloned yet (launch the profile once to provision it)"
                );
                continue;
            }
            let resolved = resolve::resolve(name, paths, cwd, env, bundled)?;
            let lp = lock::lock_path(name, &resolved.path, &resolved.source, paths);
            let mut lf = lock::Lockfile::load(&lp)?.unwrap_or_else(|| lock::Lockfile::new(name));
            commands::update::reresolve_profile(&git::RealGit, &profile, name, cwd, paths, &dir_lookup, &mut lf)?;
            lf.save(&lp)?;
        }
    }
    Ok(())
}

/// Resolve `extends` for a profile, printing a warning and returning `None`
/// (so the caller can skip it) instead of aborting the whole `update` run.
fn resolve_extends_or_warn(
    name: &str,
    profile: &profile::Profile,
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    bundled: &std::path::Path,
) -> Option<profile::Profile> {
    match extends::resolve_extends(profile.clone(), &|parent| {
        Ok(resolve::resolve(parent, paths, cwd, env, bundled)?.profile)
    }) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("warning: skipping '{name}': could not resolve extends: {e}");
            None
        }
    }
}

/// Dispatch a launch: no args prints help, one arg launches a single profile (with the
/// owner/repo sugar), several launch a combined session.
fn handle_launch(
    names: &[String],
    assume_yes: bool,
    extra: &[String],
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    bundled: &std::path::Path,
) -> anyhow::Result<i32> {
    match names {
        [] => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(0)
        }
        [name] => {
            let one = resolve_one(name, paths, cwd, env, bundled)?;
            let lock_file = lock::lock_path(&one.key, &one.path, &one.source, paths);
            provision_pin_launch(&one.profile, &one.key, &lock_file, assume_yes, extra, cwd, paths)
        }
        _ => launch_combined(names, assume_yes, extra, paths, cwd, env, bundled),
    }
}

/// A resolved launch target: its display key, `extends`-expanded profile, and the source
/// file/origin (used to place a single profile's lockfile).
struct ResolvedTarget {
    key: String,
    profile: profile::Profile,
    path: std::path::PathBuf,
    source: resolve::ProfileSource,
}

/// Resolve one launch target (a profile name, or an owner/repo/URL installed as a pack).
fn resolve_one(
    name: &str,
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    bundled: &std::path::Path,
) -> anyhow::Result<ResolvedTarget> {
    let key = if name.contains('/') {
        let dir = pack::install_pack(&git::RealGit, name, paths)?;
        pack::default_profile_name(&dir)?
    } else {
        name.to_string()
    };
    let resolved = resolve::resolve(&key, paths, cwd, env, bundled)?;
    let (path, source) = (resolved.path.clone(), resolved.source.clone());
    let profile = extends::resolve_extends(resolved.profile, &|parent| {
        Ok(resolve::resolve(parent, paths, cwd, env, bundled)?.profile)
    })?;
    Ok(ResolvedTarget { key, profile, path, source })
}

/// Resolve several targets, merge them into one effective profile, and launch it with a
/// combined lockfile under `~/.claude-profiles/locks/`.
fn launch_combined(
    names: &[String],
    assume_yes: bool,
    extra: &[String],
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    bundled: &std::path::Path,
) -> anyhow::Result<i32> {
    let mut resolved = Vec::new();
    for name in names {
        let t = resolve_one(name, paths, cwd, env, bundled)?;
        resolved.push((t.key, t.profile));
    }
    let combined = combine::combine_profiles(&resolved)?;
    let key = combined.name.clone();
    let lock_file = paths.locks_dir().join(format!("{key}.lock"));
    provision_pin_launch(&combined, &key, &lock_file, assume_yes, extra, cwd, paths)
}

/// Shared launch tail: provision the profile (cloning marketplaces / prompting as needed),
/// pin its marketplaces into `lock_file`, vendor plugins against the pinned checkout, then
/// spawn `claude` for the session with the vendored plugin dirs.
fn provision_pin_launch(
    profile: &profile::Profile,
    key: &str,
    lock_file: &std::path::Path,
    assume_yes: bool,
    extra: &[String],
    cwd: &std::path::Path,
    paths: &fs_paths::Paths,
) -> anyhow::Result<i32> {
    spinner::spin("provisioning marketplaces...", "provisioned", || {
        provision::provision(&git::RealGit, profile, key, cwd, paths, assume_yes)
    })?;

    let mut lock = lock::Lockfile::load(lock_file)?.unwrap_or_else(|| lock::Lockfile::new(key));
    let dir_lookup = |n: &str| paths.marketplace_clone_dir(n);
    provision::pin_marketplaces(&git::RealGit, profile, &dir_lookup, &mut lock, false)?;
    if let Some(parent) = lock_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    lock.save(lock_file)?;

    spinner::spin("vendoring plugins...", "vendored", || {
        provision::vendor_plugins(&git::RealGit, profile, key, cwd, paths, false)
    })?;

    let args = launch::build_args(profile, key, paths, extra)?;
    launch::spawn(key, &args)
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}
