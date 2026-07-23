use crate::fs_paths::Paths;
use crate::git::{parse_repo_ref, RealGit};
use crate::profile::Profile;
use crate::{extends, pack, resolve};
use std::collections::BTreeSet;
use std::io::IsTerminal;
use std::path::Path;

const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

fn use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// One installed-vs-new line: dim + "(installed)" when present, green "+ …(new)" otherwise.
fn item_line(installed: bool, text: &str, color: bool) -> String {
    match (installed, color) {
        (true, true) => format!("    {DIM}{text}  (installed){RESET}"),
        (true, false) => format!("    {text}  (installed)"),
        (false, true) => format!("  {GREEN}+ {text}  (new){RESET}"),
        (false, false) => format!("  + {text}  (new)"),
    }
}

fn heading(text: &str, color: bool) -> String {
    if color { format!("{BOLD}{text}{RESET}") } else { text.to_string() }
}

/// Render everything `show` prints. Pure: no I/O, so it is unit-tested directly.
pub fn format_show(
    profile: &Profile,
    author: Option<&str>,
    source_label: &str,
    installed_plugins: &BTreeSet<String>,
    installed_mkts: &BTreeSet<String>,
    color: bool,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", heading(&profile.name, color)));
    out.push_str(&format!(
        "  {}\n\n",
        profile.description.as_deref().unwrap_or("(no description)")
    ));
    out.push_str(&format!("  Author:  {}\n", author.unwrap_or("—")));
    out.push_str(&format!("  Source:  {source_label}\n"));
    if let Some(ext) = &profile.extends {
        out.push_str(&format!("  Extends: {ext} (not expanded)\n"));
    }
    if profile.bare {
        out.push_str("  Mode:    bare (API-key isolation)\n");
    }

    out.push_str(&format!("\n{}\n", heading(&format!("Marketplaces ({})", profile.marketplaces.len()), color)));
    for (name, src) in &profile.marketplaces {
        let text = format!("{name}  ({src})");
        out.push_str(&item_line(installed_mkts.contains(name), &text, color));
        out.push('\n');
    }

    out.push_str(&format!("{}\n", heading(&format!("Plugins ({})", profile.plugins.len()), color)));
    for id in &profile.plugins {
        out.push_str(&item_line(installed_plugins.contains(id), id, color));
        out.push('\n');
    }

    if let Some(map) = profile.mcp_servers.as_object() {
        if !map.is_empty() {
            out.push_str(&format!("{}\n", heading(&format!("MCP servers ({})", map.len()), color)));
            for key in map.keys() {
                out.push_str(&format!("    - {key}\n"));
            }
        }
    }

    if !profile.plugin_dirs.is_empty() {
        out.push_str(&format!("{}\n", heading(&format!("Plugin dirs ({})", profile.plugin_dirs.len()), color)));
        for d in &profile.plugin_dirs {
            out.push_str(&format!("    - {d}\n"));
        }
    }
    out
}

fn source_label(source: &resolve::ProfileSource) -> String {
    match source {
        resolve::ProfileSource::EnvDir => "env dir (CLAUDE_PROFILE_DIR)".into(),
        resolve::ProfileSource::ProjectDir => "project".into(),
        resolve::ProfileSource::UserDir => "personal (~/.claude-profiles)".into(),
        resolve::ProfileSource::Pack(p) => format!("pack {p}"),
        resolve::ProfileSource::BundledDir => "bundled".into(),
    }
}

/// Author fallback for a pack source: the owner half of an `owner--repo` dir name.
fn pack_owner(source: &resolve::ProfileSource) -> Option<String> {
    match source {
        resolve::ProfileSource::Pack(p) => p.split_once("--").map(|(o, _)| o.to_string()),
        _ => None,
    }
}

