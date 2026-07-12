use clap::{Parser, Subcommand};
use claude::ClaudeCli;
use std::path::PathBuf;

mod claude;
mod combine;
mod commands;
mod enablement;
mod extends;
mod fs_paths;
mod git;
mod index;
mod launch;
mod lock;
mod pack;
mod profile;
mod provision;
mod refmap;
mod resolve;

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
    /// Show installed plugins/marketplaces and which profiles reference each.
    Status,
    /// Disable (in global settings) a profile's plugins that no other profile uses,
    /// so plain `claude` sessions don't load them. They stay installed; launching the
    /// profile re-enables them for that session.
    Disable {
        profile: String,
        /// Report what would be disabled without writing settings.
        #[arg(long)]
        dry_run: bool,
    },
    /// Uninstall plugins/marketplaces no profile references.
    Gc {
        #[arg(long)]
        dry_run: bool,
    },
    /// Delete a personal profile or cloned pack.
    Remove {
        target: String,
        /// Also gc plugins/marketplaces left unreferenced afterward.
        #[arg(long)]
        prune: bool,
    },
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

type ProfilesForRefmap = (Vec<(String, profile::Profile)>, Vec<(PathBuf, String)>);

fn profiles_for_refmap(
    paths: &fs_paths::Paths, cwd: &std::path::Path,
    env: Option<&std::path::Path>, bundled: &std::path::Path,
) -> ProfilesForRefmap {
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
            let (profiles, _failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &bundled);
            let refmap = refmap::build_refmap(&profiles);
            let referenced: Vec<String> = refmap.plugin_refs.keys().cloned().collect();
            commands::self_uninstall::run(&paths, purge, &referenced)?;
            Ok(0)
        }
        Some(Command::Disable { profile, dry_run }) => {
            handle_disable(&profile, dry_run, &paths, &cwd, env.as_deref(), &bundled)?;
            Ok(0)
        }
        Some(Command::Status) => {
            let (profiles, failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &bundled);
            for (path, err) in &failed {
                eprintln!("warning: could not parse profile {}: {err}", path.display());
            }
            commands::status::run(&claude::RealClaude::new(), &profiles)?;
            Ok(0)
        }
        Some(Command::Gc { dry_run }) => {
            let (profiles, failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &bundled);
            if !failed.is_empty() {
                for (path, err) in &failed {
                    eprintln!("error: could not parse profile {}: {err}", path.display());
                }
                anyhow::bail!(
                    "refusing to gc: {} profile(s) could not be parsed; fix or remove them first (a broken profile hides the plugins it references, which gc would then delete)",
                    failed.len()
                );
            }
            let report = commands::gc::run(&claude::RealClaude::new(), &profiles, dry_run)?;
            let verb = if dry_run { "would remove" } else { "removed" };
            for id in &report.removed_plugins { println!("{verb} plugin {id}"); }
            for m in &report.removed_marketplaces { println!("{verb} marketplace {m}"); }
            if report.removed_plugins.is_empty() && report.removed_marketplaces.is_empty() {
                println!("nothing to remove");
            }
            Ok(0)
        }
        Some(Command::Remove { target, prune }) => {
            let plan = commands::remove::remove_target(&target, &paths, &cwd, env.as_deref(), &bundled)?;
            commands::remove::apply(&plan)?;
            println!("removed {target}");
            if prune {
                let (profiles, failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &bundled);
                if !failed.is_empty() {
                    for (path, err) in &failed {
                        eprintln!("error: could not parse profile {}: {err}", path.display());
                    }
                    anyhow::bail!(
                        "refusing to gc: {} profile(s) could not be parsed; fix or remove them first (a broken profile hides the plugins it references, which gc would then delete)",
                        failed.len()
                    );
                }
                let report = commands::gc::run(&claude::RealClaude::new(), &profiles, false)?;
                for id in &report.removed_plugins { println!("pruned plugin {id}"); }
                for m in &report.removed_marketplaces { println!("pruned marketplace {m}"); }
            }
            Ok(0)
        }
        None => handle_launch(&cli.profiles, cli.yes, &cli.extra, &paths, &cwd, env.as_deref(), &bundled),
    }
}

