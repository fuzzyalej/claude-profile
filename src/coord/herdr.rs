use crate::coord::backend::{launch_args, Backend, LaunchSpec, Observed};
use crate::coord::state::WorkerState;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

pub trait HerdrCli {
    fn run(&self, args: &[String]) -> Result<Value>;
    fn run_text(&self, args: &[String]) -> Result<String>;
}

const DETECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const DETECT_POLL: std::time::Duration = std::time::Duration::from_millis(500);

pub struct RealHerdr;

#[derive(Debug)]
pub struct HerdrError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for HerdrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HerdrError {}

fn parse_error(text: &str) -> Option<HerdrError> {
    let v = serde_json::from_str::<Value>(text).ok()?;
    let e = v.get("error")?;
    Some(HerdrError {
        code: e
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("error")
            .to_string(),
        message: e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

fn command_error(verb: &str, stdout: &str, stderr: &str) -> anyhow::Error {
    if let Some(e) = parse_error(stderr).or_else(|| parse_error(stdout)) {
        return anyhow::Error::new(e).context(format!("herdr {verb} failed"));
    }
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    anyhow!("herdr {verb} failed: {}", text.trim())
}

fn parse_json_output(stdout: &str) -> Result<Value> {
    if stdout.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(stdout).context("herdr printed invalid JSON")
}

impl RealHerdr {
    fn exec(&self, args: &[String]) -> Result<String> {
        let out = crate::exe::command("herdr")
            .args(args)
            .output()
            .context("failed to run herdr")?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.status.success() {
            return Err(command_error(
                args.first().map(String::as_str).unwrap_or(""),
                &stdout,
                &String::from_utf8_lossy(&out.stderr),
            ));
        }
        Ok(stdout)
    }
}

impl HerdrCli for RealHerdr {
    fn run(&self, args: &[String]) -> Result<Value> {
        let stdout = self.exec(args)?;
        parse_json_output(&stdout)
    }

    fn run_text(&self, args: &[String]) -> Result<String> {
        self.exec(args)
    }
}

pub struct Herdr<C: HerdrCli> {
    cli: C,
    workspace: Option<String>,
}

impl<C: HerdrCli> Herdr<C> {
    pub fn new(cli: C, coordinator_pane: String) -> Self {
        let workspace = coordinator_pane.split_once(':').map(|(w, _)| w.to_string());
        Herdr { cli, workspace }
    }

    fn create_tab(&self, label: &str, cwd: &str) -> Result<Value> {
        let mut a = args(&["tab", "create"]);
        if let Some(ws) = &self.workspace {
            a.extend(args(&["--workspace", ws]));
        }
        a.extend(args(&["--label", label, "--cwd", cwd, "--no-focus"]));
        self.cli.run(&a)
    }

    fn prepare(&self, w: &WorkerState, spec: &LaunchSpec, pane: &str, prompt: &str) -> Result<()> {
        self.cli.run(&[
            "pane".into(),
            "run".into(),
            pane.to_string(),
            pane_command(spec, &w.session_id),
        ])?;
        self.wait_idle(pane)?;
        self.cli.run(&args(&["agent", "prompt", pane, prompt]))?;
        let title = format!("{} · {}", w.name, w.profiles.join("+"));
        self.cli.run(&args(&[
            "pane",
            "report-metadata",
            pane,
            "--source",
            "claude-profile",
            "--title",
            &title,
        ]))?;
        Ok(())
    }

    fn wait_idle(&self, pane: &str) -> Result<()> {
        let started = std::time::Instant::now();
        loop {
            let result = self.cli.run(&args(&[
                "agent",
                "wait",
                pane,
                "--until",
                "idle",
                "--timeout",
                "60000",
            ]));
            match result {
                Err(e) if is_gone(&e) && started.elapsed() < DETECT_TIMEOUT => {
                    std::thread::sleep(DETECT_POLL);
                }
                other => return other.map(|_| ()),
            }
        }
    }

    fn pane(w: &WorkerState) -> Result<&str> {
        w.pane_id
            .as_deref()
            .ok_or_else(|| anyhow!("worker {} has no pane", w.id))
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

pub fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:=@-".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

pub fn pane_command(spec: &LaunchSpec, session_id: &str) -> String {
    let tail = vec!["--session-id".to_string(), session_id.to_string()];
    let mut parts = vec![shell_quote(&spec.exe.to_string_lossy())];
    parts.extend(launch_args(spec, &tail).iter().map(|a| shell_quote(a)));
    parts.join(" ")
}

pub fn map_status(agent_status: &str) -> Observed {
    match agent_status {
        "blocked" => Observed::Blocked,
        "idle" | "done" => Observed::Idle,
        _ => Observed::Running,
    }
}

fn is_gone(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<HerdrError>()
            .is_some_and(|h| h.code == "pane_not_found" || h.code == "agent_not_found")
    })
}

impl<C: HerdrCli> Backend for Herdr<C> {
    fn start(&mut self, w: &mut WorkerState, spec: &LaunchSpec, prompt: &str) -> Result<()> {
        let cwd = w.cwd.to_string_lossy().into_owned();
        let tab = self.create_tab(&w.name, &cwd)?;
        let pane = tab
            .pointer("/result/root_pane/pane_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("herdr tab create returned no pane id"))?
            .to_string();
        if let Err(e) = self.prepare(w, spec, &pane, prompt) {
            let output = self
                .cli
                .run_text(&args(&["pane", "read", &pane, "--source", "recent", "--lines", "50"]))
                .unwrap_or_default();
            let _ = self.cli.run(&args(&["pane", "close", &pane]));
            w.pane_id = None;
            let output = output.trim();
            if output.is_empty() {
                return Err(e);
            }
            return Err(e.context(format!("worker pane output:\n{output}")));
        }
        w.pane_id = Some(pane);
        Ok(())
    }

    fn send(&mut self, w: &mut WorkerState, _spec: &LaunchSpec, prompt: &str) -> Result<()> {
        let pane = Self::pane(w)?;
        match self.cli.run(&args(&["agent", "prompt", pane, prompt])) {
            Ok(_) => Ok(()),
            Err(e) if is_gone(&e) => bail!("pane closed: {e:#}"),
            Err(e) => Err(e),
        }
    }

    fn observe(&mut self, w: &WorkerState) -> Result<Observed> {
        let pane = Self::pane(w)?;
        match self.cli.run(&args(&["agent", "get", pane])) {
            Ok(v) => Ok(map_status(
                v.pointer("/result/agent/agent_status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
            )),
            Err(e) if is_gone(&e) => Ok(Observed::Gone),
            Err(e) => Err(e),
        }
    }

    fn last_output(&mut self, w: &WorkerState, lines: usize) -> Result<String> {
        let pane = Self::pane(w)?;
        self.cli.run_text(&args(&[
            "agent",
            "read",
            pane,
            "--source",
            "recent",
            "--lines",
            &lines.to_string(),
        ]))
    }

    fn stop(&mut self, w: &WorkerState) -> Result<()> {
        let pane = Self::pane(w)?;
        for _ in 0..2 {
            self.cli
                .run(&args(&["agent", "send-keys", pane, "ctrl+c"]))?;
        }
        Ok(())
    }

    fn close(&mut self, w: &WorkerState) -> Result<()> {
        let pane = Self::pane(w)?;
        match self.cli.run(&args(&["pane", "close", pane])) {
            Ok(_) => Ok(()),
            Err(e) if is_gone(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::state::Status;
    use serde_json::json;
    use std::cell::RefCell;
    use std::path::PathBuf;

    struct FakeHerdr {
        calls: RefCell<Vec<Vec<String>>>,
        next_pane: RefCell<usize>,
        status: String,
        fail_code: Option<String>,
        fail_when: RefCell<Vec<(String, String)>>,
    }

    impl FakeHerdr {
        fn new() -> Self {
            FakeHerdr {
                calls: RefCell::new(vec![]),
                next_pane: RefCell::new(2),
                status: "idle".into(),
                fail_code: None,
                fail_when: RefCell::new(vec![]),
            }
        }

        fn fail_on(&self, prefix: &str, code: &str) {
            self.fail_when.borrow_mut().push((prefix.into(), code.into()));
        }
    }

    impl HerdrCli for FakeHerdr {
        fn run(&self, args: &[String]) -> Result<Value> {
            self.calls.borrow_mut().push(args.to_vec());
            let line = args.join(" ");
            let matched = self
                .fail_when
                .borrow()
                .iter()
                .find(|(prefix, _)| line.starts_with(prefix.as_str()))
                .map(|(_, code)| code.clone());
            if let Some(code) = matched.or_else(|| self.fail_code.clone()) {
                return Err(anyhow::Error::new(HerdrError {
                    code,
                    message: "nope".into(),
                }));
            }
            if args[0] == "tab" && args[1] == "create" {
                let mut n = self.next_pane.borrow_mut();
                let id = format!("w9:p{}", *n);
                *n += 1;
                return Ok(json!({"result": {"root_pane": {"pane_id": id}}}));
            }
            if args[0] == "agent" && args[1] == "get" {
                return Ok(json!({"result": {"agent": {"agent_status": self.status}}}));
            }
            Ok(json!({"result": {}}))
        }

        fn run_text(&self, args: &[String]) -> Result<String> {
            self.calls.borrow_mut().push(args.to_vec());
            Ok("plain text".into())
        }
    }

    fn spec() -> LaunchSpec {
        LaunchSpec {
            exe: PathBuf::from("/usr/bin/claude-profile"),
            profiles: vec!["rust".into()],
            permission_flags: vec!["--allowedTools".into(), "Bash(git log:*) Edit".into()],
            run_dir: PathBuf::from("/tmp/run dir"),
        }
    }

    fn worker(name: &str) -> WorkerState {
        WorkerState {
            id: name.into(),
            name: name.into(),
            profiles: vec!["rust".into(), "tdd".into()],
            cwd: PathBuf::from("/tmp/wt"),
            task: "t".into(),
            status: Status::Starting,
            status_detail: None,
            worktree: None,
            branch: None,
            base: None,
            session_id: "sess-1".into(),
            pane_id: None,
            pid: None,
            created_at: 0,
            updated_at: 0,
            idle_since: None,
        }
    }

    fn joined(h: &Herdr<FakeHerdr>) -> Vec<String> {
        h.cli.calls.borrow().iter().map(|c| c.join(" ")).collect()
    }

    #[test]
    fn empty_output_is_success() {
        assert_eq!(parse_json_output("").unwrap(), Value::Null);
        assert_eq!(parse_json_output("\n").unwrap(), Value::Null);
        assert_eq!(parse_json_output("{\"a\":1}").unwrap(), json!({"a": 1}));
        assert!(parse_json_output("nope").is_err());
    }

    #[test]
    fn shell_quote_cases() {
        assert_eq!(shell_quote("rust"), "rust");
        assert_eq!(shell_quote("Bash(git log:*)"), "'Bash(git log:*)'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn pane_command_quotes_permission_values() {
        let cmd = pane_command(&spec(), "sess-1");
        assert_eq!(
            cmd,
            "/usr/bin/claude-profile rust --yes -- --session-id sess-1 --add-dir '/tmp/run dir' --allowedTools 'Bash(git log:*) Edit'"
        );
    }

    #[test]
    fn start_issues_commands_in_order() {
        let mut h = Herdr::new(FakeHerdr::new(), "w9:p1".into());
        let mut w = worker("a");
        let prompt = "say \"hi\"\nit's 'ok'";
        h.start(&mut w, &spec(), prompt).unwrap();
        let calls = joined(&h);
        assert_eq!(
            calls[0],
            "tab create --workspace w9 --label a --cwd /tmp/wt --no-focus"
        );
        assert_eq!(calls[1], format!("pane run w9:p2 {}", pane_command(&spec(), "sess-1")));
        assert_eq!(calls[2], "agent wait w9:p2 --until idle --timeout 60000");
        assert_eq!(calls[3], format!("agent prompt w9:p2 {prompt}"));
        assert_eq!(
            h.cli.calls.borrow()[3][3],
            prompt,
            "prompt is one argv element"
        );
        assert_eq!(
            calls[4],
            "pane report-metadata w9:p2 --source claude-profile --title a · rust+tdd"
        );
        assert_eq!(w.pane_id.as_deref(), Some("w9:p2"));
    }

    #[test]
    fn send_passes_prompt_as_one_argument() {
        let mut h = Herdr::new(FakeHerdr::new(), "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        h.send(&mut w, &spec(), "do it").unwrap();
        assert_eq!(joined(&h), ["agent prompt w9:p2 do it"]);
    }

    #[test]
    fn start_tab_create_error_is_returned() {
        let cli = FakeHerdr::new();
        cli.fail_on("tab create", "invalid_key");
        let mut h = Herdr::new(cli, "w9:p1".into());
        assert!(h.start(&mut worker("a"), &spec(), "x").is_err());
        assert_eq!(joined(&h).len(), 1);
    }

    #[test]
    fn start_failure_after_tab_create_closes_pane_with_output() {
        let cli = FakeHerdr::new();
        cli.fail_on("agent wait", "timeout");
        let mut h = Herdr::new(cli, "w9:p1".into());
        let mut w = worker("a");
        let err = format!("{:#}", h.start(&mut w, &spec(), "x").unwrap_err());
        assert!(err.contains("timeout"), "{err}");
        assert!(err.contains("plain text"), "{err}");
        let calls = joined(&h);
        assert!(calls.contains(&"pane read w9:p2 --source recent --lines 50".to_string()), "{calls:?}");
        assert_eq!(calls.last().unwrap(), "pane close w9:p2");
        assert!(w.pane_id.is_none());
    }

    #[test]
    fn map_status_cases() {
        assert_eq!(map_status("working"), Observed::Running);
        assert_eq!(map_status("blocked"), Observed::Blocked);
        assert_eq!(map_status("idle"), Observed::Idle);
        assert_eq!(map_status("done"), Observed::Idle);
        assert_eq!(map_status("unknown"), Observed::Running);
    }

    #[test]
    fn observe_gone_on_agent_not_found() {
        let mut cli = FakeHerdr::new();
        cli.fail_code = Some("agent_not_found".into());
        let mut h = Herdr::new(cli, "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        assert_eq!(h.observe(&w).unwrap(), Observed::Gone);
    }

    #[test]
    fn observe_gone_on_pane_not_found() {
        let mut cli = FakeHerdr::new();
        cli.fail_code = Some("pane_not_found".into());
        let mut h = Herdr::new(cli, "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        assert_eq!(h.observe(&w).unwrap(), Observed::Gone);
    }

    #[test]
    fn observe_propagates_other_errors() {
        let mut cli = FakeHerdr::new();
        cli.fail_code = Some("invalid_key".into());
        let mut h = Herdr::new(cli, "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        assert!(h.observe(&w).is_err());
    }

    #[test]
    fn command_error_parses_stderr_json() {
        let stderr = r#"{"error":{"code":"agent_not_found","message":"agent target w9:p99 not found"},"id":"cli:agent:get"}"#;
        let e = command_error("agent", "", stderr);
        assert!(is_gone(&e));
        assert!(e.to_string().contains("herdr agent failed"));
        assert!(format!("{e:#}").contains("agent_not_found: agent target w9:p99 not found"));
    }

    #[test]
    fn command_error_falls_back_to_raw_text() {
        let e = command_error("pane", "", "boom\n");
        assert!(!is_gone(&e));
        assert_eq!(e.to_string(), "herdr pane failed: boom");
    }

    #[test]
    fn observe_maps_agent_status() {
        let mut cli = FakeHerdr::new();
        cli.status = "blocked".into();
        let mut h = Herdr::new(cli, "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        assert_eq!(h.observe(&w).unwrap(), Observed::Blocked);
    }

    #[test]
    fn send_reports_pane_closed() {
        let mut cli = FakeHerdr::new();
        cli.fail_code = Some("agent_not_found".into());
        let mut h = Herdr::new(cli, "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        let err = h.send(&mut w, &spec(), "hi").unwrap_err();
        assert!(err.to_string().contains("pane closed"));
    }

    #[test]
    fn close_is_ok_when_pane_gone() {
        for code in ["pane_not_found", "agent_not_found"] {
            let mut cli = FakeHerdr::new();
            cli.fail_code = Some(code.into());
            let mut h = Herdr::new(cli, "w9:p1".into());
            let mut w = worker("a");
            w.pane_id = Some("w9:p2".into());
            h.close(&w).unwrap();
        }
    }

    #[test]
    fn close_propagates_other_errors() {
        let mut cli = FakeHerdr::new();
        cli.fail_code = Some("invalid_key".into());
        let mut h = Herdr::new(cli, "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        assert!(h.close(&w).is_err());
    }

    #[test]
    fn last_output_stop_close() {
        let mut h = Herdr::new(FakeHerdr::new(), "w9:p1".into());
        let mut w = worker("a");
        w.pane_id = Some("w9:p2".into());
        assert_eq!(h.last_output(&w, 20).unwrap(), "plain text");
        h.stop(&w).unwrap();
        h.close(&w).unwrap();
        let calls = joined(&h);
        assert_eq!(calls[0], "agent read w9:p2 --source recent --lines 20");
        assert_eq!(calls[1], "agent send-keys w9:p2 ctrl+c");
        assert_eq!(calls[2], "agent send-keys w9:p2 ctrl+c");
        assert_eq!(calls[3], "pane close w9:p2");
    }
}
