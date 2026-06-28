#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinueRequest {
    pub open: bool,
    pub role: String,
}

pub fn roles_command_args(args: &str) -> Result<Vec<String>, String> {
    let parts = args
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Ok(vec!["roles".to_string(), "list".to_string()]);
    }

    match parts[0].as_str() {
        "list" | "show" | "register" | "profile" | "options" | "opts" | "models" | "delete"
        | "remove" | "rm" => {
            let mut command_args = vec!["roles".to_string()];
            if matches!(parts[0].as_str(), "remove" | "rm") {
                command_args.push("delete".to_string());
                command_args.extend(parts.into_iter().skip(1));
            } else if matches!(parts[0].as_str(), "opts" | "models") {
                command_args.push("options".to_string());
                command_args.extend(parts.into_iter().skip(1));
            } else {
                command_args.extend(parts);
            }
            Ok(command_args)
        }
        "save" | "set" | "add" => legacy_register_args(&parts[1..]),
        role if parts.len() == 1 => Ok(vec![
            "roles".to_string(),
            "show".to_string(),
            role.to_string(),
        ]),
        _ if parts.len() >= 3 => legacy_register_args(&parts),
        _ => Err(format!("unknown /roles command '{}'", parts[0])),
    }
}

pub fn provider_command_args(args: &str) -> Result<Vec<Vec<String>>, String> {
    let parts = args
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Ok(vec![vec![
            "config".to_string(),
            "provider".to_string(),
            "list".to_string(),
        ]]);
    }

    match parts[0].as_str() {
        "list" | "status" => Ok(vec![vec![
            "config".to_string(),
            "provider".to_string(),
            "list".to_string(),
        ]]),
        "show" | "open" | "view" if parts.len() == 2 => Ok(vec![vec![
            "config".to_string(),
            "provider".to_string(),
            "show".to_string(),
            parts[1].clone(),
        ]]),
        "show" | "open" | "view" => Err("provider show needs <provider>".to_string()),
        "setup" | "edit" => provider_setup_args(&parts[1..]),
        "delete" | "remove" | "rm" => provider_delete_args(&parts[1..]),
        "key" | "set-key" => provider_key_args(&parts[1..]),
        "env" | "set-env" => provider_env_key_args(&parts[1..]),
        "endpoint" | "base-url" | "url" | "set-endpoint" => provider_endpoint_args(&parts[1..]),
        _provider if parts.len() == 1 => Ok(vec![vec![
            "config".to_string(),
            "provider".to_string(),
            "show".to_string(),
            parts[0].clone(),
        ]]),
        provider if parts.len() == 2 => {
            let provider = provider.to_string();
            let key = parts[1].clone();
            Ok(vec![vec![
                "config".to_string(),
                "set-key".to_string(),
                provider,
                "--key".to_string(),
                key,
            ]])
        }
        provider if parts.len() == 3 => {
            let provider = provider.to_string();
            let key = parts[1].clone();
            let endpoint = parts[2].clone();
            Ok(vec![
                vec![
                    "config".to_string(),
                    "set-key".to_string(),
                    provider.clone(),
                    "--key".to_string(),
                    key,
                ],
                vec![
                    "config".to_string(),
                    "set-endpoint".to_string(),
                    provider,
                    endpoint,
                ],
            ])
        }
        _ => Err(format!("unknown /provider command '{}'", parts[0])),
    }
}

pub fn doctor_command_args(args: &str) -> Result<Vec<String>, String> {
    let parts = args
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [] => Ok(vec!["doctor".to_string()]),
        [arg] if matches!(arg.as_str(), "run" | "check" | "now") => Ok(vec!["doctor".to_string()]),
        [arg] => Err(format!("unknown /doctor command '{arg}'")),
        _ => Err("doctor accepts at most one argument".to_string()),
    }
}

