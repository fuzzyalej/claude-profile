use crate::fs_paths::Paths;
use crate::git::{parse_repo_ref, GitCli};
use crate::profile::Profile;
use crate::vendor::{self, PluginSource};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const GIT_HOSTS: &[&str] = &["github.com", "gitlab.com", "bitbucket.org", "codeberg.org"];
const RUNNERS: &[&str] = &[
    "npx", "bunx", "uvx", "pipx", "node", "bun", "deno", "python", "python3", "uv",
];

pub struct Options {
    pub targets: Vec<String>,
    pub with: Vec<String>,
    pub save: Option<String>,
    pub dry_run: bool,
    pub command: Option<String>,
    pub env: Vec<String>,
    pub yes: bool,
    pub extra: Vec<String>,
}

/// One `try` target, classified from its spelling alone (no network, no filesystem).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Npm(String),
    Http(String),
    Plugin { name: String, marketplace: String },
    Repo(String),
    Local(String),
}

/// Classify a target. Rules, first match wins:
/// `npm:<pkg>` is an npm MCP server; `http:<url>` is an HTTP MCP server; an `http(s)://` URL
/// is a git repo when its host is a known git host or its path ends in `.git`, otherwise an
/// HTTP MCP server; `git@…` and other `scheme://` URLs are git repos; `.`, `./…`, `../…`,
/// `/…`, and `~/…` are local paths; `name@marketplace` without a `/` is a plugin;
/// `owner/repo[#ref]` is a GitHub repo.
pub fn parse_target(raw: &str) -> Result<Target> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("empty target");
    }
    if let Some(pkg) = s.strip_prefix("npm:") {
        if pkg.is_empty() {
            bail!("missing package name after 'npm:'");
        }
        return Ok(Target::Npm(pkg.to_string()));
    }
    if let Some(url) = s.strip_prefix("http:") {
        if url.contains("://") {
            return Ok(Target::Http(url.to_string()));
        }
    }
    if s.starts_with("https://") || s.starts_with("http://") {
        return Ok(if is_git_url(s) {
            Target::Repo(s.to_string())
        } else {
            Target::Http(s.to_string())
        });
    }
    if s.starts_with("git@") || s.contains("://") {
        return Ok(Target::Repo(s.to_string()));
    }
    if is_path_like(s) {
        return Ok(Target::Local(s.to_string()));
    }
    if !s.contains('/') {
        if let Some((name, marketplace)) = s.rsplit_once('@') {
            if !name.is_empty() && !marketplace.is_empty() {
                return Ok(Target::Plugin {
                    name: name.to_string(),
                    marketplace: marketplace.to_string(),
                });
            }
        }
    }
    if parse_repo_ref(s).is_ok() {
        return Ok(Target::Repo(s.to_string()));
    }
    bail!(
        "unrecognized target '{raw}': expected owner/repo[#ref], a git or MCP URL, npm:<package>, \
         http:<url>, plugin@marketplace, or a local path"
    )
}

/// Whether an `http(s)://` URL names a git repository rather than an MCP endpoint.
pub fn is_git_url(url: &str) -> bool {
    let without_ref = url.split('#').next().unwrap_or(url);
    if without_ref.trim_end_matches('/').ends_with(".git") {
        return true;
    }
    let host = url_host(without_ref);
    let host = host.strip_prefix("www.").unwrap_or(&host);
    GIT_HOSTS.contains(&host)
}

fn url_host(url: &str) -> String {
    let after = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

fn is_path_like(s: &str) -> bool {
    s == "."
        || s == ".."
        || s == "~"
        || ["./", "../", "/", "~/", ".\\", "..\\"]
            .iter()
            .any(|p| s.starts_with(p))
        || (s.len() > 2 && s.as_bytes()[1] == b':' && matches!(s.as_bytes()[2], b'\\' | b'/'))
}

/// What a fetched or local directory turned out to be.
#[derive(Debug, PartialEq)]
pub enum Detected {
    ProfileRepo,
    Marketplace {
        name: String,
        plugins: Vec<vendor::MarketplacePlugin>,
    },
    Plugin,
    RootSkill {
        name: String,
    },
    SkillsDir,
    Npm {
        package: String,
    },
    Unknown,
}

/// Detect a directory's kind. `fallback_name` names a marketplace or skill that doesn't name
/// itself.
pub fn detect(dir: &Path, fallback_name: &str) -> Result<Detected> {
    if is_profile_repo(dir) {
        return Ok(Detected::ProfileRepo);
    }
    let manifest = dir.join(".claude-plugin").join("marketplace.json");
    if manifest.is_file() {
        let body = std::fs::read_to_string(&manifest)?;
        let plugins = vendor::parse_marketplace_json(&body)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let name = serde_json::from_str::<Value>(&body)?
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| fallback_name.to_string());
        return Ok(Detected::Marketplace { name, plugins });
    }
    if dir.join(".claude-plugin").join("plugin.json").is_file() {
        return Ok(Detected::Plugin);
    }
    let root_skill = dir.join("SKILL.md");
    if root_skill.is_file() {
        let body = std::fs::read_to_string(&root_skill)?;
        let name = skill_name(&body).unwrap_or_else(|| fallback_name.to_string());
        return Ok(Detected::RootSkill { name });
    }
    if has_skills_dir(dir) {
        return Ok(Detected::SkillsDir);
    }
    if let Some(package) = npm_bin_package(dir)? {
        return Ok(Detected::Npm { package });
    }
    Ok(Detected::Unknown)
}

fn is_profile_repo(dir: &Path) -> bool {
    if dir.join("profile.json").is_file() {
        return true;
    }
    std::fs::read_dir(dir.join("profiles"))
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        })
        .unwrap_or(false)
}

fn has_skills_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir.join("skills"))
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().join("SKILL.md").is_file())
        })
        .unwrap_or(false)
}

fn skill_name(body: &str) -> Option<String> {
    let mut lines = body.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    lines
        .take_while(|l| l.trim() != "---")
        .find_map(|l| l.strip_prefix("name:"))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| !v.is_empty())
}

