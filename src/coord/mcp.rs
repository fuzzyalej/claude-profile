use crate::coord::backend::Backend;
use crate::coord::manager::{Manager, SpawnArgs};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};

const DEFAULT_PROTOCOL: &str = "2025-06-18";
const STATUS_VALUES: &str = "queued, starting, working, blocked, done, failed, cancelled";

pub fn tools() -> Value {
    json!([
        {
            "name": "list_profiles",
            "description": "List the claude-profile profiles installed on this machine. Takes no input. Returns an array of {name, description}; description is null when the profile has none. Call this before spawn to pick valid profile names.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "spawn",
            "description": format!(
                "Start a worker: a separate Claude Code session that runs one task with the given profiles. The worker cannot see this conversation, so the task must be self-contained: include every file path, requirement and constraint it needs. \
With worktree=true (the default) the worker runs in its own git worktree on a new branch cp/<id>; this needs the current directory to be inside a git repo. The worktree starts from HEAD, or from base_ref when given, and does not include uncommitted changes. Use worktree=false to run in the current directory, which is required outside a git repo. \
Profiles must be installed (see list_profiles). If no profile fits the task, use [\"clean\"]: plain Claude Code with no extra plugins, skills or MCP servers. Don't force a poor fit. If the number of active workers is already at the concurrency limit (default 4), the new worker is queued and starts when a slot frees up. \
Returns {{id, status}}, plus branch and worktree when a worktree was created. Status is starting or queued. All worker status values: {STATUS_VALUES}. Use status to poll and result to read the outcome."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "profiles": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Names of installed profiles to launch the worker with. At least one."
                    },
                    "task": {
                        "type": "string",
                        "description": "Self-contained instructions for the worker. It cannot see this conversation."
                    },
                    "worktree": {
                        "type": "boolean",
                        "default": true,
                        "description": "Run in a new git worktree on branch cp/<id>. Needs a git repo. Set false to run in the current directory."
                    },
                    "base_ref": {
                        "type": "string",
                        "description": "Git ref the worktree branch starts from. Defaults to HEAD. Ignored when worktree=false."
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional short name used to build the worker id."
                    }
                },
                "required": ["profiles", "task"]
            }
        },
        {
            "name": "status",
            "description": format!(
                "Get the current status of one worker, or of every worker in this run when id is omitted. Returns one status object for an id, or an array of them. Status values: {STATUS_VALUES}. Workers in done, failed or cancelled are finished; call result to read the outcome."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Worker id from spawn. Omit to list all workers."}
                }
            }
        },
        {
            "name": "result",
            "description": format!(
                "Read the outcome of a worker. Works at any time, but the summary is only final once status is done or failed. Returns {{status, summary, notes, branch, worktree, base, changed_files, session_id}}: summary and notes come from the worker's result file, or from its last output if it wrote none; branch, worktree, base and changed_files come from git. base is the commit the worktree branch started from; review the work with git diff <base>...<branch>. changed_files lists committed and uncommitted changes since base; if git cannot list them, changed_files is empty and changed_files_error says why. Status values: {STATUS_VALUES}."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Worker id from spawn."}
                },
                "required": ["id"]
            }
        },
        {
            "name": "send",
            "description": format!(
                "Send a follow-up message to an existing worker and resume it with its previous context. Use it to answer a blocked worker or to ask for changes after it finished. Works for a worker in any status except queued: starting, working, blocked, done, failed or cancelled. A headless worker that is still running is stopped and resumed with the message. Returns {{id, status}} with the new status, normally working. Status values: {STATUS_VALUES}."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Worker id from spawn."},
                    "message": {"type": "string", "description": "Follow-up instructions or answer for the worker."}
                },
                "required": ["id", "message"]
            }
        },
        {
            "name": "cancel",
            "description": format!(
                "Stop a worker. Use it for a worker that is queued, starting, working or blocked. Its status becomes cancelled; its worktree and branch are kept until cleanup. Returns the worker's updated status object. Status values: {STATUS_VALUES}."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Worker id from spawn."}
                },
                "required": ["id"]
            }
        },
        {
            "name": "cleanup",
            "description": "Release a worker's resources: close its Herdr pane, if it has one, and, when remove_worktree is true (the default), delete its git worktree and its cp/<id> branch. Call it only after you have read result and merged or discarded the work, because removing the worktree deletes uncommitted changes. The worker should be finished or cancelled. Returns an object describing what was removed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Worker id from spawn."},
                    "remove_worktree": {
                        "type": "boolean",
                        "default": true,
                        "description": "Also delete the worktree and branch cp/<id>."
                    }
                },
                "required": ["id"]
            }
        }
    ])
}