pub fn thread_command_args(args: &str) -> Result<Vec<String>, String> {
    let parts = args
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [] => Ok(vec!["thread".to_string(), "list".to_string()]),
        [cmd] if matches!(cmd.as_str(), "list" | "ls" | "show" | "status") => {
            Ok(vec!["thread".to_string(), "list".to_string()])
        }
        [cmd] if cmd.as_str() == "export" => Err("thread export needs <role>".to_string()),
        [cmd, role, rest @ ..] if cmd.as_str() == "export" => {
            if role.trim().is_empty() {
                return Err("thread export needs <role>".to_string());
            }
            thread_export_command_args(role, rest)
        }
        [role] => Ok(vec![
            "thread".to_string(),
            "show".to_string(),
            role.to_string(),
        ]),
        [cmd, role]
            if matches!(
                cmd.as_str(),
                "show" | "status" | "clear" | "reset" | "fresh"
            ) =>
        {
            let subcommand = if matches!(cmd.as_str(), "clear" | "reset" | "fresh") {
                "clear"
            } else {
                "show"
            };
            Ok(vec![
                "thread".to_string(),
                subcommand.to_string(),
                role.to_string(),
            ])
        }
        [cmd, ..] => Err(format!("unknown /thread command '{cmd}'")),
    }
}

pub fn jobs_command_args(args: &str) -> Result<Vec<String>, String> {
    let parts = args.split_whitespace().collect::<Vec<_>>();
    let limit = match parts.as_slice() {
        [] => 20,
        ["recover"] | ["repair"] => {
            return Ok(vec!["jobs".to_string(), "recover".to_string()]);
        }
        ["recover" | "repair", value] => {
            let minutes = parse_jobs_stale_minutes(value)?;
            return Ok(vec![
                "jobs".to_string(),
                "recover".to_string(),
                "--stale-minutes".to_string(),
                minutes.to_string(),
            ]);
        }
        [value] if matches!(*value, "list" | "show" | "status") => 20,
        [value] => parse_jobs_limit(value)?,
        ["show" | "open", id] => {
            return Ok(vec![
                "jobs".to_string(),
                "show".to_string(),
                (*id).to_string(),
            ]);
        }
        ["cancel", id] => {
            return Ok(vec![
                "jobs".to_string(),
                "cancel".to_string(),
                (*id).to_string(),
            ]);
        }
        ["retry" | "rerun" | "again", id] => {
            return Ok(vec![
                "jobs".to_string(),
                "retry".to_string(),
                (*id).to_string(),
                "--tui-handoff".to_string(),
            ]);
        }
        [cmd, value] if matches!(*cmd, "list" | "show" | "status") => parse_jobs_limit(value)?,
        [cmd, ..] => return Err(format!("unknown /jobs command '{cmd}'")),
    };
    Ok(vec![
        "jobs".to_string(),
        "--limit".to_string(),
        limit.to_string(),
    ])
}

pub fn continue_request(args: &str) -> Result<ContinueRequest, String> {
    let mut parts = args.split_whitespace().collect::<Vec<_>>();
    let open = matches!(parts.first().copied(), Some("open"));
    if open {
        parts.remove(0);
    }
    let target = parts.first().copied().unwrap_or("worker-cc");
    if parts.len() > 1 {
        return Err("continue accepts one target".to_string());
    }
    let role = continue_target_role(target)?;
    Ok(ContinueRequest { open, role })
}

pub fn continue_target_role(target: &str) -> Result<String, String> {
    let target = if target.trim().is_empty() {
        "worker-cc"
    } else {
        target.trim()
    };
    let role = match target {
        "claude" | "claude-code" | "cc" => "worker-cc",
        "codex" => "worker-codex",
        "gemini" | "gemini-cli" => "worker-gemini",
        role => role,
    };
    if role.trim().is_empty() {
        return Err("continue needs <role|claude|codex|gemini>".to_string());
    }
    Ok(role.to_string())
}

