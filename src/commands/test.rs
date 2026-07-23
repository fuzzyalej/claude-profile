use std::process::Command;

pub fn build_eval_args(target: &str, json: bool, extra: &[String]) -> Vec<String> {
    let mut args = vec!["plugin".to_string(), "eval".to_string(), target.to_string()];
    if json {
        args.push("--json".to_string());
    }
    args.extend(extra.iter().cloned());
    args
}

pub fn run(target: &str, json: bool, extra: &[String]) -> anyhow::Result<i32> {
    let args = build_eval_args(target, json, extra);
    let status = Command::new("claude").args(&args).status()
        .map_err(|e| anyhow::anyhow!("failed to run claude plugin eval: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_eval_argv() {
        let a = build_eval_args("my-skill", true, &["--case".into(), "smoke*".into()]);
        assert_eq!(a[0], "plugin");
        assert_eq!(a[1], "eval");
        assert_eq!(a[2], "my-skill");
        assert!(a.contains(&"--json".to_string()));
        let i = a.iter().position(|x| x == "--case").unwrap();
        assert_eq!(a[i + 1], "smoke*");
    }

    #[test]
    fn omits_json_when_false() {
        let a = build_eval_args("t", false, &[]);
        assert!(!a.contains(&"--json".to_string()));
    }
}