fn required_str<'a>(args: &'a Map<String, Value>, field: &str) -> Result<&'a str, String> {
    match args.get(field) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(format!("argument '{field}' must be a string")),
        None => Err(format!("missing required argument '{field}'")),
    }
}

fn optional_str(args: &Map<String, Value>, field: &str) -> Result<Option<String>, String> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("argument '{field}' must be a string")),
    }
}

fn bool_or(args: &Map<String, Value>, field: &str, default: bool) -> Result<bool, String> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("argument '{field}' must be a boolean")),
    }
}

fn spawn_args(args: &Map<String, Value>) -> Result<SpawnArgs, String> {
    let profiles = match args.get("profiles") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or("argument 'profiles' must be an array of strings")?,
        Some(_) => return Err("argument 'profiles' must be an array of strings".into()),
        None => return Err("missing required argument 'profiles'".into()),
    };
    Ok(SpawnArgs {
        profiles,
        task: required_str(args, "task")?.to_string(),
        worktree: bool_or(args, "worktree", true)?,
        base_ref: optional_str(args, "base_ref")?,
        name: optional_str(args, "name")?,
    })
}

fn dispatch<B: Backend>(mgr: &mut Manager<B>, name: &str, args: &Map<String, Value>) -> Result<Value, String> {
    let out = match name {
        "list_profiles" => mgr.list_profiles(),
        "spawn" => mgr.spawn(spawn_args(args)?),
        "status" => {
            let id = optional_str(args, "id")?;
            mgr.status(id.as_deref())
        }
        "result" => mgr.result(required_str(args, "id")?),
        "send" => mgr.send(required_str(args, "id")?, required_str(args, "message")?),
        "cancel" => mgr.cancel(required_str(args, "id")?),
        "cleanup" => mgr.cleanup(required_str(args, "id")?, bool_or(args, "remove_worktree", true)?),
        other => return Err(format!("unknown tool '{other}'")),
    };
    out.map_err(|e| format!("{e:#}"))
}