fn parse_jobs_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| format!("invalid jobs limit '{value}'"))?;
    if !(1..=200).contains(&limit) {
        return Err("jobs limit must be between 1 and 200".to_string());
    }
    Ok(limit)
}

fn parse_jobs_stale_minutes(value: &str) -> Result<u64, String> {
    let minutes = value
        .parse::<u64>()
        .map_err(|_| format!("invalid stale minute value '{value}'"))?;
    if !(1..=10_080).contains(&minutes) {
        return Err("jobs recover minutes must be between 1 and 10080".to_string());
    }
    Ok(minutes)
}

fn thread_export_command_args(role: &str, rest: &[String]) -> Result<Vec<String>, String> {
    let mut command_args = vec!["thread".to_string(), "export".to_string(), role.to_string()];
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--format" | "-f" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("thread export --format needs markdown or html".to_string());
                };
                match value.as_str() {
                    "markdown" | "html" => {
                        command_args.push("--format".to_string());
                        command_args.push(value.clone());
                    }
                    _ => return Err("thread export --format accepts markdown or html".to_string()),
                }
                i += 2;
            }
            "--out" | "-o" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("thread export --out needs a file path".to_string());
                };
                command_args.push("--out".to_string());
                command_args.push(value.clone());
                i += 2;
            }
            "--clipboard" | "--copy" => {
                command_args.push("--clipboard".to_string());
                i += 1;
            }
            value => return Err(format!("unknown /thread export option '{value}'")),
        }
    }
    Ok(command_args)
}

fn provider_delete_args(parts: &[String]) -> Result<Vec<Vec<String>>, String> {
    if parts.len() != 1 {
        return Err("provider delete needs <provider>".to_string());
    }
    Ok(vec![vec![
        "config".to_string(),
        "provider".to_string(),
        "delete".to_string(),
        parts[0].clone(),
    ]])
}

fn provider_key_args(parts: &[String]) -> Result<Vec<Vec<String>>, String> {
    if parts.len() != 2 {
        return Err("provider key needs <provider> <key-or-$ENV>".to_string());
    }
    Ok(vec![vec![
        "config".to_string(),
        "set-key".to_string(),
        parts[0].clone(),
        "--key".to_string(),
        parts[1].clone(),
    ]])
}

fn provider_env_key_args(parts: &[String]) -> Result<Vec<Vec<String>>, String> {
    if parts.len() != 2 {
        return Err("provider env needs <provider> <ENV_VAR>".to_string());
    }
    let env = parts[1].trim_start_matches('$');
    if env.is_empty() {
        return Err("provider env variable cannot be empty".to_string());
    }
    Ok(vec![vec![
        "config".to_string(),
        "set-key".to_string(),
        parts[0].clone(),
        "--key".to_string(),
        format!("${env}"),
    ]])
}

