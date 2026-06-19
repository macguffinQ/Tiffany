use std::collections::HashSet;
use std::collections::VecDeque;
use std::process::ExitStatus;
use std::process::Stdio;

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use serde::Deserialize;
use tiffany_event_format as event_format;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::process::Command;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::history_cell::PlainHistoryCell;
use codex_app_server_protocol::UserInput;

const TIFFANY_BLUE: Color = Color::Rgb(10, 186, 181);
const TIFFANY_DARK: Color = Color::Rgb(7, 94, 91);
const TIFFANY_SOFT: Color = Color::Rgb(76, 210, 204);
const HUMANIZE_MAX_CHARS: usize = 240_000;

#[derive(Clone, Debug)]
pub struct TiffanyOrchestratorLaunch {
    pub bin: String,
    pub user_prompt: String,
    pub prompt: String,
    pub extra_args: Vec<String>,
    pub config_path: Option<String>,
    pub context_turn_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct TiffanyOrchestratorConfig {
    pub(crate) bin: String,
    pub(crate) extra_args: Vec<String>,
    pub(crate) config_path: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TiffanyOrchestratorTurn {
    pub(crate) user_prompt: String,
    pub(crate) result: String,
}

impl TiffanyOrchestratorLaunch {
    pub(crate) fn config(&self) -> TiffanyOrchestratorConfig {
        TiffanyOrchestratorConfig {
            bin: self.bin.clone(),
            extra_args: self.extra_args.clone(),
            config_path: self.config_path.clone(),
        }
    }
}

impl TiffanyOrchestratorConfig {
    pub(crate) fn launch(
        &self,
        user_prompt: String,
        prompt: String,
        context_turn_count: usize,
    ) -> TiffanyOrchestratorLaunch {
        TiffanyOrchestratorLaunch {
            bin: self.bin.clone(),
            user_prompt,
            prompt,
            extra_args: self.extra_args.clone(),
            config_path: self.config_path.clone(),
            context_turn_count,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct TiffanyProgressEvent {
    role: String,
    status: String,
    message: String,
    task_id: Option<String>,
    agent: Option<String>,
    worker_role: Option<String>,
    runtime: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    task_prompt: Option<String>,
    content: Option<String>,
    approved: Option<bool>,
    issues: Option<usize>,
    count: Option<usize>,
    duration_ms: Option<u64>,
}

#[derive(Default)]
struct BridgeState {
    seen_outputs: HashSet<String>,
    seen_statuses: HashSet<String>,
    malformed_stdout: VecDeque<String>,
    final_output: Option<String>,
}

pub(crate) fn spawn_event_bridge(app_event_tx: AppEventSender, launch: TiffanyOrchestratorLaunch) {
    tokio::spawn(async move {
        emit_intro(&app_event_tx, &launch);
        if let Err(err) = run_event_bridge(app_event_tx.clone(), launch).await {
            emit_lines(
                &app_event_tx,
                vec![status_line(
                    "✗",
                    Color::Red,
                    "orchestrator",
                    &format!("event bridge failed - {err}"),
                )],
            );
        }
        app_event_tx.send(AppEvent::TiffanyOrchestratorRunFinished);
    });
}

pub(crate) fn spawn_roles_command(
    app_event_tx: AppEventSender,
    config: TiffanyOrchestratorConfig,
    args: String,
) {
    tokio::spawn(async move {
        let command_args = match roles_command_args(&args) {
            Ok(command_args) => command_args,
            Err(err) => {
                emit_lines(
                    &app_event_tx,
                    vec![
                        status_line("✗", Color::Red, "roles", &err),
                        body_line(
                            "Usage: /roles [list|show <role>|register <role> --model <model-id> --runtime <runtime-id>]",
                            true,
                        ),
                    ],
                );
                return;
            }
        };

        match run_roles_command(&config, &command_args).await {
            Ok(output) => emit_roles_output(&app_event_tx, &command_args, output),
            Err(err) => emit_lines(
                &app_event_tx,
                spawn_error_lines(
                    "roles",
                    "✗",
                    Color::Red,
                    &format!("failed to run roles command - {err}"),
                    &err.to_string(),
                ),
            ),
        }
    });
}

pub(crate) fn spawn_provider_command(
    app_event_tx: AppEventSender,
    config: TiffanyOrchestratorConfig,
    args: String,
) {
    tokio::spawn(async move {
        let command_args = match provider_command_args(&args) {
            Ok(command_args) => command_args,
            Err(err) => {
                emit_lines(
                    &app_event_tx,
                    vec![
                        status_line("✗", Color::Red, "provider", &err),
                        body_line(
                            "Usage: /provider [setup|list|delete <provider>|key <provider> <key-or-$ENV>|endpoint <provider> <url>]",
                            true,
                        ),
                    ],
                );
                return;
            }
        };

        for args in command_args {
            match run_orchestrator_command(&config, &args).await {
                Ok(output) => emit_command_output(&app_event_tx, "provider", &args, output),
                Err(err) => emit_lines(
                    &app_event_tx,
                    spawn_error_lines(
                        "provider",
                        "✗",
                        Color::Red,
                        &format!("failed to run provider command - {err}"),
                        &err.to_string(),
                    ),
                ),
            }
        }
    });
}

pub(crate) fn idle_intro_lines() -> Vec<Line<'static>> {
    vec![
        brand_line("orchestration shell"),
        idle_status_line(),
        workflow_line(),
        command_hint_line(),
    ]
}

pub(crate) fn contextual_prompt(
    turns: &VecDeque<TiffanyOrchestratorTurn>,
    current_prompt: &str,
) -> String {
    let current_prompt = current_prompt.trim();
    if turns.is_empty() {
        return current_prompt.to_string();
    }

    let recent = turns
        .iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|turn| {
            format!(
                "user:\n{}\n\nassistant result:\n{}",
                truncate_context_text(&turn.user_prompt, 1_200),
                truncate_context_text(&turn.result, 3_000)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    format!(
        "You are continuing a multi-turn tiffany-loop orchestrator conversation.\n\
         Use the previous turns to resolve follow-ups, pronouns, and references.\n\
         The current user request below is the highest priority.\n\n\
         Previous turns:\n{}\n\n\
         ---\n\
         Current user request:\n{}",
        recent, current_prompt
    )
}

fn truncate_context_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
}

pub(crate) fn prompt_from_app_command(op: &AppCommand) -> Option<String> {
    let AppCommand::UserTurn { items, .. } = op else {
        return None;
    };
    prompt_from_user_inputs(items)
}

fn prompt_from_user_inputs(items: &[UserInput]) -> Option<String> {
    let text = items
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.trim().is_empty()).then_some(text)
}

async fn run_event_bridge(
    app_event_tx: AppEventSender,
    launch: TiffanyOrchestratorLaunch,
) -> anyhow::Result<()> {
    let mut command = orchestrator_command(&launch.bin, launch.config_path.as_deref());
    let mut child = command
        .arg("events")
        .arg(&launch.prompt)
        .args(&launch.extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("orchestrator stdout was not captured"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("orchestrator stderr was not captured"))?;

    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });

    let mut state = BridgeState::default();
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<TiffanyProgressEvent>(trimmed) {
            Ok(event) => state.emit_event(&app_event_tx, event),
            Err(_) => state.remember_malformed_stdout(trimmed),
        }
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    if status.success() {
        if let Some(result) = state.final_output.take() {
            app_event_tx.send(AppEvent::TiffanyOrchestratorTurnCaptured {
                user_prompt: launch.user_prompt,
                result,
            });
        }
    } else {
        emit_lines(
            &app_event_tx,
            orchestrator_failure_lines(status, &stderr, &state.malformed_stdout_text()),
        );
    }

    Ok(())
}

async fn run_roles_command(
    config: &TiffanyOrchestratorConfig,
    command_args: &[String],
) -> anyhow::Result<std::process::Output> {
    run_orchestrator_command(config, command_args).await
}

async fn run_orchestrator_command(
    config: &TiffanyOrchestratorConfig,
    command_args: &[String],
) -> anyhow::Result<std::process::Output> {
    let mut command = orchestrator_command(&config.bin, config.config_path.as_deref());
    Ok(command.args(command_args).output().await?)
}

fn orchestrator_command(bin: &str, config_path: Option<&str>) -> Command {
    let mut command = Command::new(bin);
    if let Some(config_path) = config_path.filter(|path| !path.trim().is_empty()) {
        command.arg("--config").arg(config_path);
    }
    command
}

fn roles_command_args(args: &str) -> Result<Vec<String>, String> {
    let parts = args
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Ok(vec!["roles".to_string(), "list".to_string()]);
    }

    match parts[0].as_str() {
        "list" | "show" | "register" => {
            let mut command_args = vec!["roles".to_string()];
            command_args.extend(parts);
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

fn provider_command_args(args: &str) -> Result<Vec<Vec<String>>, String> {
    let parts = args
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Ok(vec![vec!["config".to_string(), "show".to_string()]]);
    }

    match parts[0].as_str() {
        "list" | "show" | "status" => Ok(vec![vec![
            "config".to_string(),
            "provider".to_string(),
            "list".to_string(),
        ]]),
        "setup" => provider_setup_args(&parts[1..]),
        "delete" | "remove" | "rm" => provider_delete_args(&parts[1..]),
        "key" | "set-key" => provider_key_args(&parts[1..]),
        "env" | "set-env" => provider_env_key_args(&parts[1..]),
        "endpoint" | "base-url" | "url" | "set-endpoint" => provider_endpoint_args(&parts[1..]),
        _provider if parts.len() == 1 => Ok(vec![vec!["config".to_string(), "show".to_string()]]),
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
        provider.clone(),
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

fn emit_roles_output(
    app_event_tx: &AppEventSender,
    command_args: &[String],
    output: std::process::Output,
) {
    emit_command_output(app_event_tx, "roles", command_args, output);
}

fn emit_command_output(
    app_event_tx: &AppEventSender,
    label: &'static str,
    command_args: &[String],
    output: std::process::Output,
) {
    if output.status.success()
        && let Some(lines) = concise_success_lines(label, command_args, &output)
    {
        emit_lines(app_event_tx, lines);
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut lines = vec![status_line(
        if output.status.success() {
            "✓"
        } else {
            "✗"
        },
        if output.status.success() {
            TIFFANY_BLUE
        } else {
            Color::Red
        },
        label,
        if output.status.success() {
            "done"
        } else {
            "command failed"
        },
    )];

    let mut emitted_body = false;
    for line in stdout.lines().chain(stderr.lines()) {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        emitted_body = true;
        lines.push(body_line(line, !output.status.success()));
    }
    if !emitted_body {
        lines.push(body_line("no output", true));
    }
    if !output.status.success()
        && let Some(hint) = permission_hint(&format!("{stdout}\n{stderr}"))
    {
        lines.push(body_line(hint, true));
    }
    emit_lines(app_event_tx, lines);
}

fn concise_success_lines(
    label: &'static str,
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    match label {
        "roles" => concise_roles_success(command_args),
        "provider" => concise_provider_success(command_args, output),
        _ => None,
    }
}

fn concise_roles_success(command_args: &[String]) -> Option<Vec<Line<'static>>> {
    if command_args.get(0).map(String::as_str) != Some("roles") {
        return None;
    }
    match command_args.get(1).map(String::as_str) {
        Some("register") => {
            let role = command_args.get(2).map(String::as_str).unwrap_or("<role>");
            let model = flag_value(command_args, "--model-name")
                .or_else(|| flag_value(command_args, "--model"))
                .unwrap_or("<model>");
            let provider = flag_value(command_args, "--provider").unwrap_or("existing");
            let runtime = flag_value(command_args, "--runtime").unwrap_or("runtime");
            Some(vec![status_line(
                "✓",
                TIFFANY_BLUE,
                "role",
                &format!("saved {role} -> {model} via {provider}/{runtime}"),
            )])
        }
        Some("list") | Some("show") => None,
        _ => None,
    }
}

fn concise_provider_success(
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    match (
        command_args.get(0).map(String::as_str),
        command_args.get(1).map(String::as_str),
        command_args.get(2).map(String::as_str),
    ) {
        (Some("config"), Some("provider"), Some("setup")) => {
            let provider = command_args
                .get(3)
                .map(String::as_str)
                .unwrap_or("<provider>");
            let rest = command_args.get(4..).unwrap_or(&[]);
            let mut lines = vec![status_line(
                "✓",
                TIFFANY_BLUE,
                "provider",
                &format!("saved {provider}"),
            )];
            if let Some(env) = flag_value(rest, "--env") {
                lines.push(body_line(&format!("auth: ${env}"), true));
            } else if flag_value(rest, "--key").is_some() {
                lines.push(body_line("auth: literal key", true));
            }
            if let Some(endpoint) = flag_value(rest, "--endpoint") {
                lines.push(body_line(&format!("endpoint: {endpoint}"), true));
            }
            Some(lines)
        }
        (Some("config"), Some("provider"), Some("delete")) => {
            let provider = command_args
                .get(3)
                .map(String::as_str)
                .unwrap_or("<provider>");
            Some(vec![status_line(
                "✓",
                TIFFANY_BLUE,
                "provider",
                &format!("deleted {provider}"),
            )])
        }
        (Some("config"), Some("set-endpoint"), _) => {
            let provider = command_args
                .get(2)
                .map(String::as_str)
                .unwrap_or("<provider>");
            let url = command_args.get(3).map(String::as_str).unwrap_or("<url>");
            Some(vec![status_line(
                "✓",
                TIFFANY_BLUE,
                "provider",
                &format!("{provider} endpoint -> {url}"),
            )])
        }
        (Some("config"), Some("set-key"), _) => {
            let provider = command_args
                .get(2)
                .map(String::as_str)
                .unwrap_or("<provider>");
            Some(vec![status_line(
                "✓",
                TIFFANY_BLUE,
                "provider",
                &format!("{provider} auth updated"),
            )])
        }
        (Some("config"), Some("provider"), Some("list")) | (Some("config"), Some("show"), _) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut lines = vec![status_line("✓", TIFFANY_BLUE, "provider", "configured")];
            lines.extend(
                stdout
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .take(12)
                    .map(|line| body_line(line.trim_end(), false)),
            );
            Some(lines)
        }
        _ => None,
    }
}

fn spawn_error_lines(
    label: &'static str,
    symbol: &'static str,
    color: Color,
    message: &str,
    raw_error: &str,
) -> Vec<Line<'static>> {
    let mut lines = vec![status_line(symbol, color, label, message)];
    if let Some(hint) = permission_hint(raw_error) {
        lines.push(body_line(hint, true));
    }
    lines
}

fn orchestrator_failure_lines(
    status: ExitStatus,
    stderr: &str,
    malformed_stdout: &str,
) -> Vec<Line<'static>> {
    let mut lines = vec![status_line(
        "✗",
        Color::Red,
        "orchestrator",
        &format!("exited with status {status}"),
    )];

    let mut detail_text = stderr.trim().to_string();
    if !malformed_stdout.trim().is_empty() {
        if !detail_text.is_empty() {
            detail_text.push('\n');
        }
        detail_text.push_str(malformed_stdout.trim());
    }
    if detail_text.is_empty() {
        if let Some(log_tail) = read_orchestrator_tui_log_tail(10) {
            lines.push(body_line(
                "stderr was empty; recent ~/.orchestrator/tui.log:",
                true,
            ));
            detail_text = log_tail;
        } else {
            lines.push(body_line(
                "stderr was empty and ~/.orchestrator/tui.log could not be read",
                true,
            ));
        }
    }

    let mut emitted = false;
    for line in nonempty_tail_lines(&detail_text, 12) {
        emitted = true;
        lines.push(body_line(&line, true));
    }
    if !emitted {
        lines.push(body_line("no error detail captured", true));
    }

    if let Some(hint) = diagnostic_hint(&detail_text) {
        lines.push(body_line(hint, true));
    }
    lines.push(body_line(
        "next: run `orchestrator doctor` or open `/doctor` in this TUI",
        true,
    ));
    lines
}

fn permission_hint(error: &str) -> Option<&'static str> {
    diagnostic_hint(error)
}

fn diagnostic_hint(error: &str) -> Option<&'static str> {
    let lower = error.to_ascii_lowercase();
    if lower.contains("reading config") || lower.contains("config.yaml") {
        return Some("hint: run `orchestrator init`, then configure providers and roles");
    }
    if lower.contains("missing api key") || lower.contains("api key") {
        return Some(
            "hint: configure provider auth with `/provider` or `orchestrator config provider`",
        );
    }
    if lower.contains("no default worker role")
        || lower.contains("role") && (lower.contains("not found") || lower.contains("unknown role"))
    {
        return Some(
            "hint: register/select a worker with `/role`, `/roles`, or `orchestrator roles register`",
        );
    }
    if lower.contains("failed to spawn")
        || lower.contains("cannot find")
        || lower.contains("command not found")
        || lower.contains("no such file or directory")
    {
        return Some(
            "hint: install the selected runtime binary (`claude` or `codex`) or update the role runtime",
        );
    }
    if lower.contains("permission denied") || lower.contains("operation not permitted") {
        return Some(
            "hint: check file permissions, executable bit, macOS privacy access, or run /doctor",
        );
    }
    if lower.contains("model not found")
        || lower.contains("unknown model")
        || lower.contains("invalid model")
        || lower.contains("模型不存在")
        || lower.contains("1211")
    {
        return Some(
            "hint: check `/role <role>`: model id must point to the provider API model name; then run `/doctor`",
        );
    }
    if lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("401")
        || lower.contains("403")
    {
        return Some(
            "hint: check provider API key or endpoint; use /provider, `orchestrator config provider`, or /doctor",
        );
    }
    None
}

fn nonempty_tail_lines(text: &str, max_lines: usize) -> Vec<String> {
    let lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max_lines);
    lines.into_iter().skip(start).collect()
}