fn npm_bin_package(dir: &Path) -> Result<Option<String>> {
    let path = dir.join("package.json");
    if !path.is_file() {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)
        .with_context(|| format!("reading {}", path.display()))?;
    if v.get("bin").is_none() {
        return Ok(None);
    }
    match v.get("name").and_then(Value::as_str) {
        Some(name) if !name.is_empty() => Ok(Some(name.to_string())),
        _ => bail!("{} has a bin field but no package name", path.display()),
    }
}

/// The parts of a profile that `try` generates, before any `--with` base is merged in.
#[derive(Debug, Default)]
pub struct Draft {
    pub marketplaces: BTreeMap<String, String>,
    pub plugins: Vec<String>,
    pub plugin_dirs: Vec<String>,
    pub mcp: Map<String, Value>,
    stdio: Vec<String>,
    pub checkouts: Vec<(String, PathBuf)>,
}

impl Draft {
    fn add_mcp(&mut self, name: &str, server: Value, stdio: bool) -> Result<()> {
        match self.mcp.get(name) {
            Some(existing) if *existing != server => {
                bail!("two targets define MCP server '{name}' differently; try them separately")
            }
            Some(_) => {}
            None => {
                self.mcp.insert(name.to_string(), server);
                if stdio {
                    self.stdio.push(name.to_string());
                }
            }
        }
        Ok(())
    }

    fn add_marketplace(&mut self, name: &str, source: &str) -> Result<()> {
        match self.marketplaces.get(name) {
            Some(existing) if existing != source => {
                bail!("marketplace '{name}' comes from both {existing} and {source}; try them separately")
            }
            _ => {
                self.marketplaces
                    .insert(name.to_string(), source.to_string());
                Ok(())
            }
        }
    }

    fn add_plugin(&mut self, id: String) {
        if !self.plugins.contains(&id) {
            self.plugins.push(id);
        }
    }

    fn add_dir(&mut self, dir: &Path) {
        let d = dir.to_string_lossy().to_string();
        if !self.plugin_dirs.contains(&d) {
            self.plugin_dirs.push(d);
        }
    }

    fn apply_env(&mut self, vars: &[(String, String)]) -> Result<()> {
        if vars.is_empty() {
            return Ok(());
        }
        if self.stdio.is_empty() {
            bail!("--env needs a stdio MCP server target (npm:<package>, a repo with a package.json bin, or --command)");
        }
        for name in &self.stdio {
            let server = self
                .mcp
                .get_mut(name)
                .and_then(Value::as_object_mut)
                .expect("stdio server is an object");
            let env = server.entry("env").or_insert_with(|| json!({}));
            for (k, v) in vars {
                env[k] = json!(v);
            }
        }
        Ok(())
    }
}

/// Where target resolution may write and what it can look up.
pub struct Ctx<'a> {
    pub paths: &'a Paths,
    pub cwd: &'a Path,
    pub repos_root: PathBuf,
    pub known_marketplaces: BTreeMap<String, String>,
}

/// Resolve every target (and `--command`/`--env`) into a draft profile. Repo, path, npm, and
/// HTTP targets go first so a plugin target can refer to a marketplace tried in the same call.
pub fn resolve_targets<G: GitCli>(
    git: &G,
    targets: &[Target],
    command: Option<&str>,
    env: &[String],
    ctx: &Ctx,
) -> Result<Draft> {
    let vars = parse_env(env)?;
    let mut draft = Draft::default();
    for target in targets
        .iter()
        .filter(|t| !matches!(t, Target::Plugin { .. }))
    {
        match target {
            Target::Npm(pkg) => add_npm(&mut draft, pkg)?,
            Target::Http(url) => draft.add_mcp(
                &http_server_name(url),
                json!({"type": "http", "url": url}),
                false,
            )?,
            Target::Repo(spec) => {
                let dir = fetch_repo(git, spec, &ctx.repos_root)?;
                let name = parse_repo_ref(spec)?.repo;
                apply_dir(&mut draft, &dir, Some(spec), &name, ctx)?;
            }
            Target::Local(raw) => {
                let dir = local_dir(raw, ctx)?;
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "local".into());
                apply_dir(&mut draft, &dir, None, &name, ctx)?;
            }
            Target::Plugin { .. } => unreachable!(),
        }
    }
    for target in targets {
        if let Target::Plugin { name, marketplace } = target {
            add_marketplace_plugin(&mut draft, name, marketplace, ctx)?;
        }
    }
    if let Some(cmd) = command {
        let argv = split_command(cmd)?;
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("--command is empty"))?;
        draft.add_mcp(
            &command_server_name(&argv),
            json!({"command": program, "args": args}),
            true,
        )?;
    }
    draft.apply_env(&vars)?;
    if draft.mcp.is_empty() && draft.plugins.is_empty() && draft.plugin_dirs.is_empty() {
        bail!("nothing to try: give at least one target or --command");
    }
    Ok(draft)
}

fn add_npm(draft: &mut Draft, pkg: &str) -> Result<()> {
    let spec = if npm_has_version(pkg) {
        pkg.to_string()
    } else {
        format!("{pkg}@latest")
    };
    draft.add_mcp(
        &npm_server_name(pkg),
        json!({"command": "npx", "args": ["-y", spec]}),
        true,
    )
}

fn add_marketplace_plugin(
    draft: &mut Draft,
    name: &str,
    marketplace: &str,
    ctx: &Ctx,
) -> Result<()> {
    let source = draft
        .marketplaces
        .get(marketplace)
        .or_else(|| ctx.known_marketplaces.get(marketplace))
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown marketplace '{marketplace}' for '{name}@{marketplace}': no profile or search index \
                 defines it. Try the marketplace repo instead (claude-profile try owner/repo), or run \
                 `claude-profile find --sync` to refresh the index"
            )
        })?;
    draft.add_marketplace(marketplace, &source)?;
    draft.add_plugin(format!("{name}@{marketplace}"));
    Ok(())
}