fn provider_setup_args(parts: &[String]) -> Result<Vec<Vec<String>>, String> {
    if parts.is_empty() {
        return Err("provider setup needs <provider>".to_string());
    }

    let provider = parts[0].clone();
    let mut kind: Option<String> = None;
    let mut key: Option<String> = None;
    let mut env: Option<String> = None;
    let mut endpoint: Option<String> = None;
    let mut i = 1;
    while i < parts.len() {
        match parts[i].as_str() {
            "--type" | "--kind" | "-t" => {
                let Some(value) = parts.get(i + 1) else {
                    return Err("provider setup --type needs a value".to_string());
                };
                kind = Some(value.clone());
                i += 2;
            }
            "--key" | "-k" => {
                let Some(value) = parts.get(i + 1) else {
                    return Err("provider setup --key needs a value".to_string());
                };
                key = Some(value.clone());
                i += 2;
            }
            "--env" | "-e" => {
                let Some(value) = parts.get(i + 1) else {
                    return Err("provider setup --env needs an env var".to_string());
                };
                let env_name = value.trim_start_matches('$');
                if env_name.is_empty() {
                    return Err("provider setup --env cannot be empty".to_string());
                }
                env = Some(env_name.to_string());
                i += 2;
            }
            "--endpoint" | "--base-url" | "--url" | "-u" => {
                let Some(value) = parts.get(i + 1) else {
                    return Err("provider setup --endpoint needs a url".to_string());
                };
                endpoint = Some(value.clone());
                i += 2;
            }
            value if key.is_none() => {
                key = Some(value.to_string());
                i += 1;
            }
            value if endpoint.is_none() => {
                endpoint = Some(value.to_string());
                i += 1;
            }
            value => return Err(format!("unknown provider setup argument '{value}'")),
        }
    }

    if key.is_some() && env.is_some() {
        return Err("provider setup cannot use both --key and --env".to_string());
    }

    let mut command = vec![
        "config".to_string(),
        "provider".to_string(),
        "setup".to_string(),
        provider,
    ];
    if let Some(kind) = kind.filter(|value| !value.trim().is_empty()) {
        command.push("--type".to_string());
        command.push(kind);
    }
    if let Some(key) = key.filter(|value| !value.trim().is_empty()) {
        command.push("--key".to_string());
        command.push(key);
    }
    if let Some(env) = env.filter(|value| !value.trim().is_empty()) {
        command.push("--env".to_string());
        command.push(env);
    }
    if let Some(endpoint) = endpoint.filter(|value| !value.trim().is_empty()) {
        command.push("--endpoint".to_string());
        command.push(endpoint);
    }

    Ok(vec![command])
}

fn provider_endpoint_args(parts: &[String]) -> Result<Vec<Vec<String>>, String> {
    if parts.len() != 2 {
        return Err("provider endpoint needs <provider> <url>".to_string());
    }
    Ok(vec![vec![
        "config".to_string(),
        "set-endpoint".to_string(),
        parts[0].clone(),
        parts[1].clone(),
    ]])
}