fn read_orchestrator_tui_log_tail(max_lines: usize) -> Option<String> {
    let path = orchestrator_tui_log_path()?;
    let contents = std::fs::read_to_string(path).ok()?;
    let tail = nonempty_tail_lines(&contents, max_lines).join("\n");
    (!tail.trim().is_empty()).then_some(tail)
}

fn orchestrator_tui_log_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .map(|home| home.join(".orchestrator").join("tui.log"))
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|window| window.first().is_some_and(|value| value == flag))
        .and_then(|window| window.get(1))
        .map(String::as_str)
}

impl BridgeState {
    fn emit_event(&mut self, app_event_tx: &AppEventSender, event: TiffanyProgressEvent) {
        let visible = visible_content(&event);
        if event.status == "output" && visible.is_none() {
            return;
        }

        if event.status == "output"
            && let Some(content) = visible.as_deref()
        {
            if is_redundant_role_output(&event.role, content) {
                return;
            }
            if event.role == "worker" {
                if let Some(result) = event.content.as_deref().and_then(final_output_candidate) {
                    remember_better_text(&mut self.final_output, result);
                }
            }
            let key = normalized_output_key(content);
            let scope = output_scope(&event);
            if !self.seen_outputs.insert(format!("{scope}:{key}")) {
                return;
            }
            if event.role == "worker" {
                remember_better_text(&mut self.final_output, content.to_string());
            }
        } else {
            let title = event_title(&event);
            let key = format!("{}:{}:{title}", event.role, event.status);
            if !self.seen_statuses.insert(key) {
                return;
            }
        }

        let captured_result = event.role == "orchestrator"
            && event.status == "done"
            && self
                .final_output
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty());
        emit_event_lines(app_event_tx, &event, visible);
        if captured_result {
            emit_lines(app_event_tx, vec![memory_capture_line()]);
        }
    }

    fn remember_malformed_stdout(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || is_ignorable_stdout_noise(line) {
            return;
        }
        self.malformed_stdout.push_back(line.to_string());
        while self.malformed_stdout.len() > 16 {
            self.malformed_stdout.pop_front();
        }
    }

    fn malformed_stdout_text(&self) -> String {
        self.malformed_stdout
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn emit_intro(app_event_tx: &AppEventSender, launch: &TiffanyOrchestratorLaunch) {
    emit_lines(
        app_event_tx,
        vec![
            brand_line("orchestration run"),
            run_status_line(launch),
            workflow_line(),
        ],
    );
}

fn brand_line(subtitle: &'static str) -> Line<'static> {
    let mut spans = tiffany_logo_spans();
    spans.extend([
        Span::styled("orchestrator", Style::default().fg(TIFFANY_SOFT)),
        Span::raw("  "),
        Span::styled(subtitle, Style::default().fg(TIFFANY_DARK)),
    ]);
    Line::from(spans)
}

fn tiffany_logo_spans() -> Vec<Span<'static>> {
    vec![
        Span::styled(
            "◆",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "T",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            ">",
            Style::default()
                .fg(TIFFANY_SOFT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "_",
            Style::default()
                .fg(TIFFANY_DARK)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "tiffany-loop",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]
}

fn idle_status_line() -> Line<'static> {
    Line::from(vec![
        status_chip("status", "ready", TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("mode", "native", TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("memory", "on", TIFFANY_BLUE),
    ])
}

fn run_status_line(launch: &TiffanyOrchestratorLaunch) -> Line<'static> {
    let context = if launch.context_turn_count == 0 {
        "new".to_string()
    } else {
        format!("{} turn(s)", launch.context_turn_count)
    };
    Line::from(vec![
        status_chip("status", "running", TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("route", route_label(&launch.extra_args), TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("context", context, TIFFANY_SOFT),
        Span::raw("  "),
        status_chip("config", config_label(launch), TIFFANY_SOFT),
    ])
}

fn workflow_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "flow",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("planner", Style::default().fg(TIFFANY_SOFT)),
        Span::styled(" → ", Style::default().fg(TIFFANY_DARK)),
        Span::styled("critic", Style::default().fg(TIFFANY_SOFT)),
        Span::styled(" → ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            "worker",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" → ", Style::default().fg(TIFFANY_DARK)),
        Span::styled("reviewer", Style::default().fg(TIFFANY_SOFT)),
    ])
}

fn command_hint_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "cmd",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("/provider", Style::default().fg(TIFFANY_SOFT)),
        Span::raw("  "),
        Span::styled("/role", Style::default().fg(TIFFANY_SOFT)),
        Span::raw("  "),
        Span::styled("/roles", Style::default().fg(TIFFANY_SOFT)),
        Span::raw("  "),
        Span::styled("/doctor", Style::default().fg(TIFFANY_SOFT)),
    ])
}

