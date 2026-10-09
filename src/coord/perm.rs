const SINGLE_VALUE: [&str; 1] = ["--permission-mode"];
const TOOL_LISTS: [&str; 4] = [
    "--allowedTools",
    "--disallowedTools",
    "--allowed-tools",
    "--disallowed-tools",
];
const NO_VALUE: [&str; 1] = ["--dangerously-skip-permissions"];

fn is_flag(arg: &str) -> bool {
    arg.starts_with('-')
}

pub fn permission_flags(extra: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < extra.len() {
        let element = extra[i].as_str();
        i += 1;

        if let Some((flag, _)) = element.split_once('=') {
            if SINGLE_VALUE.contains(&flag) || TOOL_LISTS.contains(&flag) || NO_VALUE.contains(&flag) {
                result.push(element.to_string());
            }
        } else if NO_VALUE.contains(&element) {
            result.push(element.to_string());
        } else if SINGLE_VALUE.contains(&element) {
            if i < extra.len() {
                result.push(element.to_string());
                result.push(extra[i].clone());
                i += 1;
            }
        } else if TOOL_LISTS.contains(&element) {
            let start = i;
            while i < extra.len() && !is_flag(&extra[i]) {
                i += 1;
            }
            if i > start {
                result.push(element.to_string());
                result.extend(extra[start..i].iter().cloned());
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_permission_mode_and_value() {
        let input = vec![
            "--model".to_string(),
            "opus".to_string(),
            "--permission-mode".to_string(),
            "acceptEdits".to_string(),
        ];
        let result = permission_flags(&input);
        assert_eq!(result, vec!["--permission-mode", "acceptEdits"]);
    }

    #[test]
    fn keeps_equals_form() {
        let input = vec!["--permission-mode=plan".to_string()];
        let result = permission_flags(&input);
        assert_eq!(result, vec!["--permission-mode=plan"]);
    }

    #[test]
    fn keeps_skip_permissions_flag() {
        let input = vec![
            "--dangerously-skip-permissions".to_string(),
            "-c".to_string(),
        ];
        let result = permission_flags(&input);
        assert_eq!(result, vec!["--dangerously-skip-permissions"]);
    }

    #[test]
    fn keeps_tool_lists_with_spaces() {
        let input = vec![
            "--allowedTools".to_string(),
            "Bash(git log:*) Edit".to_string(),
            "--disallowedTools".to_string(),
            "WebFetch".to_string(),
        ];
        let result = permission_flags(&input);
        assert_eq!(
            result,
            vec![
                "--allowedTools",
                "Bash(git log:*) Edit",
                "--disallowedTools",
                "WebFetch"
            ]
        );
    }

    #[test]
    fn keeps_kebab_case_tool_flags() {
        let input: Vec<String> = ["--allowed-tools", "Edit", "--disallowed-tools=WebFetch", "--model", "opus"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            permission_flags(&input),
            vec!["--allowed-tools", "Edit", "--disallowed-tools=WebFetch"]
        );
    }

    #[test]
    fn tool_flags_keep_every_following_value() {
        let input: Vec<String> = ["--allowedTools", "Edit", "Bash(git:*)", "--model", "opus", "--disallowedTools", "A", "B"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            permission_flags(&input),
            vec!["--allowedTools", "Edit", "Bash(git:*)", "--disallowedTools", "A", "B"]
        );
    }

    #[test]
    fn permission_mode_keeps_one_value() {
        let input: Vec<String> = ["--permission-mode", "plan", "do the thing"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(permission_flags(&input), vec!["--permission-mode", "plan"]);
    }

    #[test]
    fn missing_value_at_end_is_dropped() {
        let input = vec!["--permission-mode".to_string()];
        let result = permission_flags(&input);
        assert_eq!(result, Vec::<String>::new());
    }
}