fn legacy_register_args(parts: &[String]) -> Result<Vec<String>, String> {
    if parts.len() < 3 {
        return Err("role registration needs <role> <model> <runtime>".to_string());
    }
    let mut command_args = vec![
        "roles".to_string(),
        "register".to_string(),
        parts[0].clone(),
        "--model".to_string(),
        parts[1].clone(),
        "--runtime".to_string(),
        parts[2].clone(),
    ];
    for part in &parts[3..] {
        match part.as_str() {
            "teams" | "agent-teams" | "true" => command_args.push("--agent-teams".to_string()),
            "no-teams" | "no-agent-teams" | "false" => {
                command_args.push("--no-agent-teams".to_string())
            }
            _ => command_args.push(part.clone()),
        }
    }
    Ok(command_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn thread_args_map_native_tui_shortcuts() {
        assert_eq!(
            thread_command_args("").unwrap(),
            strings(&["thread", "list"])
        );
        assert_eq!(
            thread_command_args("worker-cc").unwrap(),
            strings(&["thread", "show", "worker-cc"])
        );
        assert_eq!(
            thread_command_args("status worker-cc").unwrap(),
            strings(&["thread", "show", "worker-cc"])
        );
        assert_eq!(
            thread_command_args("fresh worker-cc").unwrap(),
            strings(&["thread", "clear", "worker-cc"])
        );
        assert_eq!(
            thread_command_args("export worker-cc --format html --out /tmp/session.html").unwrap(),
            strings(&[
                "thread",
                "export",
                "worker-cc",
                "--format",
                "html",
                "--out",
                "/tmp/session.html",
            ])
        );
        assert_eq!(
            thread_command_args("export worker-cc --clipboard").unwrap(),
            strings(&["thread", "export", "worker-cc", "--clipboard"])
        );
        assert!(thread_command_args("export").is_err());
        assert!(thread_command_args("export worker-cc --format pdf").is_err());
        assert!(thread_command_args("clear worker-cc extra").is_err());
    }

    #[test]
    fn jobs_args_map_native_tui_shortcuts() {
        assert_eq!(
            jobs_command_args("").unwrap(),
            strings(&["jobs", "--limit", "20"])
        );
        assert_eq!(
            jobs_command_args("12").unwrap(),
            strings(&["jobs", "--limit", "12"])
        );
        assert_eq!(
            jobs_command_args("list 5").unwrap(),
            strings(&["jobs", "--limit", "5"])
        );
        assert_eq!(
            jobs_command_args("show abcd1234").unwrap(),
            strings(&["jobs", "show", "abcd1234"])
        );
        assert_eq!(
            jobs_command_args("retry abcd1234").unwrap(),
            strings(&["jobs", "retry", "abcd1234", "--tui-handoff"])
        );
        assert_eq!(
            jobs_command_args("recover 45").unwrap(),
            strings(&["jobs", "recover", "--stale-minutes", "45"])
        );
        assert!(jobs_command_args("0").is_err());
        assert!(jobs_command_args("abc").is_err());
        assert!(jobs_command_args("list 5 extra").is_err());
        assert!(jobs_command_args("cancel").is_err());
        assert!(jobs_command_args("retry").is_err());
        assert!(jobs_command_args("recover 0").is_err());
    }

    #[test]
    fn continue_request_maps_runtime_aliases() {
        assert_eq!(continue_request("").unwrap().role, "worker-cc");
        assert_eq!(continue_target_role("claude").unwrap(), "worker-cc");
        assert_eq!(continue_target_role("claude-code").unwrap(), "worker-cc");
        assert_eq!(continue_target_role("cc").unwrap(), "worker-cc");
        assert_eq!(continue_target_role("codex").unwrap(), "worker-codex");
        assert_eq!(continue_target_role("gemini").unwrap(), "worker-gemini");
        assert_eq!(continue_target_role("gemini-cli").unwrap(), "worker-gemini");
        assert_eq!(
            continue_target_role("worker-reviewer").unwrap(),
            "worker-reviewer"
        );
        assert!(continue_request("worker-cc extra").is_err());
        assert_eq!(
            continue_request("open claude").unwrap(),
            ContinueRequest {
                open: true,
                role: "worker-cc".to_string(),
            }
        );
        assert!(continue_request("open worker-cc extra").is_err());
    }

    #[test]
    fn doctor_args_accept_read_only_diagnostics() {
        assert_eq!(doctor_command_args("").unwrap(), strings(&["doctor"]));
        assert_eq!(doctor_command_args("run").unwrap(), strings(&["doctor"]));
        assert_eq!(doctor_command_args("check").unwrap(), strings(&["doctor"]));
        assert!(
            doctor_command_args("delete")
                .unwrap_err()
                .contains("unknown")
        );
        assert!(
            doctor_command_args("run now")
                .unwrap_err()
                .contains("at most")
        );
    }

    #[test]
    fn roles_args_pass_through_modern_commands() {
        assert_eq!(
            roles_command_args("").expect("roles args"),
            strings(&["roles", "list"])
        );
        assert_eq!(
            roles_command_args("show critic").expect("roles args"),
            strings(&["roles", "show", "critic"])
        );
        assert_eq!(
            roles_command_args("opts").expect("roles args"),
            strings(&["roles", "options"])
        );
        assert_eq!(
            roles_command_args("models").expect("roles args"),
            strings(&["roles", "options"])
        );
        assert_eq!(
            roles_command_args("rm worker-gemini").expect("roles args"),
            strings(&["roles", "delete", "worker-gemini"])
        );
        assert_eq!(
            roles_command_args("profile dev --planner sonnet@claude-code --worker-cc minimax/MiniMax-M3@claude-code")
                .expect("roles args"),
            strings(&[
                "roles",
                "profile",
                "dev",
                "--planner",
                "sonnet@claude-code",
                "--worker-cc",
                "minimax/MiniMax-M3@claude-code",
            ])
        );
    }

    #[test]
    fn roles_args_support_legacy_positional_registration() {
        assert_eq!(
            roles_command_args("save worker-cc minimax-m3 claude-code teams").expect("roles args"),
            strings(&[
                "roles",
                "register",
                "worker-cc",
                "--model",
                "minimax-m3",
                "--runtime",
                "claude-code",
                "--agent-teams",
            ])
        );
        assert_eq!(
            roles_command_args("critic glm51 codex --provider openai --model-name glm-5.1")
                .expect("roles args"),
            strings(&[
                "roles",
                "register",
                "critic",
                "--model",
                "glm51",
                "--runtime",
                "codex",
                "--provider",
                "openai",
                "--model-name",
                "glm-5.1",
            ])
        );
    }

    #[test]
    fn provider_args_default_to_provider_list() {
        assert_eq!(
            provider_command_args("").expect("provider args"),
            vec![strings(&["config", "provider", "list"])]
        );
        assert_eq!(
            provider_command_args("status").expect("provider args"),
            vec![strings(&["config", "provider", "list"])]
        );
    }

    #[test]
    fn provider_args_support_detail_delete_key_endpoint_and_env() {
        assert_eq!(
            provider_command_args("openai").expect("provider args"),
            vec![strings(&["config", "provider", "show", "openai"])]
        );
        assert_eq!(
            provider_command_args("show openai").expect("provider args"),
            vec![strings(&["config", "provider", "show", "openai"])]
        );
        assert!(provider_command_args("show").is_err());
        assert_eq!(
            provider_command_args("rm openrouter").expect("provider args"),
            vec![strings(&["config", "provider", "delete", "openrouter"])]
        );
        assert_eq!(
            provider_command_args("key openai $OPENAI_API_KEY").expect("provider args"),
            vec![strings(&[
                "config",
                "set-key",
                "openai",
                "--key",
                "$OPENAI_API_KEY",
            ])]
        );
        assert_eq!(
            provider_command_args("env anthropic ANTHROPIC_API_KEY").expect("provider args"),
            vec![strings(&[
                "config",
                "set-key",
                "anthropic",
                "--key",
                "$ANTHROPIC_API_KEY",
            ])]
        );
        assert_eq!(
            provider_command_args("endpoint openai https://api.openai.com/v1")
                .expect("provider args"),
            vec![strings(&[
                "config",
                "set-endpoint",
                "openai",
                "https://api.openai.com/v1",
            ])]
        );
    }

    #[test]
    fn provider_args_support_positional_and_form_setup() {
        assert_eq!(
            provider_command_args("openai $OPENAI_API_KEY https://api.openai.com/v1")
                .expect("provider args"),
            vec![
                strings(&["config", "set-key", "openai", "--key", "$OPENAI_API_KEY"]),
                strings(&[
                    "config",
                    "set-endpoint",
                    "openai",
                    "https://api.openai.com/v1",
                ]),
            ]
        );
        assert_eq!(
            provider_command_args(
                "setup openai --type openai --env OPENAI_API_KEY --endpoint https://api.openai.com/v1"
            )
            .expect("provider args"),
            vec![strings(&[
                "config",
                "provider",
                "setup",
                "openai",
                "--type",
                "openai",
                "--env",
                "OPENAI_API_KEY",
                "--endpoint",
                "https://api.openai.com/v1",
            ])]
        );
        assert_eq!(
            provider_command_args("edit minimax --env MINIMAX_API_KEY").expect("provider args"),
            vec![strings(&[
                "config",
                "provider",
                "setup",
                "minimax",
                "--env",
                "MINIMAX_API_KEY",
            ])]
        );
        assert_eq!(
            provider_command_args("setup ollama --type ollama --endpoint http://localhost:11434")
                .expect("provider args"),
            vec![strings(&[
                "config",
                "provider",
                "setup",
                "ollama",
                "--type",
                "ollama",
                "--endpoint",
                "http://localhost:11434",
            ])]
        );
    }
}