fn memory_capture_line() -> Line<'static> {
    Line::from(vec![
        Span::styled("✓", Style::default().fg(TIFFANY_BLUE)),
        Span::raw(" "),
        Span::styled("memory", Style::default().fg(TIFFANY_BLUE)),
        Span::raw("  "),
        Span::styled(
            "result captured for follow-up",
            Style::default().fg(TIFFANY_SOFT),
        ),
    ])
}

fn status_chip(
    label: impl Into<String>,
    value: impl Into<String>,
    value_color: Color,
) -> Span<'static> {
    Span::styled(
        format!("{} {}", label.into(), value.into()),
        Style::default().fg(value_color),
    )
}

fn route_label(args: &[String]) -> String {
    flag_value(args, "--worker")
        .unwrap_or("worker-cc")
        .to_string()
}

fn config_label(launch: &TiffanyOrchestratorLaunch) -> &'static str {
    if launch
        .config_path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
    {
        "custom"
    } else {
        "default"
    }
}

fn emit_event_lines(
    app_event_tx: &AppEventSender,
    event: &TiffanyProgressEvent,
    visible: Option<String>,
) {
    if event.status == "output" {
        if let Some(content) = visible {
            emit_lines(app_event_tx, output_event_lines(event, &content));
        }
        return;
    }

    let mut lines = vec![status_line(
        status_symbol(&event),
        status_color(&event),
        &event.role,
        &event_title(&event),
    )];
    lines.extend(event_detail_lines(event));

    emit_lines(app_event_tx, lines);
}