fn apply_dir(
    draft: &mut Draft,
    dir: &Path,
    spec: Option<&str>,
    fallback_name: &str,
    ctx: &Ctx,
) -> Result<()> {
    let label = spec
        .map(str::to_string)
        .unwrap_or_else(|| dir.display().to_string());
    match detect(dir, fallback_name)? {
        Detected::ProfileRepo => bail!(
            "'{label}' is a claude-profile profile repo, not a plugin. Launch it directly: claude-profile {label}"
        ),
        Detected::Marketplace { name, plugins } => match spec {
            Some(spec) => {
                if let Some(known) = ctx.known_marketplaces.get(&name).filter(|k| k.as_str() != spec) {
                    bail!(
                        "'{spec}' is marketplace '{name}', but a marketplace with that name already comes from \
                         {known}; remove or rename one of them first"
                    );
                }
                draft.add_marketplace(&name, spec)?;
                for p in &plugins {
                    draft.add_plugin(format!("{}@{name}", p.name));
                }
                draft.checkouts.push((name, dir.to_path_buf()));
            }
            None => {
                for p in &plugins {
                    match &p.source {
                        PluginSource::RelativePath(rel) => draft.add_dir(&dir.join(rel.trim_start_matches("./"))),
                        _ => bail!(
                            "plugin '{}' in local marketplace {label} lives in another repo; try the marketplace \
                             from its git repo instead",
                            p.name
                        ),
                    }
                }
            }
        },
        Detected::Plugin => draft.add_dir(dir),
        Detected::RootSkill { name } => draft.add_dir(&skill_plugin_dir(dir, &name, spec.is_none(), ctx)?),
        Detected::SkillsDir => draft.add_dir(&skill_plugin_dir(dir, fallback_name, spec.is_none(), ctx)?),
        Detected::Npm { package } => add_npm(draft, &package)?,
        Detected::Unknown => bail!(
            "could not tell what '{label}' is: it has no .claude-plugin/marketplace.json, \
             .claude-plugin/plugin.json, SKILL.md, skills/*/SKILL.md, or package.json with a bin field. \
             For an MCP server published to npm, use npm:<package>; otherwise pass its start command with \
             --command \"<cmd …>\""
        ),
    }
    Ok(())
}

/// Make a skill folder loadable with `--plugin-dir` the same way vendored loose skills are: a
/// generated `.claude-plugin/plugin.json`. Local folders are copied first so the original stays
/// untouched.
fn skill_plugin_dir(dir: &Path, name: &str, local: bool, ctx: &Ctx) -> Result<PathBuf> {
    let target = if local {
        let dest = ctx.repos_root.join(name);
        remove_path(&dest)?;
        crate::vendor_fs::copy_dir_atomic(dir, &dest)?;
        dest
    } else {
        dir.to_path_buf()
    };
    crate::vendor_fs::ensure_manifest(&target, name)?;
    Ok(target)
}

fn fetch_repo<G: GitCli>(git: &G, spec: &str, repos_root: &Path) -> Result<PathBuf> {
    let dest = repos_root.join(parse_repo_ref(spec)?.pack_dir_name());
    remove_path(&dest)?;
    std::fs::create_dir_all(repos_root)?;
    crate::progress::step(&format!("cloning {spec}"));
    crate::pack::fetch_into(git, spec, &dest).with_context(|| format!("cloning {spec}"))?;
    crate::progress::done(&format!("cloned {spec}"));
    Ok(dest)
}

fn local_dir(raw: &str, ctx: &Ctx) -> Result<PathBuf> {
    let path = match raw.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            ctx.paths.home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => ctx.cwd.join(raw),
    };
    let dir = path
        .canonicalize()
        .with_context(|| format!("local path '{raw}' not found"))?;
    if !dir.is_dir() {
        bail!("local path '{raw}' is not a directory");
    }
    Ok(dir)
}

fn npm_has_version(pkg: &str) -> bool {
    let rest = if pkg.starts_with('@') {
        pkg.split_once('/').map(|(_, r)| r).unwrap_or("")
    } else {
        pkg
    };
    rest.contains('@')
}

/// MCP server name for an npm package: the package name without scope or version.
pub fn npm_server_name(pkg: &str) -> String {
    let unscoped = if pkg.starts_with('@') {
        pkg.split_once('/').map(|(_, r)| r).unwrap_or(pkg)
    } else {
        pkg
    };
    sanitize(unscoped.split('@').next().unwrap_or(unscoped))
}

/// MCP server name for an HTTP endpoint: its host, dots turned into dashes.
pub fn http_server_name(url: &str) -> String {
    let host = url_host(url);
    sanitize(host.strip_prefix("www.").unwrap_or(&host))
}

/// MCP server name for a command line: the program, or for runners like `npx` the first
/// non-flag argument.
pub fn command_server_name(argv: &[String]) -> String {
    let base = |s: &str| {
        let file = s.rsplit(['/', '\\']).next().unwrap_or(s).to_string();
        file.split_once('.')
            .map(|(stem, _)| stem.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(file)
    };
    let Some(program) = argv.first() else {
        return "command".into();
    };
    let program = base(program);
    if RUNNERS.contains(&program.as_str()) {
        if let Some(arg) = argv[1..].iter().find(|a| !a.starts_with('-')) {
            let name = if arg.contains('/') && !arg.starts_with('@') {
                base(arg)
            } else {
                npm_server_name(arg)
            };
            return sanitize(&name);
        }
    }
    sanitize(&program)
}

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "server".into()
    } else {
        out
    }
}

fn parse_env(vars: &[String]) -> Result<Vec<(String, String)>> {
    vars.iter()
        .map(|v| match v.split_once('=') {
            Some((k, val)) if !k.is_empty() => Ok((k.to_string(), val.to_string())),
            _ => bail!("--env expects KEY=VAL, got '{v}'"),
        })
        .collect()
}

