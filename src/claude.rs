use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub id: String,
    // Phase 2/3: enablement UI will read this to reflect `claude plugin list` state.
    #[allow(dead_code)]
    #[serde(default)]
    pub enabled: bool,
    // Phase 2/3: scope-aware plugin management (user vs project scope).
    #[allow(dead_code)]
    #[serde(default)]
    pub scope: String,
    #[serde(default = "empty_obj")]
    pub mcp_servers: serde_json::Value,
}

fn empty_obj() -> serde_json::Value { serde_json::json!({}) }

#[derive(Debug, Clone, Deserialize)]
pub struct Marketplace {
    pub name: String,
}

pub fn parse_plugin_list(json: &str) -> anyhow::Result<Vec<InstalledPlugin>> {
    Ok(serde_json::from_str(json)?)
}

pub trait ClaudeCli {
    fn list_plugins(&self) -> anyhow::Result<Vec<InstalledPlugin>>;
    fn list_marketplaces(&self) -> anyhow::Result<Vec<Marketplace>>;
    fn marketplace_add(&self, source: &str) -> anyhow::Result<()>;
    fn install_plugin(&self, id: &str) -> anyhow::Result<()>;
    fn uninstall_plugin(&self, id: &str) -> anyhow::Result<()>;
    fn marketplace_remove(&self, name: &str) -> anyhow::Result<()>;
}

pub struct RealClaude {
    pub bin: String,
}

impl RealClaude {
    pub fn new() -> RealClaude {
        RealClaude { bin: "claude".to_string() }
    }

    fn run(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        let out = Command::new(&self.bin).args(args).output()
            .map_err(|e| anyhow::anyhow!("failed to run `{} {}`: {e}", self.bin, args.join(" ")))?;
        if !out.status.success() {
            anyhow::bail!("`{} {}` failed: {}", self.bin, args.join(" "),
                String::from_utf8_lossy(&out.stderr));
        }
        Ok(out)
    }

    // Phase 2: launch-time marketplace pinning needs installLocation, which
    // `list_marketplaces`/the `Marketplace` struct don't carry.
    pub fn marketplace_list_raw(&self) -> anyhow::Result<String> {
        let out = self.run(&["plugin", "marketplace", "list", "--json"])?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}

impl ClaudeCli for RealClaude {
    fn list_plugins(&self) -> anyhow::Result<Vec<InstalledPlugin>> {
        let out = self.run(&["plugin", "list", "--json"])?;
        parse_plugin_list(&String::from_utf8_lossy(&out.stdout))
    }

    fn list_marketplaces(&self) -> anyhow::Result<Vec<Marketplace>> {
        let out = self.run(&["plugin", "marketplace", "list", "--json"])?;
        Ok(serde_json::from_str(&String::from_utf8_lossy(&out.stdout))?)
    }

    fn marketplace_add(&self, source: &str) -> anyhow::Result<()> {
        self.run(&["plugin", "marketplace", "add", source]).map(|_| ())
    }

    fn install_plugin(&self, id: &str) -> anyhow::Result<()> {
        self.run(&["plugin", "install", id, "--scope", "user"]).map(|_| ())
    }

    fn uninstall_plugin(&self, id: &str) -> anyhow::Result<()> {
        self.run(&["plugin", "uninstall", id, "--scope", "user"]).map(|_| ())
    }

    fn marketplace_remove(&self, name: &str) -> anyhow::Result<()> {
        self.run(&["plugin", "marketplace", "remove", name]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_list_json() {
        let json = r#"[
          {"id":"a@m1","version":"1","scope":"user","enabled":true,
           "installPath":"/x","mcpServers":{"s":{"command":"echo"}}},
          {"id":"b@m2","version":"2","scope":"user","enabled":false,"installPath":"/y"}
        ]"#;
        let v = parse_plugin_list(json).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "a@m1");
        assert!(v[0].enabled);
        assert!(v[0].mcp_servers.get("s").is_some());
        assert!(!v[1].enabled);
        assert_eq!(v[1].mcp_servers, serde_json::json!({}));
    }
}

#[cfg(test)]
mod cmd_tests {
    use super::*;
    // Verify the argv the RealClaude methods would run, via a small arg-builder.
    // RealClaude shells out (not unit-tested), so we assert the trait exists and
    // is object-safe by constructing a trait object over a local mock.
    struct M { calls: std::cell::RefCell<Vec<String>> }
    impl ClaudeCli for M {
        fn list_plugins(&self) -> anyhow::Result<Vec<InstalledPlugin>> { Ok(vec![]) }
        fn list_marketplaces(&self) -> anyhow::Result<Vec<Marketplace>> { Ok(vec![]) }
        fn marketplace_add(&self, _s: &str) -> anyhow::Result<()> { Ok(()) }
        fn install_plugin(&self, _i: &str) -> anyhow::Result<()> { Ok(()) }
        fn uninstall_plugin(&self, id: &str) -> anyhow::Result<()> { self.calls.borrow_mut().push(format!("uninstall:{id}")); Ok(()) }
        fn marketplace_remove(&self, n: &str) -> anyhow::Result<()> { self.calls.borrow_mut().push(format!("rmmkt:{n}")); Ok(()) }
    }

    #[test]
    fn trait_has_uninstall_and_remove() {
        let m = M { calls: std::cell::RefCell::new(vec![]) };
        m.uninstall_plugin("x@m").unwrap();
        m.marketplace_remove("m").unwrap();
        assert_eq!(*m.calls.borrow(), vec!["uninstall:x@m".to_string(), "rmmkt:m".to_string()]);
    }
}