fn output_event_lines(event: &TiffanyProgressEvent, content: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "↳",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            output_title(event),
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    lines.extend(content.lines().map(output_body_line));
    lines
}

fn emit_lines(app_event_tx: &AppEventSender, lines: Vec<Line<'static>>) {
    if lines.is_empty() {
        return;
    }
    app_event_tx.send(AppEvent::InsertHistoryCell(Box::new(
        PlainHistoryCell::new(lines),
    )));
}

fn status_symbol(event: &TiffanyProgressEvent) -> &'static str {
    match event.status.as_str() {
        "done" => "✓",
        "failed" => "✗",
        "warning" => "⚠",
        "output" => "↳",
        _ => "●",
    }
}

fn status_color(event: &TiffanyProgressEvent) -> Color {
    match event.status.as_str() {
        "done" => TIFFANY_BLUE,
        "failed" => Color::Red,
        "warning" => Color::Yellow,
        "output" => Color::Gray,
        _ => TIFFANY_BLUE,
    }
}

fn event_title(event: &TiffanyProgressEvent) -> String {
    if event.role == "worker" && event.status == "running" && event.task_id.is_some() {
        let role = event
            .worker_role
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| event.agent.as_deref().unwrap_or("worker"));
        let runtime = event
            .runtime
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(event.agent.as_deref())
            .unwrap_or("runtime");
        let route = match provider_model_label(event) {
            Some(model) => format!("{role} · {runtime} · {model}"),
            None => format!("{role} · {runtime}"),
        };
        if let Some(id) = short_task_id(event.task_id.as_deref()) {
            return format!("{route} · {id}");
        }
        return route;
    }
    if event.role == "worker"
        && matches!(event.status.as_str(), "done" | "failed")
        && event.task_id.is_some()
    {
        let role = event
            .worker_role
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| event.agent.as_deref().unwrap_or("worker"));
        let status = if event.status == "done" {
            "done"
        } else {
            "failed"
        };
        let runtime = event
            .agent
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let mut title = match runtime {
            Some(runtime) if runtime != role => format!("{role} {status} · {runtime}"),
            _ => format!("{role} {status}"),
        };
        if let Some(id) = short_task_id(event.task_id.as_deref()) {
            title = format!("{title} · {id}");
        }
        if let Some(duration) = event.duration_ms.map(format_duration_ms) {
            title = format!("{title} · {duration}");
        }
        return title;
    }

    let mut title = event.message.clone();
    if let Some(agent) = event.agent.as_deref()
        && !title.contains(agent)
    {
        title = format!("{agent} - {title}");
    }
    if let Some(id) = short_task_id(event.task_id.as_deref()) {
        title = format!("{title} · {id}");
    }
    if event.status == "warning"
        && let Some(issues) = event.issues
        && !title.contains("issue")
    {
        title = format!("{title} - {issues} issue(s)");
    }
    if event.status == "done"
        && let Some(count) = event.count
        && !title.contains("task")
        && event.role == "orchestrator"
    {
        title = format!("{title} - {count} task(s)");
    }
    if let Some(approved) = event.approved
        && event.role == "reviewer"
        && !title.contains("approved")
        && !title.contains("fix")
    {
        title = format!("{title} - approved={approved}");
    }
    title
}