/// Split a command line into words. Supports single quotes, double quotes, and backslash
/// escapes outside single quotes.
pub fn split_command(s: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => cur.push(c),
            (_, '\\') => {
                cur.push(
                    chars
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--command ends with a backslash"))?,
                );
                in_word = true;
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        bail!("unclosed quote in --command");
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// Build the profile `try` will launch: the draft on its own, or merged on top of the `--with`
/// base. The result is flat (no `extends`), so `--dry-run` shows exactly what runs.
pub fn build_profile(
    name: &str,
    description: &str,
    draft: &Draft,
    base: Option<(String, Profile)>,
) -> Result<Profile> {
    let mut generated = Profile {
        name: name.to_string(),
        description: Some(description.to_string()),
        author: None,
        extends: None,
        marketplaces: draft.marketplaces.clone(),
        plugins: draft.plugins.clone(),
        remove_plugins: Vec::new(),
        plugin_dirs: draft.plugin_dirs.clone(),
        mcp_servers: Value::Object(draft.mcp.clone()),
        bare: false,
    };
    let Some((base_key, base)) = base else {
        return Ok(generated);
    };
    generated.bare = base.bare;
    let mut combined =
        crate::combine::combine_profiles(&[(base_key, base), (name.to_string(), generated)])?;
    combined.name = name.to_string();
    combined.description = Some(description.to_string());
    Ok(combined)
}

/// The lines the confirmation prompt shows: everything that will be fetched or run.
pub fn plan_lines(profile: &Profile) -> Vec<String> {
    let mut out = Vec::new();
    for (name, source) in &profile.marketplaces {
        out.push(format!("  marketplace: {name} ({source})"));
    }
    for id in &profile.plugins {
        out.push(format!("  plugin:      {id}"));
    }
    for dir in &profile.plugin_dirs {
        out.push(format!("  plugin dir:  {dir}"));
    }
    if let Some(servers) = profile.mcp_servers.as_object() {
        for (name, server) in servers {
            out.push(format!(
                "  MCP server:  {name}: {}",
                describe_server(server)
            ));
        }
    }
    out
}

fn describe_server(server: &Value) -> String {
    let mut s = if let Some(url) = server.get("url").and_then(Value::as_str) {
        let kind = server.get("type").and_then(Value::as_str).unwrap_or("http");
        format!("{kind} {url}")
    } else {
        let mut words: Vec<String> = server
            .get("command")
            .and_then(Value::as_str)
            .map(|c| vec![c.to_string()])
            .unwrap_or_default();
        if let Some(args) = server.get("args").and_then(Value::as_array) {
            words.extend(args.iter().map(|a| {
                a.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| a.to_string())
            }));
        }
        words.join(" ")
    };
    if let Some(env) = server
        .get("env")
        .and_then(Value::as_object)
        .filter(|e| !e.is_empty())
    {
        let keys: Vec<&str> = env.keys().map(String::as_str).collect();
        s.push_str(&format!(" (env: {})", keys.join(", ")));
    }
    s
}

fn confirm(key: &str, profile: &Profile) -> bool {
    println!("claude-profile will start a throwaway session '{key}' with:");
    for line in plan_lines(profile) {
        println!("{line}");
    }
    println!("This fetches and runs third-party code on your machine.");
    print!("Proceed? [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes")
}

fn validate_save_name(name: &str, paths: &Paths) -> Result<PathBuf> {
    let valid = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid {
        bail!("invalid profile name '{name}': use letters, digits, '-', '_', or '.'");
    }
    let path = paths.user_profiles_dir().join(format!("{name}.json"));
    if path.exists() {
        bail!("profile '{name}' already exists at {}", path.display());
    }
    Ok(path)
}

fn known_marketplaces(
    paths: &Paths,
    cwd: &Path,
    env: Option<&Path>,
    bundled: &Path,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(body) = std::fs::read_to_string(paths.index_file()) {
        if let Ok(index) = serde_json::from_str::<crate::index::Index>(&body) {
            for e in index.entries {
                out.insert(e.marketplace, e.repo);
            }
        }
    }
    for (_, path, _) in crate::resolve::list_available(paths, cwd, env, bundled) {
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(p) = Profile::from_json_str(&body) else {
            continue;
        };
        out.extend(p.marketplaces);
    }
    out
}

fn new_session_id() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("try-{}", &id[..8])
}

/// A temporary session directory under `~/.claude-profiles/tmp/`, plus every path `try`
/// created elsewhere. Dropping it removes them all unless `keep` was called. The list is
/// mirrored to `session.json` so a later run can clean up after a crash.
pub struct Session {
    dir: PathBuf,
    created: Vec<PathBuf>,
}