fn tool_result(outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(v) => {
            let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            json!({"content": [{"type": "text", "text": text}]})
        }
        Err(msg) => json!({"content": [{"type": "text", "text": msg}], "isError": true}),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn handle<B: Backend>(mgr: &mut Manager<B>, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params");
    let reply = match method {
        "initialize" => {
            let version = params
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "claude-profile-workers", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({"tools": tools()})),
        "tools/call" => {
            let name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
            let outcome = match name {
                None => Err("missing required parameter 'name'".to_string()),
                Some(name) => {
                    let empty = Map::new();
                    let args = match params.and_then(|p| p.get("arguments")) {
                        None | Some(Value::Null) => Ok(&empty),
                        Some(Value::Object(m)) => Ok(m),
                        Some(_) => Err("'arguments' must be an object".to_string()),
                    };
                    args.and_then(|a| dispatch(mgr, name, a))
                }
            };
            rpc_result(id, tool_result(outcome))
        }
        other => rpc_error(id, -32601, &format!("method not found: {other}")),
    };
    Some(reply)
}

fn write_line<W: Write>(output: &mut W, value: &Value) -> Result<()> {
    writeln!(output, "{value}")?;
    output.flush()?;
    Ok(())
}

pub fn serve<B: Backend, R: BufRead, W: Write>(mgr: &mut Manager<B>, input: R, mut output: W) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(mgr, &msg),
            Err(_) => Some(rpc_error(Value::Null, -32700, "parse error")),
        };
        if let Some(reply) = reply {
            write_line(&mut output, &reply)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::backend::FakeBackend;
    use crate::coord::state::{BackendKind, RunDir, RunState};
    use crate::fs_paths::Paths;
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn manager(tmp: &TempDir) -> Manager<FakeBackend> {
        let paths = Paths::from_home(tmp.path().join("home"));
        let run = RunState {
            run_id: "1-abcdef".into(),
            repo_root: None,
            cwd: tmp.path().to_path_buf(),
            permission_flags: vec![],
            backend: BackendKind::Headless,
            max_workers: 4,
            coordinator_pane: None,
            created_at: 1,
        };
        let dir = RunDir::create(&paths, &run).unwrap();
        let run_dir = dir.root.clone();
        Manager {
            run,
            dir,
            backend: FakeBackend::default(),
            spec_base: (PathBuf::from("claude-profile"), run_dir),
            profile_exists: Box::new(|n| n == "rust"),
            list: Box::new(|| vec![("rust".into(), Some("Rust work".into()))]),
            paths: Paths::from_home(tmp.path().join("home")),
            fallback: Box::new(|| Box::new(FakeBackend::default())),
            fallback_backend: None,
            prepare: Box::new(|_| Ok(())),
        }
    }

    fn run(lines: &[Value]) -> Vec<Value> {
        let raw: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        run_raw(&raw.join("\n"))
    }

    fn run_raw(input: &str) -> Vec<Value> {
        let tmp = TempDir::new().unwrap();
        let mut m = manager(&tmp);
        let mut out = Vec::new();
        serve(&mut m, input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn call(name: &str, args: Value) -> Value {
        let r = run(&[json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})]);
        r[0].clone()
    }

    fn text(r: &Value) -> &str {
        r["result"]["content"][0]["text"].as_str().unwrap()
    }

    fn is_error(r: &Value) -> bool {
        r["result"]["isError"] == true
    }

    #[test]
    fn initialize_echoes_protocol_version() {
        let r = run(&[json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["id"], 1);
        assert_eq!(r[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r[0]["result"]["capabilities"], json!({"tools": {}}));
        assert_eq!(r[0]["result"]["serverInfo"]["name"], "claude-profile-workers");
        assert_eq!(r[0]["result"]["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn initialize_without_version_defaults() {
        let r = run(&[json!({"jsonrpc":"2.0","id":"a","method":"initialize","params":{}})]);
        assert_eq!(r[0]["id"], "a");
        assert_eq!(r[0]["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn notification_gets_no_reply() {
        let r = run(&[
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","method":"nope"}),
        ]);
        assert!(r.is_empty());
    }

    #[test]
    fn ping_returns_empty_result() {
        let r = run(&[json!({"jsonrpc":"2.0","id":7,"method":"ping"})]);
        assert_eq!(r[0]["result"], json!({}));
    }

    #[test]
    fn tools_list_has_seven_tools() {
        let r = run(&[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})]);
        let names: Vec<&str> = r[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["list_profiles", "spawn", "status", "result", "send", "cancel", "cleanup"]
        );
    }

    #[test]
    fn every_tool_has_description_and_schema() {
        for t in tools().as_array().unwrap() {
            assert!(t["description"].as_str().unwrap().len() > 40, "{}", t["name"]);
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["inputSchema"]["properties"].is_object(), "{}", t["name"]);
        }
    }

    #[test]
    fn spawn_schema_requires_profiles_and_task() {
        let t = tools();
        let spawn = t.as_array().unwrap().iter().find(|t| t["name"] == "spawn").unwrap();
        assert_eq!(spawn["inputSchema"]["required"], json!(["profiles", "task"]));
        assert_eq!(spawn["inputSchema"]["properties"]["worktree"]["default"], true);
        let d = spawn["description"].as_str().unwrap();
        for needle in ["git repo", "HEAD", "base_ref", "uncommitted", "conversation", "self-contained", "queued", "clean"] {
            assert!(d.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn descriptions_list_status_values() {
        for t in tools().as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            if ["status", "result", "send", "cancel", "spawn"].contains(&name) {
                let d = t["description"].as_str().unwrap();
                assert!(d.contains("queued") && d.contains("cancelled"), "{name}");
            }
        }
    }

    #[test]
    fn result_and_send_descriptions_match_behavior() {
        let t = tools();
        let find = |n: &str| t.as_array().unwrap().iter().find(|x| x["name"] == n).unwrap()["description"].as_str().unwrap().to_string();
        let result = find("result");
        for needle in ["base", "changed_files_error", "changed_files"] {
            assert!(result.contains(needle), "result missing {needle}");
        }
        let send = find("send");
        assert!(send.contains("except queued"), "{send}");
        assert!(!send.contains("must not be cancelled"), "{send}");
    }

    #[test]
    fn call_spawn_returns_text_content() {
        let r = call("spawn", json!({"profiles":["rust"],"task":"fix it","worktree":false,"name":"fix"}));
        assert_eq!(r["id"], 1);
        assert!(!is_error(&r));
        assert_eq!(r["result"]["content"][0]["type"], "text");
        let v: Value = serde_json::from_str(text(&r)).unwrap();
        assert_eq!(v["id"], "fix");
        assert_eq!(v["status"], "starting");
    }

    #[test]
    fn manager_error_is_tool_error_not_rpc_error() {
        let r = call("spawn", json!({"profiles":["nope"],"task":"x","worktree":false}));
        assert!(r.get("error").is_none());
        assert!(is_error(&r));
        assert!(!text(&r).is_empty());
    }

    #[test]
    fn missing_task_is_tool_error_naming_field() {
        let r = call("spawn", json!({"profiles":["rust"]}));
        assert!(r.get("error").is_none());
        assert!(is_error(&r));
        assert!(text(&r).contains("task"));
    }

    #[test]
    fn missing_id_is_tool_error_naming_field() {
        for name in ["result", "cancel", "cleanup"] {
            let r = call(name, json!({}));
            assert!(is_error(&r), "{name}");
            assert!(text(&r).contains("id"), "{name}");
        }
        let r = call("send", json!({"id":"a"}));
        assert!(text(&r).contains("message"));
    }

    #[test]
    fn unknown_tool_is_tool_error() {
        let r = call("bogus", json!({}));
        assert!(r.get("error").is_none());
        assert!(is_error(&r));
        assert!(text(&r).contains("bogus"));
    }

    #[test]
    fn list_profiles_and_status_work() {
        let r = call("list_profiles", json!({}));
        let v: Value = serde_json::from_str(text(&r)).unwrap();
        assert_eq!(v[0]["name"], "rust");
        let r = call("status", json!({}));
        assert!(!is_error(&r));
    }

    #[test]
    fn unknown_method_is_32601() {
        let r = run(&[json!({"jsonrpc":"2.0","id":3,"method":"resources/list"})]);
        assert_eq!(r[0]["id"], 3);
        assert_eq!(r[0]["error"]["code"], -32601);
    }

    #[test]
    fn malformed_json_is_32700_with_null_id() {
        let r = run_raw("{not json\n");
        assert_eq!(r[0]["error"]["code"], -32700);
        assert!(r[0]["id"].is_null());
    }

    #[test]
    fn blank_lines_skipped_and_loop_continues_after_error() {
        let r = run_raw("\n{bad\n\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
        assert_eq!(r.len(), 2);
        assert_eq!(r[1]["id"], 2);
    }
}