fn output_title(event: &TiffanyProgressEvent) -> String {
    let label = output_label(event);
    let suffix = match event.role.as_str() {
        "planner" => "plan",
        "critic" => "critique",
        "reviewer" => "review",
        "worker" => "output",
        _ => "details",
    };
    format!("{label} {suffix}")
}

fn output_label(event: &TiffanyProgressEvent) -> String {
    let role = event
        .worker_role
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let agent = event
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut label = match (role, agent) {
        (Some(role), Some(agent)) if event.role == "worker" && role != agent => {
            format!("{role} · {agent}")
        }
        (Some(role), _) => role.to_string(),
        (None, Some(agent)) => agent.to_string(),
        (None, None) => event.role.clone(),
    };
    if let Some(id) = short_task_id(event.task_id.as_deref()) {
        label.push_str(" · ");
        label.push_str(id);
    }
    label
}

fn event_detail_lines(event: &TiffanyProgressEvent) -> Vec<Line<'static>> {
    if event.role != "worker" || event.status != "running" || event.task_id.is_none() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if let Some(model) = provider_model_label(event) {
        lines.push(body_line(&format!("model: {model}"), true));
    }
    if let Some(prompt) = event
        .task_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(body_line(
            &format!("task: {}", truncate_text(prompt, 180)),
            false,
        ));
    }
    lines
}

fn provider_model_label(event: &TiffanyProgressEvent) -> Option<String> {
    let model = event
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(
        match event
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(provider) => format!("{provider}/{model}"),
            None => model.to_string(),
        },
    )
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return format!("{duration_ms}ms");
    }
    let seconds = duration_ms / 1_000;
    let millis = duration_ms % 1_000;
    if seconds < 60 {
        if millis == 0 {
            return format!("{seconds}s");
        }
        return format!("{seconds}.{}s", millis / 100);
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}m{seconds:02}s")
}

fn visible_content(event: &TiffanyProgressEvent) -> Option<String> {
    let content = event.content.as_deref()?.trim();
    if content.is_empty() {
        return None;
    }

    let cleaned = content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return Some(String::new());
            }
            if trimmed == "thinking" || trimmed.ends_with(" system: system") {
                return None;
            }
            Some(strip_runtime_prefix(line))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if cleaned.is_empty() {
        return None;
    }
    let humanized = event_format::humanize_jsonish(&cleaned, HUMANIZE_MAX_CHARS);
    let display = strip_redundant_role_prefix(&event.role, &strip_final_heading(&humanized));
    let display = format_tiffany_summary_style(&display);
    if is_low_value_output(&display) {
        return None;
    }
    (!display.trim().is_empty()).then_some(display)
}