impl Session {
    pub fn create(paths: &Paths, id: &str) -> Result<Session> {
        let dir = paths.try_sessions_dir().join(id);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let session = Session {
            dir,
            created: Vec::new(),
        };
        session.save()?;
        Ok(session)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn track(&mut self, path: PathBuf) -> Result<()> {
        if !self.created.contains(&path) {
            self.created.push(path);
        }
        self.save()
    }

    pub fn keep(&mut self) -> Result<()> {
        self.created.clear();
        self.save()
    }

    fn save(&self) -> Result<()> {
        let state = json!({ "pid": std::process::id(), "created": self.created });
        std::fs::write(
            self.dir.join("session.json"),
            serde_json::to_string_pretty(&state)?,
        )?;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for path in &self.created {
            let _ = remove_path(path);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn remove_path(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path)?,
        Ok(_) => std::fs::remove_file(path)?,
        Err(_) => {}
    }
    Ok(())
}

/// Remove sessions whose process is gone, with everything they recorded as created.
pub fn sweep_stale(paths: &Paths) {
    let Ok(entries) = std::fs::read_dir(paths.try_sessions_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let state: Option<Value> = std::fs::read_to_string(dir.join("session.json"))
            .ok()
            .and_then(|b| serde_json::from_str(&b).ok());
        let pid = state
            .as_ref()
            .and_then(|s| s.get("pid"))
            .and_then(Value::as_u64);
        if !is_stale(&dir, pid) {
            continue;
        }
        if let Some(created) = state
            .as_ref()
            .and_then(|s| s.get("created"))
            .and_then(Value::as_array)
        {
            for p in created.iter().filter_map(Value::as_str) {
                let _ = remove_path(Path::new(p));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(unix)]
fn is_stale(_dir: &Path, pid: Option<u64>) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    match pid.and_then(|p| i32::try_from(p).ok()) {
        Some(p) if p as u32 == std::process::id() => false,
        Some(p) => unsafe { kill(p, 0) != 0 },
        None => true,
    }
}

#[cfg(not(unix))]
fn is_stale(dir: &Path, _pid: Option<u64>) -> bool {
    std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > std::time::Duration::from_secs(24 * 60 * 60))
}

fn list_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

pub fn run(
    opts: &Options,
    paths: &Paths,
    cwd: &Path,
    env: Option<&Path>,
    bundled: &Path,
) -> Result<i32> {
    if opts.targets.is_empty() && opts.command.is_none() {
        bail!("nothing to try: give at least one target or --command");
    }
    let targets = opts
        .targets
        .iter()
        .map(|t| parse_target(t))
        .collect::<Result<Vec<_>>>()?;
    let save_path = opts
        .save
        .as_deref()
        .map(|n| validate_save_name(n, paths))
        .transpose()?;

    sweep_stale(paths);
    let id = new_session_id();
    let key = opts.save.clone().unwrap_or_else(|| id.clone());
    let mut session = Session::create(paths, &id)?;
    let repos_root = if opts.save.is_some() {
        let root = paths.store_dir().join(&key).join("repos");
        session.track(root.clone())?;
        root
    } else {
        session.dir().join("repos")
    };

    let ctx = Ctx {
        paths,
        cwd,
        repos_root,
        known_marketplaces: known_marketplaces(paths, cwd, env, bundled),
    };
    let draft = resolve_targets(
        &crate::git::RealGit,
        &targets,
        opts.command.as_deref(),
        &opts.env,
        &ctx,
    )?;
    let base = if opts.with.is_empty() {
        None
    } else {
        let t = crate::resolve_launch(&opts.with, paths, cwd, env, bundled)?;
        Some((t.key, t.profile))
    };
    let mut described: Vec<String> = opts.targets.clone();
    if let Some(cmd) = &opts.command {
        described.push(format!("--command \"{cmd}\""));
    }
    let profile = build_profile(&key, &format!("try: {}", described.join(" ")), &draft, base)?;

    if opts.dry_run {
        crate::progress::clear();
        println!("{}", serde_json::to_string_pretty(&profile)?);
        return Ok(0);
    }

    if !opts.yes && !crate::progress::suspend(|| confirm(&key, &profile)) {
        bail!("declined by user");
    }

    for (name, checkout) in &draft.checkouts {
        let dir = paths.marketplace_clone_dir(name);
        if dir.exists() {
            continue;
        }
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::rename(checkout, &dir).is_ok() && opts.save.is_none() {
            session.track(dir)?;
        }
    }
    let marketplaces_dir = paths.store_dir().join("marketplaces");
    let external_dir = marketplaces_dir.join("_external");
    let before: Vec<PathBuf> = list_dirs(&marketplaces_dir)
        .into_iter()
        .chain(list_dirs(&external_dir))
        .collect();
    let lock_file = match &save_path {
        Some(path) => path.with_extension("lock"),
        None => paths.locks_dir().join(format!("{key}.lock")),
    };
    session.track(paths.store_dir().join(&key))?;
    session.track(lock_file.clone())?;

    let provisioned = crate::provision_and_pin(&profile, &key, &lock_file, true, cwd, paths);
    if opts.save.is_none() {
        for dir in list_dirs(&marketplaces_dir)
            .into_iter()
            .chain(list_dirs(&external_dir))
        {
            if !before.contains(&dir) && dir != external_dir {
                session.track(dir)?;
            }
        }
    }
    provisioned?;

    if let Some(path) = &save_path {
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&profile)?),
        )?;
        session.keep()?;
        crate::progress::clear();
        eprintln!("saved profile '{key}' at {}", path.display());
    }

    let args = crate::launch::build_args(&profile, &key, paths, &opts.extra)?;
    crate::progress::clear();
    crate::launch::run_and_wait(&key, &args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;

    struct FixtureGit {
        fixtures: PathBuf,
        cloned: RefCell<Vec<String>>,
    }

    impl GitCli for FixtureGit {
        fn clone(&self, url: &str, dest: &Path) -> Result<()> {
            self.cloned.borrow_mut().push(url.to_string());
            let name = url.trim_end_matches(".git").rsplit('/').next().unwrap();
            crate::vendor_fs::copy_dir_atomic(&self.fixtures.join(name), dest)
        }
        fn pull(&self, _r: &Path) -> Result<()> {
            Ok(())
        }
        fn head_sha(&self, _r: &Path) -> Result<String> {
            Ok("sha".into())
        }
        fn checkout(&self, _r: &Path, _g: &str) -> Result<()> {
            Ok(())
        }
        fn is_repo(&self, _r: &Path) -> bool {
            true
        }
        fn sparse_fetch(&self, _u: &str, _d: &Path, _s: &str) -> Result<()> {
            Ok(())
        }
    }

    struct Fixture {
        tmp: tempfile::TempDir,
        paths: Paths,
        git: FixtureGit,
    }

    impl Fixture {
        fn new() -> Fixture {
            let tmp = tempfile::tempdir().unwrap();
            let paths = Paths::from_home(tmp.path().join("home"));
            let git = FixtureGit {
                fixtures: tmp.path().join("fixtures"),
                cloned: RefCell::new(vec![]),
            };
            Fixture { tmp, paths, git }
        }

        fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let dir = self.git.fixtures.join(name);
            write_files(&dir, files);
            dir
        }

        fn ctx(&self) -> Ctx<'_> {
            Ctx {
                paths: &self.paths,
                cwd: self.tmp.path(),
                repos_root: self.tmp.path().join("repos"),
                known_marketplaces: BTreeMap::new(),
            }
        }

        fn resolve(&self, targets: &[&str]) -> Result<Draft> {
            self.resolve_with(targets, None, &[], self.ctx())
        }

