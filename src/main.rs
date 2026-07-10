use clap::{Parser, Subcommand};
use claude::ClaudeCli;
use std::path::PathBuf;

mod claude;
mod commands;
mod enablement;
mod extends;
mod fs_paths;
mod git;
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
    /// Profile name to launch (when no subcommand is given).
    profile: Option<String>,
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
    env: Option<&std::path::Path>, examples: &std::path::Path,
) -> ProfilesForRefmap {
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for (name, path, _src) in resolve::list_available(paths, cwd, env, examples) {
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

fn examples_dir() -> PathBuf {
    // examples/ ships next to the binary in dev; fall back to CARGO_MANIFEST_DIR at build time.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

fn env_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_PROFILE_DIR").map(PathBuf::from)
}

fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    let paths = fs_paths::Paths::detect()?;
    let cwd = std::env::current_dir()?;
    let examples = examples_dir();
    let env = env_dir();

    match cli.command {
        Some(Command::List) => {
            commands::list::run(&paths, &cwd, env.as_deref(), &examples)?;
            Ok(0)
        }
        Some(Command::Install { spec }) => {
            let dir = pack::install_pack(&git::RealGit, &spec, &paths)?;
            println!("installed pack at {}", dir.display());
            Ok(0)
        }
        Some(Command::Update { frozen }) => {
            handle_update(frozen, &paths, &cwd, env.as_deref(), &examples)?;
            Ok(0)
        }
        Some(Command::New { name }) => {
            commands::new::run(&name, &paths)?;
            Ok(0)
        }
        Some(Command::Test { target, json, extra }) => commands::test::run(&target, json, &extra),
        Some(Command::SelfUninstall { purge }) => {
            let (profiles, _failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &examples);
            let refmap = refmap::build_refmap(&profiles);
            let referenced: Vec<String> = refmap.plugin_refs.keys().cloned().collect();
            commands::self_uninstall::run(&paths, purge, &referenced)?;
            Ok(0)
        }
        Some(Command::Status) => {
            let (profiles, failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &examples);
            for (path, err) in &failed {
                eprintln!("warning: could not parse profile {}: {err}", path.display());
            }
            commands::status::run(&claude::RealClaude::new(), &profiles)?;
            Ok(0)
        }
        Some(Command::Gc { dry_run }) => {
            let (profiles, failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &examples);
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
            let plan = commands::remove::remove_target(&target, &paths, &cwd, env.as_deref(), &examples)?;
            commands::remove::apply(&plan)?;
            println!("removed {target}");
            if prune {
                let (profiles, failed) = profiles_for_refmap(&paths, &cwd, env.as_deref(), &examples);
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
        None => {
            let name = cli.profile
                .ok_or_else(|| anyhow::anyhow!("no profile given (try `claude-profile list`)"))?;
            // owner/repo sugar: install-if-needed, then launch its default profile.
            if name.contains('/') {
                let dir = pack::install_pack(&git::RealGit, &name, &paths)?;
                let default = pack::default_profile_name(&dir)?;
                return launch_profile(&default, cli.yes, &cli.extra, &paths, &cwd, env.as_deref(), &examples);
            }
            launch_profile(&name, cli.yes, &cli.extra, &paths, &cwd, env.as_deref(), &examples)
        }
    }
}

fn handle_update(
    frozen: bool,
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    examples: &std::path::Path,
) -> anyhow::Result<()> {
    let updated = pack::update_all_packs(&git::RealGit, paths)?;
    for name in &updated { println!("updated pack {name}"); }
    let (profiles, failed) = profiles_for_refmap(paths, cwd, env, examples);
    for (path, err) in &failed {
        eprintln!("warning: skipping unparseable profile {}: {err}", path.display());
    }
    let cli = claude::RealClaude::new();
    let mkt_dirs = installed_mkts_install_dirs(&cli)?;
    let dir_lookup = |n: &str| mkt_dirs.get(n).cloned().unwrap_or_default();
    if frozen {
        let mut triples = Vec::new();
        for (name, profile) in &profiles {
            let Some(profile) = resolve_extends_or_warn(name, profile, paths, cwd, env, examples) else { continue };
            let resolved = resolve::resolve(name, paths, cwd, env, examples)?;
            let lp = lock::lock_path(name, &resolved.path, &resolved.source, paths);
            let lf = lock::Lockfile::load(&lp)?.unwrap_or_else(|| lock::Lockfile::new(name));
            triples.push((name.clone(), profile, lf));
        }
        commands::update::frozen_check(&triples)?;
        println!("--frozen: all locks up to date");
    } else {
        for (name, profile) in &profiles {
            let Some(profile) = resolve_extends_or_warn(name, profile, paths, cwd, env, examples) else { continue };
            let missing = profile.marketplaces.keys().find(|mkt| !mkt_dirs.contains_key(mkt.as_str()));
            if let Some(mkt) = missing {
                eprintln!(
                    "skipping '{name}': marketplace '{mkt}' not installed (launch the profile once to provision it)"
                );
                continue;
            }
            let resolved = resolve::resolve(name, paths, cwd, env, examples)?;
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
    examples: &std::path::Path,
) -> Option<profile::Profile> {
    match extends::resolve_extends(profile.clone(), &|parent| {
        Ok(resolve::resolve(parent, paths, cwd, env, examples)?.profile)
    }) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("warning: skipping '{name}': could not resolve extends: {e}");
            None
        }
    }
}

fn launch_profile(
    name: &str,
    assume_yes: bool,
    extra: &[String],
    paths: &fs_paths::Paths,
    cwd: &std::path::Path,
    env: Option<&std::path::Path>,
    examples: &std::path::Path,
) -> anyhow::Result<i32> {
    let resolved = resolve::resolve(name, paths, cwd, env, examples)?;
    let source = resolved.source.clone();
    let profile_path = resolved.path.clone();
    let profile = resolved.profile;
    let profile = extends::resolve_extends(profile, &|parent| {
        Ok(resolve::resolve(parent, paths, cwd, env, examples)?.profile)
    })?;

    let cli = claude::RealClaude::new();
    provision::provision(&cli, &profile, assume_yes)?;

    // --- pinning ---
    let lock_file = lock::lock_path(name, &profile_path, &source, paths);
    let mut lock = lock::Lockfile::load(&lock_file)?.unwrap_or_else(|| lock::Lockfile::new(name));
    let installed_mkts = cli.list_marketplaces()?;
    let mkt_by_name: std::collections::BTreeMap<String, std::path::PathBuf> =
        installed_mkts_install_dirs(&cli)?;
    ensure_marketplaces_installed(&profile, &mkt_by_name)?;
    let dir_lookup = |n: &str| mkt_by_name.get(n).cloned().unwrap_or_default();
    provision::pin_marketplaces(&git::RealGit, &profile, &installed_mkts, &dir_lookup, &mut lock, false)?;
    lock.save(&lock_file)?;
    // --- end pinning ---

    let installed = cli.list_plugins()?;
    let skills = enablement::scan_skills_dir(&paths.claude_skills_dir());
    let en = enablement::build(&profile, &installed, &skills);

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

    let args = launch::build_args(&profile, &en, extra);
    launch::spawn(name, &args)
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