fn strip_redundant_role_prefix(role: &str, content: &str) -> String {
    if !matches!(role, "planner" | "critic" | "reviewer") {
        return content.trim().to_string();
    }
    let trimmed = content.trim();
    let lower = trimmed.to_ascii_lowercase();
    let role = role.to_ascii_lowercase();
    for separator in [": ", " - ", " — "] {
        let prefix = format!("{role}{separator}");
        if lower.starts_with(&prefix) {
            return trimmed[prefix.len()..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

fn is_low_value_output(text: &str) -> bool {
    event_format::is_low_value_output(text)
}

fn final_output_candidate(content: &str) -> Option<String> {
    event_format::final_output_candidate(content, HUMANIZE_MAX_CHARS)
        .map(|result| strip_final_heading(&result))
        .filter(|result| !result.trim().is_empty())
}

fn remember_better_text(slot: &mut Option<String>, candidate: String) {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return;
    }
    if slot
        .as_deref()
        .is_none_or(|current| candidate.chars().count() > current.chars().count())
    {
        *slot = Some(candidate.to_string());
    }
}

fn normalized_output_key(content: &str) -> String {
    event_format::normalized_output_key(content, HUMANIZE_MAX_CHARS)
        .unwrap_or_else(|| content.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn output_scope(event: &TiffanyProgressEvent) -> String {
    let mut scope = event.role.clone();
    if let Some(agent) = event.agent.as_deref().filter(|agent| !agent.is_empty()) {
        scope.push(':');
        scope.push_str(agent);
    }
    if let Some(task_id) = short_task_id(event.task_id.as_deref()) {
        scope.push(':');
        scope.push_str(task_id);
    }
    scope
}

fn is_redundant_role_output(role: &str, content: &str) -> bool {
    if role == "worker" {
        return false;
    }
    let lower = content.trim().to_ascii_lowercase();
    match role {
        "planner" => lower.starts_with("plan ready") && !lower.contains('\n'),
        "critic" | "reviewer" => {
            lower.starts_with("approved") && !lower.contains('\n') && !lower.contains("suggestion")
        }
        _ => false,
    }
}

fn format_tiffany_summary_style(content: &str) -> String {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let first = restyle_summary_first_line(first);
    let rest = lines.collect::<Vec<_>>();
    if rest.is_empty() {
        first
    } else {
        format!("{}\n{}", first, rest.join("\n"))
    }
}

fn restyle_summary_first_line(line: &str) -> String {
    for prefix in ["plan ready", "approved", "needs changes"] {
        let needle = format!("{prefix}: ");
        if let Some(rest) = line.strip_prefix(&needle) {
            return format!("{prefix} - {rest}");
        }
    }
    line.to_string()
}

fn strip_runtime_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    for prefix in [
        "claude-code assistant: ",
        "claude-code result: ",
        "claude-code final_answer: ",
        "claude assistant: ",
        "claude result: ",
        "claude final_answer: ",
        "codex assistant: ",
        "codex result: ",
        "codex final_answer: ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.to_string();
        }
    }

    if let Some((prefix, rest)) = trimmed.split_once(": ") {
        let prefix = prefix.to_ascii_lowercase();
        let mut parts = prefix.split_whitespace();
        let runtime = parts.next();
        let kind = parts.next();
        if matches!(runtime, Some("claude" | "claude-code" | "codex"))
            && matches!(
                kind,
                Some("assistant" | "result" | "final" | "final_answer" | "task_complete")
            )
        {
            return rest.to_string();
        }
    }

    line.to_string()
}

fn strip_final_heading(content: &str) -> String {
    let trimmed = content.trim();
    for heading in [
        "final result",
        "final answer",
        "final_result",
        "final_answer",
    ] {
        if trimmed.eq_ignore_ascii_case(heading) {
            return String::new();
        }
        if trimmed
            .get(..heading.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(heading))
        {
            let rest = &trimmed[heading.len()..];
            if let Some(rest) = rest.strip_prefix(':') {
                return rest.trim_start().to_string();
            }
            if rest.starts_with('\n') || rest.starts_with("\r\n") {
                return rest.trim_start().to_string();
            }
        }
    }
    trimmed.to_string()
}

fn is_ignorable_stdout_noise(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    lower.is_empty()
        || lower.starts_with("debug ")
        || lower.starts_with("trace ")
        || lower.contains("tracing::")
        || lower.contains("tokio-runtime-worker")
}

fn truncate_text(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

fn status_line(symbol: &'static str, color: Color, role: &str, message: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            symbol,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(role.to_string(), Style::default().fg(TIFFANY_SOFT)),
        Span::raw("  "),
        Span::raw(message.to_string()),
    ])
}

fn body_line(line: &str, dim: bool) -> Line<'static> {
    let style = if dim {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled("  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(line.to_string(), style),
    ])
}

fn output_body_line(line: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  │ ", Style::default().fg(TIFFANY_DARK)),
        Span::raw(line.to_string()),
    ])
}