/// Which of a profile's marketplaces/plugins are already vendored for `profile_key`:
/// a marketplace counts as installed once its clone exists under the shared
/// `store/marketplaces/` cache; a plugin/skill counts once its own entry exists under
/// this profile's `store/<profile_key>/vendor/`.
fn installed_sets(profile: &Profile, profile_key: &str, paths: &Paths) -> (BTreeSet<String>, BTreeSet<String>) {
    let installed_mkts: BTreeSet<String> = profile
        .marketplaces
        .keys()
        .filter(|name| paths.marketplace_clone_dir(name).is_dir())
        .cloned()
        .collect();

    let vendor_dir = paths.profile_vendor_dir(profile_key);
    let installed_plugins: BTreeSet<String> = std::fs::read_dir(&vendor_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    (installed_mkts, installed_plugins)
}

pub fn run(
    target: &str,
    paths: &Paths,
    cwd: &Path,
    env: Option<&Path>,
    bundled: &Path,
) -> anyhow::Result<()> {
    let color = use_color();

    if target.contains('/') {
        // Repo reference: fetch into a throwaway temp dir, show, discard on drop.
        let (_tmp, dir) = pack::fetch_ephemeral(&RealGit, target)?;
        let profile = pack::read_default_profile(&dir)?;
        let repo_owner = parse_repo_ref(target)?.owner;
        let author = profile.author.clone().or(Some(repo_owner));
        let key = pack::default_profile_name(&dir)?;
        let (installed_mkts, installed_plugins) = installed_sets(&profile, &key, paths);
        print!(
            "{}",
            format_show(&profile, author.as_deref(), target, &installed_plugins, &installed_mkts, color)
        );
    } else {
        let resolved = resolve::resolve(target, paths, cwd, env, bundled)?;
        let label = source_label(&resolved.source);
        let author = resolved
            .profile
            .author
            .clone()
            .or_else(|| pack_owner(&resolved.source));
        // Expand `extends` so the plugin/marketplace lists reflect what actually installs.
        let profile = extends::resolve_extends(resolved.profile, &|parent| {
            Ok(resolve::resolve(parent, paths, cwd, env, bundled)?.profile)
        })?;
        let (installed_mkts, installed_plugins) = installed_sets(&profile, target, paths);
        print!(
            "{}",
            format_show(&profile, author.as_deref(), &label, &installed_plugins, &installed_mkts, color)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(json: &str) -> Profile {
        Profile::from_json_str(json).unwrap()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn installed_sets_reflects_marketplace_clones_and_vendored_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path().to_path_buf());
        std::fs::create_dir_all(paths.marketplace_clone_dir("have")).unwrap();
        std::fs::create_dir_all(paths.profile_vendor_dir("p").join("have@have")).unwrap();

        let profile = prof(
            r#"{"name":"p","marketplaces":{"have":"o/r","missing":"o/s"},"plugins":["have@have","new@have"]}"#,
        );
        let (installed_mkts, installed_plugins) = installed_sets(&profile, "p", &paths);
        assert_eq!(installed_mkts, set(&["have"]));
        assert_eq!(installed_plugins, set(&["have@have"]));
    }

    #[test]
    fn marks_installed_and_new_plugins_without_color() {
        let p = prof(r#"{"name":"x","description":"d","plugins":["have@m","new@m"]}"#);
        let s = format_show(&p, Some("alice"), "personal", &set(&["have@m"]), &set(&[]), false);
        assert!(s.contains("Author:  alice"));
        assert!(s.contains("have@m  (installed)"));
        assert!(s.contains("+ new@m  (new)"));
        assert!(!s.contains("\x1b[")); // no ANSI when color disabled
    }

    #[test]
    fn author_falls_back_to_dash() {
        let p = prof(r#"{"name":"x","plugins":[]}"#);
        let s = format_show(&p, None, "personal", &set(&[]), &set(&[]), false);
        assert!(s.contains("Author:  —"));
        assert!(s.contains("(no description)"));
    }

    #[test]
    fn marks_marketplaces_and_notes_extends() {
        let p = prof(r#"{"name":"x","extends":"base","marketplaces":{"m":"o/r"}}"#);
        let s = format_show(&p, None, "personal", &set(&[]), &set(&["m"]), false);
        assert!(s.contains("m  (o/r)  (installed)"));
        assert!(s.contains("Extends: base (not expanded)"));
    }

    #[test]
    fn color_output_uses_ansi() {
        let p = prof(r#"{"name":"x","plugins":["new@m"]}"#);
        let s = format_show(&p, None, "personal", &set(&[]), &set(&[]), true);
        assert!(s.contains(GREEN));
        assert!(s.contains(RESET));
    }

    #[test]
    fn pack_owner_extracts_owner_half() {
        assert_eq!(pack_owner(&resolve::ProfileSource::Pack("alice--tools".into())), Some("alice".to_string()));
        assert_eq!(pack_owner(&resolve::ProfileSource::UserDir), None);
    }
}
