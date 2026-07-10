use crate::claude::{self, InstalledPlugin, Marketplace};
use crate::profile;
use crate::refmap::{build_refmap, RefMap};

pub fn format_status(installed: &[InstalledPlugin], mkts: &[Marketplace], refmap: &RefMap) -> String {
    let mut out = String::from("Installed plugins:\n");
    for p in installed {
        let refs = refmap.plugin_refs.get(&p.id);
        match refs {
            Some(list) => out.push_str(&format!("  {}  ← {}\n", p.id, list.join(", "))),
            None => out.push_str(&format!("  {}  (unreferenced)\n", p.id)),
        }
    }
    out.push_str("Marketplaces:\n");
    for m in mkts {
        match refmap.marketplace_refs.get(&m.name) {
            Some(list) => out.push_str(&format!("  {}  ← {}\n", m.name, list.join(", "))),
            None => out.push_str(&format!("  {}  (unreferenced)\n", m.name)),
        }
    }
    out
}

pub fn run<C: claude::ClaudeCli>(cli: &C, profiles: &[(String, profile::Profile)]) -> anyhow::Result<()> {
    let installed = cli.list_plugins()?;
    let mkts = cli.list_marketplaces()?;
    let refmap = build_refmap(profiles);
    print!("{}", format_status(&installed, &mkts, &refmap));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{InstalledPlugin, Marketplace};
    use crate::refmap::build_refmap;

    fn plug(id: &str) -> InstalledPlugin {
        InstalledPlugin { id: id.into(), enabled: false, scope: "user".into(), mcp_servers: serde_json::json!({}) }
    }

    #[test]
    fn shows_referencing_profiles_and_unreferenced() {
        let profiles = vec![("a".to_string(),
            crate::profile::Profile::from_json_str(r#"{"name":"a","plugins":["used@m"]}"#).unwrap())];
        let rm = build_refmap(&profiles);
        let s = format_status(&[plug("used@m"), plug("stale@m")], &[Marketplace { name: "m".into() }], &rm);
        assert!(s.contains("used@m"));
        assert!(s.contains("a"));            // referencing profile listed
        assert!(s.contains("stale@m"));
        assert!(s.contains("unreferenced")); // stale plugin flagged
    }
}