fn short_task_id(task_id: Option<&str>) -> Option<&str> {
    task_id.map(|id| id.get(..8).unwrap_or(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn intro_lines_have_tiffany_blue_structure() {
        let lines = idle_intro_lines();
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("◆ T>_  tiffany-loop orchestrator"));
        assert!(text.contains("status ready"));
        assert!(text.contains("flow  planner → critic → worker → reviewer"));
        assert!(text.contains("cmd  /provider  /role  /roles  /doctor"));
        assert_eq!(lines[0].spans[0].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[0].spans[2].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[0].spans[3].style.fg, Some(TIFFANY_SOFT));
        assert_eq!(lines[2].spans[0].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[3].spans[2].style.fg, Some(TIFFANY_SOFT));
    }

    #[test]
    fn output_lines_use_tiffany_blue_source_and_gutter() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "claude output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("done".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        let lines = output_event_lines(&event, "done");

        assert_eq!(line_text(&lines[0]), "↳ claude-code · 12345678 output");
        assert_eq!(line_text(&lines[1]), "  │ done");
        assert_eq!(lines[0].spans[0].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[0].spans[2].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[1].spans[0].style.fg, Some(TIFFANY_DARK));
    }

    #[test]
    fn controller_output_uses_waterfall_title_without_prompt_marker() {
        let event = TiffanyProgressEvent {
            role: "critic".to_string(),
            status: "output".to_string(),
            message: "critic output".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some(
                "critic>\n{\"approved\":false,\"issues\":[\"missing test\"]}".to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let visible = visible_content(&event).expect("critic output visible");
        let lines = output_event_lines(&event, &visible);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("↳ critic critique"));
        assert!(text.contains("needs changes"));
        assert!(text.contains("missing test"));
        assert!(!text.contains("critic:"));
        assert!(!text.contains("critic>"));
        assert!(!text.contains('{'));
    }

    #[test]
    fn worker_output_title_keeps_route_and_runtime() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "worker output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("running tests".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        let lines = output_event_lines(&event, "running tests");

        assert_eq!(
            line_text(&lines[0]),
            "↳ worker-cc · claude-code · 12345678 output"
        );
    }

    #[test]
    fn worker_started_lines_show_route_model_and_task() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            task_prompt: Some("回答用户的问题，并给出清晰结论".to_string()),
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        let mut lines = vec![status_line(
            status_symbol(&event),
            status_color(&event),
            &event.role,
            &event_title(&event),
        )];
        lines.extend(event_detail_lines(&event));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"));
        assert!(text.contains("model: minimax/MiniMax-M3"));
        assert!(text.contains("task: 回答用户的问题"));
    }

    #[test]
    fn worker_done_title_shows_duration() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "done".to_string(),
            message: "worker-cc done".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: Some(1_250),
        };

        assert_eq!(
            event_title(&event),
            "worker-cc done · claude-code · 12345678 · 1.2s"
        );
    }

    #[test]
    fn summarizes_structured_reviewer_json() {
        let event = TiffanyProgressEvent {
            role: "reviewer".to_string(),
            status: "output".to_string(),
            message: "reviewer output".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("{\"approved\":true}".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        assert_eq!(
            visible_content(&event).as_deref(),
            Some("approved - 0 issue(s)")
        );
    }

    #[test]
    fn expands_structured_reviewer_issues() {
        let event = TiffanyProgressEvent {
            role: "critic".to_string(),
            status: "output".to_string(),
            message: "critic output".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some(
                r#"{"approved":false,"issues":["worker prompt conflicts with direct answer"],"suggestions":["answer in Chinese"]}"#
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        let visible = visible_content(&event).expect("visible critic output");

        assert!(visible.contains("needs changes - 1 issue(s)"));
        assert!(visible.contains("worker prompt conflicts with direct answer"));
        assert!(visible.contains("suggestions:"));
    }

    #[test]
    fn shows_actionable_stderr_instead_of_hiding_model_errors() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "codex output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("worker-codex".to_string()),
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("worker-codex stderr: [1211] 模型不存在".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        assert_eq!(
            visible_content(&event).as_deref(),
            Some("worker-codex stderr: [1211] 模型不存在")
        );
    }

    #[test]
    fn keeps_worker_markdown_output() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "claude output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("结果\n- done".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        assert_eq!(visible_content(&event).as_deref(), Some("结果\n- done"));
        assert_eq!(
            event_title(&event),
            "claude-code - claude output · 12345678"
        );
    }

    #[test]
    fn strips_runtime_prefix_for_manual_style_output() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "claude output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("claude-code assistant: done".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        assert_eq!(visible_content(&event).as_deref(), Some("done"));
        assert_eq!(output_label(&event), "claude-code · 12345678");
    }

    #[test]
    fn strips_redundant_final_heading_from_worker_output() {
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "claude output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("Final result\n你好！\n我可以帮你写代码。".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        assert_eq!(
            visible_content(&event).as_deref(),
            Some("你好！\n我可以帮你写代码。")
        );
    }

    #[test]
    fn extracts_final_result_from_worker_result_output() {
        assert_eq!(
            final_output_candidate("claude-code result: 完成\n- ok").as_deref(),
            Some("完成\n- ok")
        );
        assert_eq!(
            final_output_candidate(r#"claude-code result: {"result":"完成并验证"}"#).as_deref(),
            Some("完成并验证")
        );
        assert_eq!(
            final_output_candidate("claude-code result: Final answer:\n完成并验证").as_deref(),
            Some("完成并验证")
        );
    }

    #[test]
    fn output_dedupe_key_ignores_runtime_event_kind_after_visible_cleanup() {
        let assistant = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "claude output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some("claude-code assistant: useful summary".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let result = TiffanyProgressEvent {
            content: Some("claude-code result: useful summary".to_string()),
            ..assistant.clone()
        };

        let assistant_visible = visible_content(&assistant).expect("assistant visible");
        let result_visible = visible_content(&result).expect("result visible");

        assert_eq!(assistant_visible, "useful summary");
        assert_eq!(assistant_visible, result_visible);
        assert_eq!(output_scope(&assistant), output_scope(&result));
        assert_eq!(
            normalized_output_key(&assistant_visible),
            normalized_output_key(&result_visible)
        );
    }

    #[test]
    fn hides_redundant_role_output_summaries() {
        assert!(is_redundant_role_output(
            "planner",
            "plan ready - 1 sub-task(s)"
        ));
        assert!(!is_redundant_role_output(
            "planner",
            "plan ready - 1 sub-task(s)\n  1. answer the user"
        ));
        assert!(is_redundant_role_output("critic", "approved (0 issue(s))"));
        assert!(!is_redundant_role_output(
            "reviewer",
            "needs changes (2 issue(s))"
        ));
        assert!(!is_redundant_role_output(
            "reviewer",
            "approved - 0 issue(s)\n  suggestions:\n  - tighten wording"
        ));
        assert!(!is_redundant_role_output("worker", "plan ready - no"));
    }

    #[test]
    fn keeps_structured_role_details_without_raw_json() {
        let planner = TiffanyProgressEvent {
            role: "planner".to_string(),
            status: "output".to_string(),
            message: "planner output".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some(
                r#"{"sub_tasks":[{"prompt":"answer in Chinese","agent_hint":"worker-cc"}]}"#
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let planner_visible = visible_content(&planner).expect("planner details visible");
        assert!(planner_visible.contains("plan ready"));
        assert!(planner_visible.contains("answer in Chinese"));
        assert!(planner_visible.contains("worker-cc"));
        assert!(!is_redundant_role_output(&planner.role, &planner_visible));
        assert!(!planner_visible.contains('{'));

        let reviewer = TiffanyProgressEvent {
            role: "reviewer".to_string(),
            status: "output".to_string(),
            message: "reviewer output".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some(r#"{"approved":false,"issues":["missing final answer"],"suggestions":["return a concise result"]}"#.to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let reviewer_visible = visible_content(&reviewer).expect("review issue visible");
        assert!(reviewer_visible.contains("needs changes"));
        assert!(reviewer_visible.contains("missing final answer"));
        assert!(reviewer_visible.contains("return a concise result"));
        assert!(!is_redundant_role_output(&reviewer.role, &reviewer_visible));
        assert!(!reviewer_visible.contains('{'));
    }

    #[test]
    fn humanizes_truncated_role_json_without_raw_leakage() {
        let critic = TiffanyProgressEvent {
            role: "critic".to_string(),
            status: "output".to_string(),
            message: "critic output".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some(
                "critic>\n  {\"approved\": false, \"issues\": [\"Self-contradictory: delegated despite direct-answer instruction\",\n  \"Unnecessary delegation"
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        let visible = visible_content(&critic).expect("visible critic output");

        assert!(visible.contains("needs changes - 2 issue(s)"));
        assert!(visible.contains("Self-contradictory"));
        assert!(visible.contains("Unnecessary delegation"));
        assert!(!visible.contains('{'));
        assert!(!visible.contains("\"issues\""));
        assert!(!visible.contains("critic>"));
    }

    #[test]
    fn humanizes_truncated_planner_json_without_raw_leakage() {
        let planner = TiffanyProgressEvent {
            role: "planner".to_string(),
            status: "output".to_string(),
            message: "planner output".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: Some(
                "planner>\n  {\"sub_tasks\": [{\"prompt\": \"Answer in Chinese and keep the result concise"
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        let visible = visible_content(&planner).expect("visible planner output");

        assert!(visible.contains("plan ready - 1 sub-task(s)"));
        assert!(visible.contains("Answer in Chinese"));
        assert!(!visible.contains('{'));
        assert!(!visible.contains("\"sub_tasks\""));
        assert!(!visible.contains("planner>"));
    }

    #[cfg(unix)]
    #[test]
    fn failure_lines_include_stderr_and_next_step() {
        use std::os::unix::process::ExitStatusExt;

        let lines = orchestrator_failure_lines(
            ExitStatus::from_raw(1 << 8),
            "Error: reading config at /tmp/missing/config.yaml",
            "",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("exited with status"));
        assert!(text.contains("reading config"));
        assert!(text.contains("orchestrator init"));
        assert!(text.contains("orchestrator doctor"));
    }

    #[cfg(unix)]
    #[test]
    fn failure_lines_include_cached_malformed_stdout() {
        use std::os::unix::process::ExitStatusExt;

        let lines = orchestrator_failure_lines(
            ExitStatus::from_raw(1 << 8),
            "",
            "2026-06-17T03:34:28Z ERROR planner returned no sub_tasks",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("planner returned no sub_tasks"));
        assert!(!text.contains("ignored malformed event"));
    }

    #[cfg(unix)]
    #[test]
    fn failure_lines_explain_model_mapping_errors() {
        use std::os::unix::process::ExitStatusExt;

        let lines = orchestrator_failure_lines(
            ExitStatus::from_raw(1 << 8),
            "worker-codex stderr: [1211] 模型不存在",
            "",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("模型不存在"));
        assert!(text.contains("model id must point to the provider API model name"));
        assert!(text.contains("/doctor"));
    }

    #[test]
    fn roles_args_default_to_list() {
        assert_eq!(
            roles_command_args("").expect("roles args"),
            strings(&["roles", "list"])
        );
    }

    #[test]
    fn roles_args_pass_through_modern_commands() {
        assert_eq!(
            roles_command_args("show critic").expect("roles args"),
            strings(&["roles", "show", "critic"])
        );
        assert_eq!(
            roles_command_args("register critic --model glm51 --runtime codex --provider openai")
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
                "openai"
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
                "--agent-teams"
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
                "glm-5.1"
            ])
        );
    }

    #[test]
    fn provider_args_default_to_config_show() {
        assert_eq!(
            provider_command_args("").expect("provider args"),
            vec![strings(&["config", "show"])]
        );
        assert_eq!(
            provider_command_args("list").expect("provider args"),
            vec![strings(&["config", "provider", "list"])]
        );
    }

    #[test]
    fn provider_args_support_delete() {
        assert_eq!(
            provider_command_args("delete minimax").expect("provider args"),
            vec![strings(&["config", "provider", "delete", "minimax"])]
        );
        assert_eq!(
            provider_command_args("rm openrouter").expect("provider args"),
            vec![strings(&["config", "provider", "delete", "openrouter"])]
        );
    }

    #[test]
    fn provider_args_support_key_endpoint_and_env_shortcuts() {
        assert_eq!(
            provider_command_args("key openai $OPENAI_API_KEY").expect("provider args"),
            vec![strings(&[
                "config",
                "set-key",
                "openai",
                "--key",
                "$OPENAI_API_KEY"
            ])]
        );
        assert_eq!(
            provider_command_args("env anthropic ANTHROPIC_API_KEY").expect("provider args"),
            vec![strings(&[
                "config",
                "set-key",
                "anthropic",
                "--key",
                "$ANTHROPIC_API_KEY"
            ])]
        );
        assert_eq!(
            provider_command_args("endpoint openai https://api.openai.com/v1")
                .expect("provider args"),
            vec![strings(&[
                "config",
                "set-endpoint",
                "openai",
                "https://api.openai.com/v1"
            ])]
        );
    }

    #[test]
    fn provider_args_support_positional_setup() {
        assert_eq!(
            provider_command_args("openai $OPENAI_API_KEY https://api.openai.com/v1")
                .expect("provider args"),
            vec![
                strings(&["config", "set-key", "openai", "--key", "$OPENAI_API_KEY"]),
                strings(&[
                    "config",
                    "set-endpoint",
                    "openai",
                    "https://api.openai.com/v1"
                ]),
            ]
        );
    }

    #[test]
    fn provider_args_support_setup_form_command() {
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
                "https://api.openai.com/v1"
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
                "http://localhost:11434"
            ])]
        );
    }

    #[test]
    fn remembers_longest_final_output() {
        let mut state = BridgeState::default();
        remember_better_text(&mut state.final_output, "short".to_string());
        remember_better_text(&mut state.final_output, "longer result".to_string());
        remember_better_text(&mut state.final_output, "tiny".to_string());
        assert_eq!(state.final_output.as_deref(), Some("longer result"));
    }

    #[test]
    fn contextual_prompt_includes_recent_tiffany_turns() {
        let mut turns = VecDeque::new();
        turns.push_back(TiffanyOrchestratorTurn {
            user_prompt: "你好".to_string(),
            result: "你好！我是 tiffany-loop orchestrator。".to_string(),
        });

        let prompt = contextual_prompt(&turns, "你叫啥");

        assert!(prompt.contains("multi-turn tiffany-loop orchestrator conversation"));
        assert!(prompt.contains("user:\n你好"));
        assert!(prompt.contains("assistant result:\n你好！我是 tiffany-loop orchestrator。"));
        assert!(prompt.contains("Current user request:\n你叫啥"));
    }

    #[test]
    fn extracts_prompt_from_user_turn() {
        let op = AppCommand::user_turn(
            vec![UserInput::Text {
                text: "  hello  ".to_string(),
                text_elements: Vec::new(),
            }],
            std::path::PathBuf::from("."),
            codex_app_server_protocol::AskForApproval::Never,
            None,
            "test-model".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(prompt_from_app_command(&op).as_deref(), Some("hello"));
    }
}
