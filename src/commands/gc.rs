use crate::claude::ClaudeCli;
use crate::profile;
use crate::refmap::{build_refmap, unreferenced_marketplaces, unreferenced_plugins};
use crate::spinner::spin;

pub struct GcReport {
    pub removed_plugins: Vec<String>,
    pub removed_marketplaces: Vec<String>,
}

pub fn run<C: ClaudeCli>(cli: &C, profiles: &[(String, profile::Profile)], dry_run: bool) -> anyhow::Result<GcReport> {
    let installed = cli.list_plugins()?;
    let mkts = cli.list_marketplaces()?;
    let refmap = build_refmap(profiles);
    let removed_plugins = unreferenced_plugins(&installed, &refmap);
    let removed_marketplaces = unreferenced_marketplaces(&mkts, &refmap);
    if !dry_run {
        for id in &removed_plugins {
            spin(
                &format!("Removing plugin {id}..."),
                &format!("✔ Removed plugin {id}"),
                || cli.uninstall_plugin(id),
            )?;
        }
        for name in &removed_marketplaces {
            spin(
                &format!("Removing marketplace {name}..."),
                &format!("✔ Removed marketplace {name}"),
                || cli.marketplace_remove(name),
            )?;
        }
    }
    Ok(GcReport { removed_plugins, removed_marketplaces })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{ClaudeCli, InstalledPlugin, Marketplace};
    use std::cell::RefCell;

    struct MockCli {
        plugins: Vec<InstalledPlugin>, mkts: Vec<Marketplace>,
        uninstalled: RefCell<Vec<String>>, removed: RefCell<Vec<String>>,
    }
    impl ClaudeCli for MockCli {
        fn list_plugins(&self) -> anyhow::Result<Vec<InstalledPlugin>> { Ok(self.plugins.clone()) }
        fn list_marketplaces(&self) -> anyhow::Result<Vec<Marketplace>> { Ok(self.mkts.clone()) }
        fn marketplace_add(&self, _s: &str) -> anyhow::Result<()> { Ok(()) }
        fn install_plugin(&self, _i: &str) -> anyhow::Result<()> { Ok(()) }
        fn uninstall_plugin(&self, id: &str) -> anyhow::Result<()> { self.uninstalled.borrow_mut().push(id.into()); Ok(()) }
        fn marketplace_remove(&self, n: &str) -> anyhow::Result<()> { self.removed.borrow_mut().push(n.into()); Ok(()) }
    }
    fn plug(id: &str) -> InstalledPlugin {
        InstalledPlugin { id: id.into(), enabled: false, scope: "user".into(), mcp_servers: serde_json::json!({}) }
    }
    fn profiles() -> Vec<(String, crate::profile::Profile)> {
        vec![("a".into(), crate::profile::Profile::from_json_str(
            r#"{"name":"a","marketplaces":{"used":"o/r"},"plugins":["keep@used"]}"#).unwrap())]
    }

    #[test]
    fn dry_run_reports_without_removing() {
        let cli = MockCli {
            plugins: vec![plug("keep@used"), plug("orphan@x")],
            mkts: vec![Marketplace { name: "used".into() }, Marketplace { name: "stale".into() }],
            uninstalled: RefCell::new(vec![]), removed: RefCell::new(vec![]),
        };
        let report = run(&cli, &profiles(), true).unwrap();
        assert_eq!(report.removed_plugins, vec!["orphan@x".to_string()]);
        assert_eq!(report.removed_marketplaces, vec!["stale".to_string()]);
        assert!(cli.uninstalled.borrow().is_empty());
        assert!(cli.removed.borrow().is_empty());
    }

    #[test]
    fn real_run_uninstalls_and_removes() {
        let cli = MockCli {
            plugins: vec![plug("orphan@x")], mkts: vec![Marketplace { name: "stale".into() }],
            uninstalled: RefCell::new(vec![]), removed: RefCell::new(vec![]),
        };
        run(&cli, &profiles(), false).unwrap();
        assert_eq!(*cli.uninstalled.borrow(), vec!["orphan@x".to_string()]);
        assert_eq!(*cli.removed.borrow(), vec!["stale".to_string()]);
    }
}