fn handle_disable(
    profile: &str,
    dry_run: bool,
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    bundled: &std::path::Path,
) -> anyhow::Result<()> {
    let (profiles, failed) = profiles_for_refmap(paths, cwd, env, bundled);
    for (path, err) in &failed {
        eprintln!("warning: could not parse profile {}: {err}", path.display());
    }
    // Expand `extends` so inherited plugins count toward the shared set.
    let expanded: Vec<(String, profile::Profile)> = profiles
        .iter()
        .filter_map(|(name, p)| {
            resolve_extends_or_warn(name, p, paths, cwd, env, bundled).map(|rp| (name.clone(), rp))
        })
        .collect();
    commands::disable::run(paths, &expanded, profile, dry_run)
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
    let (profiles, failed) = profiles_for_refmap(paths, cwd, env, bundled);
    for (path, err) in &failed {
        eprintln!("warning: skipping unparseable profile {}: {err}", path.display());
    }
    let cli = claude::RealClaude::new();
    let mkt_dirs = installed_mkts_install_dirs(&cli)?;
    let dir_lookup = |n: &str| mkt_dirs.get(n).cloned().unwrap_or_default();
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
            let missing = profile.marketplaces.keys().find(|mkt| !mkt_dirs.contains_key(mkt.as_str()));
            if let Some(mkt) = missing {
                eprintln!(
                    "skipping '{name}': marketplace '{mkt}' not installed (launch the profile once to provision it)"
                );
                continue;
            }
            let resolved = resolve::resolve(name, paths, cwd, env, bundled)?;
            let lp = lock::lock_path(name, &resolved.path, &resolved.source, paths);
            let mut lf = lock::Lockfile::load(&lp)?.unwrap_or_else(|| lock::Lockfile::new(name));
            commands::update::reresolve_profile(&git::RealGit, &profile, &dir_lookup, &mut lf)?;
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
            provision_pin_launch(&one.profile, &one.key, &lock_file, assume_yes, extra, paths)
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
    provision_pin_launch(&combined, &key, &lock_file, assume_yes, extra, paths)
}

fn print_enablement_warnings(en: &enablement::Enablement) {
    if !en.leaking_skills.is_empty() {
        eprintln!("WARNING: these manifest-less skills load in EVERY session and cannot be gated:");
        for s in &en.leaking_skills {
            eprintln!("  - {s}  (add .claude-plugin/plugin.json to make it gateable)");
        }
    }
    if !en.suppressed_mcp.is_empty() {
        eprintln!("WARNING: --strict-mcp-config will drop MCP servers bundled by these plugins;");
        eprintln!("         re-declare them in the profile's mcpServers to keep them:");
        for id in &en.suppressed_mcp {
            eprintln!("  - {id}");
        }
    }
}

/// Shared launch tail: provision the profile, pin its marketplaces into `lock_file`, warn
/// about leaking skills / suppressed MCP, then spawn `claude` for the session.
fn provision_pin_launch(
    profile: &profile::Profile,
    key: &str,
    lock_file: &std::path::Path,
    assume_yes: bool,
    extra: &[String],
    paths: &fs_paths::Paths,
) -> anyhow::Result<i32> {
    let cli = claude::RealClaude::new();
    provision::provision(&cli, profile, assume_yes)?;

    let mut lock = lock::Lockfile::load(lock_file)?.unwrap_or_else(|| lock::Lockfile::new(key));
    let installed_mkts = cli.list_marketplaces()?;
    let mkt_by_name = installed_mkts_install_dirs(&cli)?;
    ensure_marketplaces_installed(profile, &mkt_by_name)?;
    let dir_lookup = |n: &str| mkt_by_name.get(n).cloned().unwrap_or_default();
    provision::pin_marketplaces(&git::RealGit, profile, &installed_mkts, &dir_lookup, &mut lock, false)?;
    if let Some(parent) = lock_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    lock.save(lock_file)?;

    let installed = cli.list_plugins()?;
    let skills = enablement::scan_skills_dir(&paths.claude_skills_dir());
    let en = enablement::build(profile, &installed, &skills);
    print_enablement_warnings(&en);

    let args = launch::build_args(profile, &en, extra);
    launch::spawn(key, &args)
}

/// Fail fast with a clear error if a profile references a marketplace that
/// isn't in the installed-marketplace map, instead of letting `pin_marketplaces`
/// run `git checkout` against an empty directory and surface an opaque git error.
fn ensure_marketplaces_installed(
    profile: &profile::Profile,
    mkt_by_name: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> anyhow::Result<()> {
    for name in profile.marketplaces.keys() {
        if !mkt_by_name.contains_key(name) {
            anyhow::bail!("marketplace '{name}' is not installed (provisioning may have failed); cannot pin");
        }
    }
    Ok(())
}

fn installed_mkts_install_dirs(cli: &claude::RealClaude) -> anyhow::Result<std::collections::BTreeMap<String, std::path::PathBuf>> {
    // marketplace list --json includes installLocation; parse it here.
    let raw = cli.marketplace_list_raw()?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    let mut map = std::collections::BTreeMap::new();
    if let Some(arr) = v.as_array() {
        for m in arr {
            if let (Some(name), Some(loc)) = (m.get("name").and_then(|x| x.as_str()),
                                              m.get("installLocation").and_then(|x| x.as_str())) {
                map.insert(name.to_string(), std::path::PathBuf::from(loc));
            }
        }
    }
    Ok(map)
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