        fn resolve_with(
            &self,
            targets: &[&str],
            command: Option<&str>,
            env: &[&str],
            ctx: Ctx,
        ) -> Result<Draft> {
            let parsed: Vec<Target> = targets.iter().map(|t| parse_target(t).unwrap()).collect();
            let env: Vec<String> = env.iter().map(|s| s.to_string()).collect();
            resolve_targets(&self.git, &parsed, command, &env, &ctx)
        }
    }

    fn write_files(dir: &Path, files: &[(&str, &str)]) {
        fs::create_dir_all(dir).unwrap();
        for (rel, body) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
    }

    #[test]
    fn parses_npm_targets() {
        assert_eq!(
            parse_target("npm:@mobilenext/mobile-mcp").unwrap(),
            Target::Npm("@mobilenext/mobile-mcp".into())
        );
        assert!(parse_target("npm:").is_err());
    }

    #[test]
    fn parses_git_and_mcp_urls() {
        assert_eq!(
            parse_target("https://github.com/o/r").unwrap(),
            Target::Repo("https://github.com/o/r".into())
        );
        assert_eq!(
            parse_target("https://gitlab.com/o/r#v1").unwrap(),
            Target::Repo("https://gitlab.com/o/r#v1".into())
        );
        assert_eq!(
            parse_target("https://git.example.com/o/r.git").unwrap(),
            Target::Repo("https://git.example.com/o/r.git".into())
        );
        assert_eq!(
            parse_target("git@github.com:o/r.git").unwrap(),
            Target::Repo("git@github.com:o/r.git".into())
        );
        assert_eq!(
            parse_target("https://mcp.example.com/mcp").unwrap(),
            Target::Http("https://mcp.example.com/mcp".into())
        );
        assert_eq!(
            parse_target("http://localhost:3000/mcp").unwrap(),
            Target::Http("http://localhost:3000/mcp".into())
        );
        assert_eq!(
            parse_target("http:https://github.com/mcp").unwrap(),
            Target::Http("https://github.com/mcp".into())
        );
    }

    #[test]
    fn git_url_rule_uses_known_hosts_or_dot_git() {
        assert!(is_git_url("https://github.com/o/r"));
        assert!(is_git_url("https://www.github.com/o/r"));
        assert!(is_git_url("https://bitbucket.org/o/r"));
        assert!(is_git_url("https://codeberg.org/o/r"));
        assert!(is_git_url("https://git.example.com/o/r.git#main"));
        assert!(!is_git_url("https://api.githubcopilot.com/mcp/"));
        assert!(!is_git_url("https://mcp.example.com/sse"));
    }

    #[test]
    fn parses_plugin_repo_and_local_targets() {
        assert_eq!(
            parse_target("superpowers@superpowers-marketplace").unwrap(),
            Target::Plugin {
                name: "superpowers".into(),
                marketplace: "superpowers-marketplace".into()
            }
        );
        assert_eq!(
            parse_target("obra/superpowers#v1").unwrap(),
            Target::Repo("obra/superpowers#v1".into())
        );
        assert_eq!(
            parse_target("./my-plugin").unwrap(),
            Target::Local("./my-plugin".into())
        );
        assert_eq!(
            parse_target("/abs/dir").unwrap(),
            Target::Local("/abs/dir".into())
        );
        assert_eq!(
            parse_target("~/skills/x").unwrap(),
            Target::Local("~/skills/x".into())
        );
        assert_eq!(parse_target(".").unwrap(), Target::Local(".".into()));
        assert!(parse_target("justaword").is_err());
        assert!(parse_target("a/b/c").is_err());
    }

    #[test]
    fn detects_each_layout() {
        let f = Fixture::new();
        let mkt = f.repo(
            "mkt",
            &[(
                ".claude-plugin/marketplace.json",
                r#"{"name":"m","plugins":[{"name":"a","source":"./a"}]}"#,
            )],
        );
        let plugin = f.repo("plug", &[(".claude-plugin/plugin.json", r#"{"name":"p"}"#)]);
        let skill = f.repo(
            "skill",
            &[("SKILL.md", "---\nname: \"tidy\"\ndescription: x\n---\nbody")],
        );
        let skills = f.repo("skills", &[("skills/one/SKILL.md", "# one")]);
        let npm = f.repo(
            "npm",
            &[("package.json", r#"{"name":"@a/srv","bin":{"srv":"x.js"}}"#)],
        );
        let lib = f.repo("lib", &[("package.json", r#"{"name":"lib"}"#)]);
        let prof = f.repo(
            "prof",
            &[
                ("profiles/x.json", r#"{"name":"x"}"#),
                (".claude-plugin/plugin.json", "{}"),
            ],
        );

        assert!(
            matches!(detect(&mkt, "fallback").unwrap(), Detected::Marketplace { ref name, ref plugins } if name == "m" && plugins.len() == 1)
        );
        assert_eq!(detect(&plugin, "f").unwrap(), Detected::Plugin);
        assert_eq!(
            detect(&skill, "f").unwrap(),
            Detected::RootSkill {
                name: "tidy".into()
            }
        );
        assert_eq!(detect(&skills, "f").unwrap(), Detected::SkillsDir);
        assert_eq!(
            detect(&npm, "f").unwrap(),
            Detected::Npm {
                package: "@a/srv".into()
            }
        );
        assert_eq!(detect(&lib, "f").unwrap(), Detected::Unknown);
        assert_eq!(detect(&prof, "f").unwrap(), Detected::ProfileRepo);
    }

    #[test]
    fn marketplace_name_falls_back_to_repo_name() {
        let f = Fixture::new();
        let mkt = f.repo(
            "mkt",
            &[(".claude-plugin/marketplace.json", r#"{"plugins":[]}"#)],
        );
        assert!(
            matches!(detect(&mkt, "repo-name").unwrap(), Detected::Marketplace { ref name, .. } if name == "repo-name")
        );
    }

    #[test]
    fn npm_target_becomes_npx_stdio_server() {
        let f = Fixture::new();
        let d = f.resolve(&["npm:@mobilenext/mobile-mcp"]).unwrap();
        assert_eq!(
            d.mcp["mobile-mcp"],
            json!({"command":"npx","args":["-y","@mobilenext/mobile-mcp@latest"]})
        );
        let d = f.resolve(&["npm:foo@1.2.3"]).unwrap();
        assert_eq!(
            d.mcp["foo"],
            json!({"command":"npx","args":["-y","foo@1.2.3"]})
        );
    }

    #[test]
    fn http_target_becomes_http_server() {
        let f = Fixture::new();
        let d = f.resolve(&["https://mcp.example.com/mcp"]).unwrap();
        assert_eq!(
            d.mcp["mcp-example-com"],
            json!({"type":"http","url":"https://mcp.example.com/mcp"})
        );
    }

    #[test]
    fn marketplace_repo_loads_all_plugins() {
        let f = Fixture::new();
        f.repo("mkt", &[(".claude-plugin/marketplace.json", r#"{"name":"m","plugins":[{"name":"a","source":"./a"},{"name":"b","source":"./b"}]}"#)]);
        let d = f.resolve(&["o/mkt#v1"]).unwrap();
        assert_eq!(d.marketplaces.get("m").unwrap(), "o/mkt#v1");
        assert_eq!(d.plugins, vec!["a@m", "b@m"]);
        assert_eq!(d.checkouts.len(), 1);
        assert_eq!(
            f.git.cloned.borrow().as_slice(),
            &["https://github.com/o/mkt.git".to_string()]
        );
    }

    #[test]
    fn marketplace_name_clash_with_known_source_errors() {
        let f = Fixture::new();
        f.repo(
            "mkt",
            &[(
                ".claude-plugin/marketplace.json",
                r#"{"name":"m","plugins":[]}"#,
            )],
        );
        let mut ctx = f.ctx();
        ctx.known_marketplaces
            .insert("m".into(), "other/repo".into());
        let err = f
            .resolve_with(&["o/mkt"], None, &[], ctx)
            .unwrap_err()
            .to_string();
        assert!(err.contains("other/repo"), "{err}");
    }

    #[test]
    fn plugin_repo_loads_as_plugin_dir() {
        let f = Fixture::new();
        f.repo("plug", &[(".claude-plugin/plugin.json", r#"{"name":"p"}"#)]);
        let d = f.resolve(&["o/plug"]).unwrap();
        let expected = f.tmp.path().join("repos").join("o--plug");
        assert_eq!(d.plugin_dirs, vec![expected.to_string_lossy().to_string()]);
        assert!(expected.join(".claude-plugin/plugin.json").is_file());
    }

    #[test]
    fn skill_repo_gets_generated_manifest() {
        let f = Fixture::new();
        f.repo("skill", &[("SKILL.md", "---\nname: tidy\n---\n")]);
        let d = f.resolve(&["o/skill"]).unwrap();
        let dir = PathBuf::from(&d.plugin_dirs[0]);
        let manifest: Value = serde_json::from_str(
            &fs::read_to_string(dir.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], "tidy");
    }

    #[test]
    fn local_skill_is_copied_not_modified() {
        let f = Fixture::new();
        let local = f.tmp.path().join("my-skill");
        write_files(&local, &[("SKILL.md", "# s")]);
        let d = f.resolve(&["./my-skill"]).unwrap();
        let dir = PathBuf::from(&d.plugin_dirs[0]);
        assert!(dir.starts_with(f.tmp.path().join("repos")));
        assert!(dir.join(".claude-plugin/plugin.json").is_file());
        assert!(!local.join(".claude-plugin").exists());
    }

    #[test]
    fn local_plugin_dir_is_used_in_place() {
        let f = Fixture::new();
        let local = f.tmp.path().join("plug");
        write_files(&local, &[(".claude-plugin/plugin.json", "{}")]);
        let d = f.resolve(&["./plug"]).unwrap();
        assert_eq!(
            d.plugin_dirs,
            vec![local.canonicalize().unwrap().to_string_lossy().to_string()]
        );
    }

    #[test]
    fn local_marketplace_loads_relative_plugins_as_dirs() {
        let f = Fixture::new();
        let local = f.tmp.path().join("mkt");
        write_files(
            &local,
            &[
                (
                    ".claude-plugin/marketplace.json",
                    r#"{"name":"m","plugins":[{"name":"a","source":"./plugins/a"}]}"#,
                ),
                ("plugins/a/.claude-plugin/plugin.json", "{}"),
            ],
        );
        let d = f.resolve(&["./mkt"]).unwrap();
        assert!(d.marketplaces.is_empty());
        assert!(Path::new(&d.plugin_dirs[0]).ends_with(Path::new("plugins").join("a")));
    }

    #[test]
    fn npm_repo_becomes_npx_server() {
        let f = Fixture::new();
        f.repo(
            "srv",
            &[("package.json", r#"{"name":"@acme/srv","bin":"cli.js"}"#)],
        );
        let d = f.resolve(&["o/srv"]).unwrap();
        assert_eq!(
            d.mcp["srv"],
            json!({"command":"npx","args":["-y","@acme/srv@latest"]})
        );
    }

    #[test]
    fn unknown_repo_suggests_npm_or_command() {
        let f = Fixture::new();
        f.repo("lib", &[("README.md", "hi")]);
        let err = f.resolve(&["o/lib"]).unwrap_err().to_string();
        assert!(
            err.contains("npm:<package>") && err.contains("--command"),
            "{err}"
        );
    }

    #[test]
    fn profile_repo_suggests_direct_launch() {
        let f = Fixture::new();
        f.repo("prof", &[("profile.json", r#"{"name":"x"}"#)]);
        let err = f.resolve(&["o/prof"]).unwrap_err().to_string();
        assert!(
            err.contains("profile repo") && err.contains("claude-profile o/prof"),
            "{err}"
        );
    }

    #[test]
    fn plugin_target_needs_known_marketplace() {
        let f = Fixture::new();
        let err = f.resolve(&["a@nowhere"]).unwrap_err().to_string();
        assert!(err.contains("unknown marketplace 'nowhere'"), "{err}");

        let mut ctx = f.ctx();
        ctx.known_marketplaces.insert("m".into(), "o/m".into());
        let d = f.resolve_with(&["a@m"], None, &[], ctx).unwrap();
        assert_eq!(d.marketplaces.get("m").unwrap(), "o/m");
        assert_eq!(d.plugins, vec!["a@m"]);
    }

    #[test]
    fn plugin_target_can_use_marketplace_tried_alongside() {
        let f = Fixture::new();
        f.repo(
            "mkt",
            &[(
                ".claude-plugin/marketplace.json",
                r#"{"name":"m","plugins":[{"name":"a","source":"./a"}]}"#,
            )],
        );
        let d = f.resolve(&["a@m", "o/mkt"]).unwrap();
        assert_eq!(d.marketplaces.get("m").unwrap(), "o/mkt");
    }

    #[test]
    fn several_targets_combine() {
        let f = Fixture::new();
        let d = f
            .resolve(&["npm:a", "https://mcp.example.com/mcp"])
            .unwrap();
        assert_eq!(d.mcp.len(), 2);
    }

    #[test]
    fn command_and_env_build_stdio_server() {
        let f = Fixture::new();
        let d = f
            .resolve_with(
                &["https://mcp.example.com/mcp"],
                Some("uvx my-server --port 3"),
                &["TOKEN=abc"],
                f.ctx(),
            )
            .unwrap();
        assert_eq!(
            d.mcp["my-server"],
            json!({"command":"uvx","args":["my-server","--port","3"],"env":{"TOKEN":"abc"}})
        );
        assert!(d.mcp["mcp-example-com"].get("env").is_none());
    }

    #[test]
    fn env_without_stdio_server_errors() {
        let f = Fixture::new();
        assert!(f
            .resolve_with(&["https://mcp.example.com/mcp"], None, &["A=1"], f.ctx())
            .is_err());
        assert!(f
            .resolve_with(&["npm:a"], None, &["NOEQUALS"], f.ctx())
            .is_err());
    }

    #[test]
    fn splits_command_lines() {
        assert_eq!(
            split_command(r#"node "my server.js" --flag='a b' x\ y"#).unwrap(),
            vec!["node", "my server.js", "--flag=a b", "x y"]
        );
        assert_eq!(split_command("  a   b ").unwrap(), vec!["a", "b"]);
        assert_eq!(split_command(r#"a """#).unwrap(), vec!["a", ""]);
        assert!(split_command("a 'b").is_err());
    }

    #[test]
    fn names_servers() {
        assert_eq!(npm_server_name("@mobilenext/mobile-mcp"), "mobile-mcp");
        assert_eq!(npm_server_name("foo@2"), "foo");
        assert_eq!(
            http_server_name("https://www.mcp.example.com:8443/x"),
            "mcp-example-com"
        );
        let argv = |s: &str| split_command(s).unwrap();
        assert_eq!(command_server_name(&argv("npx -y @scope/thing@1")), "thing");
        assert_eq!(
            command_server_name(&argv("/usr/local/bin/my-srv --stdio")),
            "my-srv"
        );
        assert_eq!(command_server_name(&argv("node ./dist/index.js")), "index");
    }

    #[test]
    fn builds_profile_on_base() {
        let f = Fixture::new();
        let d = f.resolve(&["npm:a"]).unwrap();
        let base = Profile::from_json_str(
            r#"{"name":"base","plugins":["x@m"],"marketplaces":{"m":"o/m"},"bare":true}"#,
        )
        .unwrap();
        let p = build_profile("try-1", "try: npm:a", &d, Some(("base".into(), base))).unwrap();
        assert_eq!(p.name, "try-1");
        assert_eq!(p.description.as_deref(), Some("try: npm:a"));
        assert_eq!(p.plugins, vec!["x@m"]);
        assert!(p.mcp_servers.get("a").is_some());
        assert!(p.bare);
    }

    #[test]
    fn plan_lines_show_commands_and_urls() {
        let p = Profile::from_json_str(r#"{"name":"t","plugins":["a@m"],"marketplaces":{"m":"o/m"},"pluginDirs":["/d"],
            "mcpServers":{"s":{"command":"npx","args":["-y","pkg@latest"],"env":{"K":"secret"}},"h":{"type":"http","url":"https://x/mcp"}}}"#).unwrap();
        let lines = plan_lines(&p).join("\n");
        assert!(lines.contains("marketplace: m (o/m)"));
        assert!(lines.contains("plugin:      a@m"));
        assert!(lines.contains("plugin dir:  /d"));
        assert!(lines.contains("s: npx -y pkg@latest (env: K)"));
        assert!(!lines.contains("secret"));
        assert!(lines.contains("h: http https://x/mcp"));
    }

    #[test]
    fn save_name_is_validated_and_never_overwrites() {
        let f = Fixture::new();
        assert!(validate_save_name("../x", &f.paths).is_err());
        assert!(validate_save_name("ok-name", &f.paths).is_ok());
        write_files(&f.paths.user_profiles_dir(), &[("taken.json", "{}")]);
        assert!(validate_save_name("taken", &f.paths)
            .unwrap_err()
            .to_string()
            .contains("already exists"));
    }

    #[test]
    fn session_removes_tracked_paths_on_drop_unless_kept() {
        let f = Fixture::new();
        let tracked = f.tmp.path().join("store-entry");
        fs::create_dir_all(&tracked).unwrap();
        let dir = {
            let mut s = Session::create(&f.paths, "try-a").unwrap();
            s.track(tracked.clone()).unwrap();
            s.dir().to_path_buf()
        };
        assert!(!dir.exists());
        assert!(!tracked.exists());

        fs::create_dir_all(&tracked).unwrap();
        {
            let mut s = Session::create(&f.paths, "try-b").unwrap();
            s.track(tracked.clone()).unwrap();
            s.keep().unwrap();
        }
        assert!(tracked.exists());
    }

    #[test]
    fn sweep_removes_sessions_of_dead_processes() {
        let f = Fixture::new();
        let tracked = f.tmp.path().join("leftover");
        fs::create_dir_all(&tracked).unwrap();
        let dead = f.paths.try_sessions_dir().join("try-dead");
        fs::create_dir_all(&dead).unwrap();
        let state = json!({"pid": u32::MAX / 2, "created": [tracked]});
        fs::write(dead.join("session.json"), state.to_string()).unwrap();
        let live = Session::create(&f.paths, "try-live").unwrap();

        sweep_stale(&f.paths);

        if cfg!(unix) {
            assert!(!dead.exists());
            assert!(!tracked.exists());
        }
        assert!(live.dir().exists());
    }
}
