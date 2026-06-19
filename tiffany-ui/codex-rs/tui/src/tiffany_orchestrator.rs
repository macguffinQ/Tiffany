use std::collections::{HashMap, HashSet, VecDeque};
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

#[allow(clippy::disallowed_methods)]
const TIFFANY_BLUE: Color = Color::Rgb(10, 186, 181);
#[allow(clippy::disallowed_methods)]
const TIFFANY_DARK: Color = Color::Rgb(7, 94, 91);
#[allow(clippy::disallowed_methods)]
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
    worker_metadata: HashMap<String, WorkerMeta>,
}

#[derive(Clone, Debug, Default)]
struct WorkerMeta {
    agent: Option<String>,
    worker_role: Option<String>,
    runtime: Option<String>,
    model: Option<String>,
    provider: Option<String>,
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

pub(crate) fn spawn_doctor_command(
    app_event_tx: AppEventSender,
    config: TiffanyOrchestratorConfig,
    args: String,
) {
    tokio::spawn(async move {
        let command_args = match doctor_command_args(&args) {
            Ok(command_args) => command_args,
            Err(err) => {
                emit_lines(
                    &app_event_tx,
                    vec![
                        status_line("✗", Color::Red, "doctor", &err),
                        body_line("Usage: /doctor [run]", true),
                    ],
                );
                return;
            }
        };

        match run_orchestrator_command(&config, &command_args).await {
            Ok(output) => emit_doctor_output(&app_event_tx, output),
            Err(err) => emit_lines(
                &app_event_tx,
                spawn_error_lines(
                    "doctor",
                    "✗",
                    Color::Red,
                    &format!("failed to run doctor command - {err}"),
                    &err.to_string(),
                ),
            ),
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
         Previous turns:\n{recent}\n\n\
         ---\n\
         Current user request:\n{current_prompt}",
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
            orchestrator_failure_lines(
                status,
                &stderr,
                &state.malformed_stdout_text(),
                &state.worker_context_lines(),
            ),
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
        "setup" | "edit" => provider_setup_args(&parts[1..]),
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

fn doctor_command_args(args: &str) -> Result<Vec<String>, String> {
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
    if label == "doctor" {
        emit_doctor_output(app_event_tx, output);
        return;
    }

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

fn emit_doctor_output(app_event_tx: &AppEventSender, output: std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = format!("{stdout}{stderr}");
    let has_issues = text.contains("issue(s) found")
        || text.lines().any(|line| line.trim_start().starts_with('✗'));
    let (symbol, color, message) = if !output.status.success() {
        ("✗", Color::Red, "command failed")
    } else if has_issues {
        ("⚠", Color::Yellow, "issues found")
    } else {
        ("✓", TIFFANY_BLUE, "ready")
    };
    let mut lines = vec![status_line(symbol, color, "doctor", message)];
    lines.extend(doctor_repair_lines(&text));
    let mut emitted_body = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        emitted_body = true;
        lines.push(body_line(line, doctor_line_is_dim(line)));
    }
    if !emitted_body {
        lines.push(body_line("no output", true));
    }
    emit_lines(app_event_tx, lines);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DoctorRepairAction {
    command: &'static str,
    detail: &'static str,
}

fn doctor_repair_lines(text: &str) -> Vec<Line<'static>> {
    doctor_repair_actions(text)
        .into_iter()
        .map(|action| {
            Line::from(vec![
                Span::styled(
                    "  repair",
                    Style::default()
                        .fg(TIFFANY_BLUE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    action.command,
                    Style::default()
                        .fg(TIFFANY_SOFT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(action.detail, Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect()
}

fn doctor_repair_actions(text: &str) -> Vec<DoctorRepairAction> {
    let lower = text.to_ascii_lowercase();
    let mut actions = Vec::new();

    if lower.contains("config missing") || lower.contains("config parse/load failed") {
        push_doctor_action(
            &mut actions,
            "orchestrator setup",
            "create providers, models, roles, and local paths",
        );
    }
    if lower.contains("api key missing")
        || lower.contains("api key env")
        || lower.contains("provider `")
        || lower.contains("provider auth")
        || lower.contains("401")
        || lower.contains("403")
    {
        push_doctor_action(
            &mut actions,
            "/provider",
            "configure provider auth or endpoint",
        );
    }
    if lower.contains("missing role config")
        || lower.contains("no worker role")
        || lower.contains("model `")
        || lower.contains("runtime `")
        || lower.contains("model not found")
        || lower.contains("unknown model")
        || lower.contains("invalid model")
        || lower.contains("1211")
        || text.contains("模型不存在")
    {
        push_doctor_action(&mut actions, "/role", "fix role model/runtime binding");
    }
    if lower.contains("not found on path")
        || lower.contains("claude: not found")
        || lower.contains("codex: not found")
    {
        push_doctor_action(
            &mut actions,
            "install runtime",
            "install claude/codex or change the role runtime",
        );
    }
    if actions.is_empty() && lower.contains("all required checks passed") {
        push_doctor_action(&mut actions, "tiffany-loop", "start an orchestration run");
    }

    actions
}

fn push_doctor_action(
    actions: &mut Vec<DoctorRepairAction>,
    command: &'static str,
    detail: &'static str,
) {
    if actions.iter().any(|action| action.command == command) {
        return;
    }
    actions.push(DoctorRepairAction { command, detail });
}

fn doctor_line_is_dim(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("hint:")
        || trimmed.starts_with("Next steps:")
        || trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn concise_success_lines(
    label: &'static str,
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    match label {
        "roles" => concise_roles_success(command_args, output),
        "provider" => concise_provider_success(command_args, output),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RoleSummary {
    name: String,
    model: String,
    display_model: Option<String>,
    provider: Option<String>,
    api_model: Option<String>,
    runtime: String,
    teams: bool,
    health: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderSummary {
    name: String,
    kind: String,
    auth: String,
    endpoint: String,
    models: Option<String>,
    roles: Option<String>,
}

fn concise_roles_success(
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    if command_args.first().map(String::as_str) != Some("roles") {
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
            let teams = if command_args.iter().any(|arg| arg == "--agent-teams") {
                "on"
            } else {
                "off"
            };
            Some(vec![
                status_line(
                    "✓",
                    TIFFANY_BLUE,
                    "role",
                    &format!("saved {role} -> {model}"),
                ),
                body_line(&format!("runtime: {runtime}"), true),
                body_line(&format!("provider: {provider}"), true),
                body_line(&format!("agent teams: {teams}"), true),
                next_line("/doctor", "verify provider/model/runtime wiring"),
            ])
        }
        Some("list") => role_summary_lines(&String::from_utf8_lossy(&output.stdout), "roles"),
        Some("show") => role_summary_lines(&String::from_utf8_lossy(&output.stdout), "role"),
        _ => None,
    }
}

fn concise_provider_success(
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    match (
        command_args.first().map(String::as_str),
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
            if let Some(kind) = flag_value(rest, "--type") {
                lines.push(body_line(&format!("type: {kind}"), true));
            }
            if let Some(env) = flag_value(rest, "--env") {
                lines.push(body_line(&format!("auth: ${env}"), true));
            } else if flag_value(rest, "--key").is_some() {
                lines.push(body_line("auth: literal key", true));
            }
            if let Some(endpoint) = flag_value(rest, "--endpoint") {
                lines.push(body_line(&format!("endpoint: {endpoint}"), true));
            }
            lines.push(next_line("/role", "bind planner/critic/worker to models"));
            lines.push(next_line("/doctor", "verify the full chain"));
            Some(lines)
        }
        (Some("config"), Some("provider"), Some("delete")) => {
            let provider = command_args
                .get(3)
                .map(String::as_str)
                .unwrap_or("<provider>");
            Some(vec![
                status_line(
                    "✓",
                    TIFFANY_BLUE,
                    "provider",
                    &format!("deleted {provider}"),
                ),
                next_line("/doctor", "check roles and models for broken bindings"),
            ])
        }
        (Some("config"), Some("set-endpoint"), _) => {
            let provider = command_args
                .get(2)
                .map(String::as_str)
                .unwrap_or("<provider>");
            let url = command_args.get(3).map(String::as_str).unwrap_or("<url>");
            Some(vec![
                status_line(
                    "✓",
                    TIFFANY_BLUE,
                    "provider",
                    &format!("{provider} endpoint -> {url}"),
                ),
                next_line("/doctor", "verify provider connectivity"),
            ])
        }
        (Some("config"), Some("set-key"), _) => {
            let provider = command_args
                .get(2)
                .map(String::as_str)
                .unwrap_or("<provider>");
            Some(vec![
                status_line(
                    "✓",
                    TIFFANY_BLUE,
                    "provider",
                    &format!("{provider} auth updated"),
                ),
                next_line("/doctor", "verify provider auth"),
            ])
        }
        (Some("config"), Some("provider"), Some("list")) | (Some("config"), Some("show"), _) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            provider_summary_lines(&stdout)
        }
        _ => None,
    }
}

fn provider_summary_lines(text: &str) -> Option<Vec<Line<'static>>> {
    let providers = parse_provider_summaries(text);
    if providers.is_empty() {
        return None;
    }

    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "provider",
        &format!("{} configured", providers.len()),
    )];
    for provider in providers.iter().take(8) {
        lines.push(provider_summary_line(provider));
    }
    if providers.len() > 8 {
        lines.push(body_line(&format!("… {} more", providers.len() - 8), true));
    }
    lines.push(next_line("/provider edit <name>", "fix auth or endpoint"));
    lines.push(next_line("/role", "bind roles to provider models"));
    lines.push(next_line("/doctor", "verify provider/model/runtime wiring"));
    Some(lines)
}

fn role_summary_lines(text: &str, label: &'static str) -> Option<Vec<Line<'static>>> {
    let roles = parse_role_summaries(text);
    if roles.is_empty() {
        return None;
    }

    let message = if roles.len() == 1 {
        format!("{} configured", roles[0].name)
    } else {
        format!("{} registered", roles.len())
    };
    let mut lines = vec![status_line("✓", TIFFANY_BLUE, label, &message)];
    for role in roles.iter().take(10) {
        lines.push(role_summary_line(role));
    }
    if roles.len() > 10 {
        lines.push(body_line(&format!("… {} more", roles.len() - 10), true));
    }
    lines.push(next_line("/role <name>", "edit one role binding"));
    lines.push(next_line("/doctor", "verify the orchestration chain"));
    Some(lines)
}

fn provider_summary_line(provider: &ProviderSummary) -> Line<'static> {
    let (symbol, color, auth, health) = provider_auth_status(provider);
    let endpoint = if provider.endpoint.trim().is_empty()
        || provider.endpoint == "-"
        || provider.endpoint == "—"
    {
        "default".to_string()
    } else {
        truncate_text(&provider.endpoint, 42)
    };
    Line::from(
        vec![
            Span::styled(
                format!("  {symbol} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<13}", provider.name),
                Style::default().fg(TIFFANY_SOFT),
            ),
            Span::styled(
                format!("{:<11}", provider.kind),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(format!("{auth:<10}"), Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:<44}", endpoint),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(health, Style::default().fg(color)),
        ]
        .into_iter()
        .chain(provider_assoc_spans(provider))
        .collect::<Vec<_>>(),
    )
}

fn role_summary_line(role: &RoleSummary) -> Line<'static> {
    let target = role_target_label(role);
    let teams = if role.teams { "teams on" } else { "teams off" };
    let (symbol, color, health) = role_health_status(role);
    Line::from(vec![
        Span::styled(
            format!("  {symbol} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<14}", role.name),
            Style::default().fg(TIFFANY_SOFT),
        ),
        Span::styled(
            format!("{:<18}", truncate_text(&role.model, 18)),
            Style::default(),
        ),
        Span::styled(
            format!("{:<13}", role.runtime),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("{:<32}", truncate_text(&target, 32)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(format!("{teams:<11}"), Style::default().fg(Color::DarkGray)),
        Span::styled(truncate_text(&health, 34), Style::default().fg(color)),
    ])
}

fn role_target_label(role: &RoleSummary) -> String {
    match (
        role.provider.as_deref().filter(|value| !value.is_empty()),
        role.api_model.as_deref().filter(|value| !value.is_empty()),
    ) {
        (Some(provider), Some(api_model)) if api_model != "-" => format!("{provider}/{api_model}"),
        (Some(provider), _) if provider != "-" => provider.to_string(),
        (_, Some(api_model)) if api_model != "-" => api_model.to_string(),
        _ => role
            .display_model
            .as_deref()
            .unwrap_or(&role.model)
            .to_string(),
    }
}

fn role_health_status(role: &RoleSummary) -> (&'static str, Color, String) {
    match role.health.as_deref().map(str::trim) {
        Some("ready") | None | Some("") => ("✓", TIFFANY_BLUE, "ready".to_string()),
        Some(health) => ("⚠", Color::Yellow, health.to_string()),
    }
}

fn provider_auth_status(
    provider: &ProviderSummary,
) -> (&'static str, Color, &'static str, &'static str) {
    let auth = provider.auth.trim();
    if provider.kind.eq_ignore_ascii_case("ollama") {
        return ("●", TIFFANY_BLUE, "local", "ready");
    }
    if auth.is_empty() || auth == "-" || auth == "—" || auth.eq_ignore_ascii_case("none") {
        return ("⚠", Color::Yellow, "no key", "needs auth");
    }
    ("✓", TIFFANY_BLUE, "auth set", "ready")
}

fn provider_assoc_spans(provider: &ProviderSummary) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(models) = provider
        .models
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "-")
    {
        spans.push(Span::styled(
            format!("  models {}", truncate_text(models, 22)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(roles) = provider
        .roles
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "-")
    {
        spans.push(Span::styled(
            format!("  roles {}", truncate_text(roles, 22)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans
}

fn parse_provider_summaries(text: &str) -> Vec<ProviderSummary> {
    let mut providers = Vec::new();
    let mut in_config_providers = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("Providers (") && trimmed.ends_with(':') {
            in_config_providers = true;
            continue;
        }
        if in_config_providers {
            if trimmed.starts_with("Models (")
                || trimmed.starts_with("Roles (")
                || trimmed.starts_with("Tag overrides")
                || trimmed.starts_with("───")
            {
                in_config_providers = false;
                continue;
            }
            if let Some(provider) = parse_config_show_provider(trimmed) {
                providers.push(provider);
            }
            continue;
        }
        if let Some(provider) = parse_provider_table_row(trimmed) {
            providers.push(provider);
        }
    }

    providers
}

fn parse_provider_table_row(line: &str) -> Option<ProviderSummary> {
    if line.starts_with("Providers ")
        || line.starts_with("provider ")
        || line.starts_with("===")
        || line.starts_with("config file:")
    {
        return None;
    }
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    if parts[0] == "-" {
        return None;
    }
    Some(ProviderSummary {
        name: parts[0].to_string(),
        kind: parts[1].to_string(),
        auth: parts[2].to_string(),
        endpoint: parts[3].to_string(),
        models: parts.get(4).map(|value| (*value).to_string()),
        roles: parts.get(5).map(|value| (*value).to_string()),
    })
}

fn parse_config_show_provider(line: &str) -> Option<ProviderSummary> {
    let line = line.strip_prefix("- ")?;
    let parts = line.split_whitespace().collect::<Vec<_>>();
    let name = parts.first()?.to_string();
    let kind = parts
        .iter()
        .find_map(|part| part.strip_prefix("type="))
        .unwrap_or("unknown")
        .to_string();
    let auth = parts
        .iter()
        .position(|part| part.starts_with("api_key="))
        .map(|idx| {
            let first = parts[idx].trim_start_matches("api_key=");
            if first == "✓" && parts.get(idx + 1) == Some(&"set") {
                "set".to_string()
            } else {
                first.to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string());
    let endpoint = parts
        .iter()
        .find_map(|part| {
            part.strip_prefix("base_url=")
                .or_else(|| part.strip_prefix("endpoint="))
        })
        .unwrap_or("-")
        .to_string();
    Some(ProviderSummary {
        name,
        kind,
        auth,
        endpoint,
        models: parts
            .iter()
            .find_map(|part| part.strip_prefix("models="))
            .map(ToString::to_string),
        roles: parts
            .iter()
            .find_map(|part| part.strip_prefix("roles="))
            .map(ToString::to_string),
    })
}

fn parse_role_summaries(text: &str) -> Vec<RoleSummary> {
    text.lines()
        .filter_map(parse_role_summary_line)
        .collect::<Vec<_>>()
}

fn parse_role_summary_line(line: &str) -> Option<RoleSummary> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("Registered roles")
        || trimmed.starts_with("Register:")
        || trimmed.starts_with("Roles (")
    {
        return None;
    }
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let normalized = trimmed.replace('→', " ");
    let parts = normalized.split_whitespace().collect::<Vec<_>>();
    let name = parts.first()?.to_string();
    let model = parts
        .iter()
        .find_map(|part| part.strip_prefix("model="))?
        .to_string();
    let display_model = parts
        .iter()
        .find_map(|part| {
            part.strip_prefix('(')
                .and_then(|value| value.strip_suffix(')'))
        })
        .map(ToString::to_string);
    let runtime = parts
        .iter()
        .find_map(|part| part.strip_prefix("runtime="))
        .unwrap_or("runtime")
        .to_string();
    let teams = parts
        .iter()
        .any(|part| *part == "[agent_teams]" || part.strip_prefix("teams=") == Some("true"));

    Some(RoleSummary {
        name,
        model,
        display_model,
        provider: parts
            .iter()
            .find_map(|part| part.strip_prefix("provider="))
            .map(ToString::to_string),
        api_model: parts
            .iter()
            .find_map(|part| {
                part.strip_prefix("api_model=")
                    .or_else(|| part.strip_prefix("model_name="))
            })
            .map(ToString::to_string),
        runtime,
        teams,
        health: parts
            .iter()
            .find_map(|part| part.strip_prefix("health="))
            .map(ToString::to_string),
    })
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
    worker_context: &[String],
) -> Vec<Line<'static>> {
    let mut lines = vec![status_line(
        "✗",
        Color::Red,
        "orchestrator",
        &format!("exited with status {status}"),
    )];

    if !worker_context.is_empty() {
        lines.push(body_line("worker context:", true));
        for line in worker_context.iter().take(6) {
            lines.push(body_line(line, true));
        }
    }

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
    fn emit_event(&mut self, app_event_tx: &AppEventSender, mut event: TiffanyProgressEvent) {
        self.remember_worker_metadata(&event);
        self.apply_worker_metadata(&mut event);

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
            if let Some(result) = final_output_from_event(&event, content) {
                remember_better_text(&mut self.final_output, result);
            }
            let key = normalized_output_key(content);
            let scope = output_scope(&event);
            if !self.seen_outputs.insert(format!("{scope}:{key}")) {
                return;
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

    fn remember_worker_metadata(&mut self, event: &TiffanyProgressEvent) {
        if event.role != "worker" {
            return;
        }
        let Some(task_id) = event
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };

        let meta = self.worker_metadata.entry(task_id.to_string()).or_default();
        fill_present(&mut meta.agent, event.agent.as_deref());
        fill_present(&mut meta.worker_role, event.worker_role.as_deref());
        fill_present(&mut meta.runtime, event.runtime.as_deref());
        fill_present(&mut meta.model, event.model.as_deref());
        fill_present(&mut meta.provider, event.provider.as_deref());
    }

    fn apply_worker_metadata(&self, event: &mut TiffanyProgressEvent) {
        if event.role != "worker" {
            return;
        }
        let Some(task_id) = event.task_id.as_deref() else {
            return;
        };
        let Some(meta) = self.worker_metadata.get(task_id) else {
            return;
        };

        fill_missing(&mut event.agent, meta.agent.as_deref());
        fill_missing(&mut event.worker_role, meta.worker_role.as_deref());
        fill_missing(&mut event.runtime, meta.runtime.as_deref());
        fill_missing(&mut event.model, meta.model.as_deref());
        fill_missing(&mut event.provider, meta.provider.as_deref());
    }

    fn worker_context_lines(&self) -> Vec<String> {
        let mut entries = self.worker_metadata.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(task_id, _)| task_id.as_str());
        entries
            .into_iter()
            .map(|(task_id, meta)| worker_context_line(task_id, meta))
            .collect()
    }
}

fn worker_context_line(task_id: &str, meta: &WorkerMeta) -> String {
    let mut parts = Vec::new();
    if let Some(role) = meta.worker_role.as_deref().and_then(nonempty_trimmed) {
        parts.push(role.to_string());
    }
    if let Some(agent) = meta.agent.as_deref().and_then(nonempty_trimmed)
        && !parts.iter().any(|part| part == agent)
    {
        parts.push(agent.to_string());
    }
    if let Some(runtime) = meta.runtime.as_deref().and_then(nonempty_trimmed)
        && !parts.iter().any(|part| part == runtime)
    {
        parts.push(format!("runtime {runtime}"));
    }
    if let Some(provider_model) = worker_meta_provider_model_label(meta) {
        parts.push(provider_model);
    }
    if let Some(id) = short_task_id(Some(task_id)) {
        parts.push(id.to_string());
    }
    if parts.is_empty() {
        parts.push(short_task_id(Some(task_id)).unwrap_or(task_id).to_string());
    }
    format!("worker: {}", parts.join(" · "))
}

fn worker_meta_provider_model_label(meta: &WorkerMeta) -> Option<String> {
    let model = meta.model.as_deref().and_then(nonempty_trimmed)?;
    Some(match meta.provider.as_deref().and_then(nonempty_trimmed) {
        Some(provider) => format!("{provider}/{model}"),
        None => model.to_string(),
    })
}

fn nonempty_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn fill_present(slot: &mut Option<String>, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    *slot = Some(value.to_string());
}

fn fill_missing(slot: &mut Option<String>, value: Option<&str>) {
    if slot
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return;
    }
    fill_present(slot, value);
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

    let mut lines = vec![waterfall_status_line(event)];
    lines.extend(event_detail_lines(event));

    emit_lines(app_event_tx, lines);
}

fn output_event_lines(event: &TiffanyProgressEvent, content: &str) -> Vec<Line<'static>> {
    if event.role == "worker" {
        return worker_output_event_lines(event, content);
    }

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

fn worker_output_event_lines(event: &TiffanyProgressEvent, content: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "│",
            Style::default()
                .fg(TIFFANY_DARK)
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

fn waterfall_status_line(event: &TiffanyProgressEvent) -> Line<'static> {
    if event.role == "worker" && event.task_id.is_some() {
        return worker_status_line(event);
    }
    if event.role == "reviewer" && event.task_id.is_some() {
        return reviewer_status_line(event);
    }

    let color = status_color(event);
    Line::from(vec![
        Span::styled(
            status_symbol(event),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "step",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:<11}", stage_label(event)),
            Style::default().fg(TIFFANY_SOFT),
        ),
        Span::raw(" "),
        Span::styled(event_title(event), Style::default()),
    ])
}

fn reviewer_status_line(event: &TiffanyProgressEvent) -> Line<'static> {
    let color = status_color(event);
    Line::from(vec![
        Span::styled(
            status_symbol(event),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "review",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(reviewer_lifecycle_title(event), Style::default()),
    ])
}

fn worker_status_line(event: &TiffanyProgressEvent) -> Line<'static> {
    let color = status_color(event);
    Line::from(vec![
        Span::styled(
            status_symbol(event),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "worker",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(worker_lifecycle_title(event), Style::default()),
    ])
}

fn stage_label(event: &TiffanyProgressEvent) -> &'static str {
    match event.role.as_str() {
        "planner" => "01 planner",
        "critic" => "02 critic",
        "worker" => "03 worker",
        "reviewer" => "04 reviewer",
        "orchestrator" => "05 result",
        _ => "00 event",
    }
}

fn event_title(event: &TiffanyProgressEvent) -> String {
    if event.role == "worker"
        && matches!(event.status.as_str(), "running" | "done" | "failed")
        && event.task_id.is_some()
    {
        return worker_lifecycle_title(event);
    }
    if event.role == "reviewer" && event.task_id.is_some() {
        return reviewer_lifecycle_title(event);
    }

    let mut title = normalize_event_message(&event.message);
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
    let suffix = match event.role.as_str() {
        "planner" => "plan",
        "critic" => "critique",
        "reviewer" => "review",
        "worker" => "output",
        _ => "details",
    };
    if event.role == "worker" {
        let label = output_label(event);
        format!("worker {suffix} · {label}")
    } else {
        format!("{} · {suffix}", stage_label(event))
    }
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
    let mut parts = Vec::new();
    match (role, agent) {
        (Some(role), Some(agent)) if event.role == "worker" && role != agent => {
            parts.push(role.to_string());
            parts.push(agent.to_string());
        }
        (Some(role), _) => parts.push(role.to_string()),
        (None, Some(agent)) => parts.push(agent.to_string()),
        (None, None) => parts.push(event.role.clone()),
    }
    if let Some(model) = provider_model_label(event) {
        parts.push(model);
    }
    if let Some(id) = short_task_id(event.task_id.as_deref()) {
        parts.push(id.to_string());
    }
    parts.join(" · ")
}

fn event_detail_lines(event: &TiffanyProgressEvent) -> Vec<Line<'static>> {
    if event.role != "worker" || event.status != "running" || event.task_id.is_none() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if let Some(prompt) = event
        .task_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(meta_line("task", &truncate_text(prompt, 180)));
    }
    lines
}

fn reviewer_lifecycle_title(event: &TiffanyProgressEvent) -> String {
    let action = match event.status.as_str() {
        "running" => "checking worker output".to_string(),
        "done" if event.approved == Some(true) => "passed".to_string(),
        "warning" if event.approved == Some(false) => "needs fixes".to_string(),
        _ => normalize_event_message(&event.message),
    };
    let mut parts = vec![action];
    if let Some(id) = short_task_id(event.task_id.as_deref()) {
        parts.push(id.to_string());
    }
    if matches!(event.status.as_str(), "warning" | "failed")
        && let Some(issues) = event.issues
    {
        parts.push(format!("{issues} issue(s)"));
    }
    parts.join(" · ")
}

fn worker_lifecycle_title(event: &TiffanyProgressEvent) -> String {
    let role = worker_role_label(event);
    let action = match event.status.as_str() {
        "running" => "started",
        "done" => "done",
        "failed" => "failed",
        _ => event.status.as_str(),
    };
    let runtime = event
        .agent
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let mut parts = vec![format!("{role} {action}")];
    if let Some(runtime) = runtime.filter(|runtime| *runtime != role) {
        parts.push(runtime.to_string());
    }
    if let Some(model) = provider_model_label(event) {
        parts.push(model);
    }
    if let Some(id) = short_task_id(event.task_id.as_deref()) {
        parts.push(id.to_string());
    }
    if let Some(duration) = event.duration_ms.map(format_duration_ms) {
        parts.push(duration);
    }
    parts.join(" · ")
}

fn worker_role_label(event: &TiffanyProgressEvent) -> String {
    event
        .worker_role
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| event.agent.as_deref().map(str::trim))
        .filter(|value| !value.is_empty())
        .unwrap_or("worker")
        .to_string()
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

fn normalize_event_message(message: &str) -> String {
    message.replace(" - ", " · ")
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
    let display = format_tiffany_summary_style(&event.role, &display);
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

fn final_output_from_event(event: &TiffanyProgressEvent, visible: &str) -> Option<String> {
    if event.role != "worker" || event.status != "output" {
        return None;
    }
    if let Some(result) = event.content.as_deref().and_then(final_output_candidate) {
        return Some(result);
    }
    let raw = event.content.as_deref()?;
    if worker_output_kind(raw).is_some_and(|kind| kind == "assistant") {
        let cleaned = strip_final_heading(visible);
        if !cleaned.trim().is_empty() {
            return Some(cleaned);
        }
    }
    None
}

fn worker_output_kind(content: &str) -> Option<&str> {
    let prefix = content.trim_start().split_once(": ")?.0;
    let mut parts = prefix.split_whitespace();
    let runtime = parts.next()?.to_ascii_lowercase();
    let kind = parts.next()?.trim();
    if matches!(runtime.as_str(), "claude" | "claude-code" | "codex") {
        Some(kind)
    } else {
        None
    }
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
    let lines = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let lower = lines
        .first()
        .copied()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match role {
        "planner" => lower.starts_with("plan ready") && lines.len() <= 1,
        "critic" | "reviewer" => {
            lower.starts_with("approved")
                || (lower.starts_with("needs changes")
                    && !role_output_has_actionable_detail(&lines))
        }
        _ => false,
    }
}

fn role_output_has_actionable_detail(lines: &[&str]) -> bool {
    lines.iter().skip(1).any(|line| {
        let lower = line.trim().to_ascii_lowercase();
        !matches!(
            lower.as_str(),
            "issue:" | "issues:" | "suggestion:" | "suggestions:"
        )
    })
}

fn format_tiffany_summary_style(role: &str, content: &str) -> String {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let first = restyle_summary_first_line(first);
    let rest = lines.collect::<Vec<_>>();
    if role == "planner" {
        return compact_plan_summary(&first, &rest);
    }
    if rest.is_empty() {
        first
    } else {
        format!("{}\n{}", first, rest.join("\n"))
    }
}

#[derive(Debug)]
struct CompactPlanTask {
    number: String,
    prompt: String,
    agent: Option<String>,
}

fn compact_plan_summary(first: &str, rest: &[&str]) -> String {
    if !first
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("plan ready")
    {
        return if rest.is_empty() {
            first.to_string()
        } else {
            format!("{}\n{}", first, rest.join("\n"))
        };
    }

    let mut tasks: Vec<CompactPlanTask> = Vec::new();
    let mut more_line: Option<String> = None;
    let mut passthrough: Vec<String> = Vec::new();
    for line in rest {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((number, prompt)) = parse_plan_task_line(trimmed) {
            tasks.push(CompactPlanTask {
                number,
                prompt,
                agent: None,
            });
            continue;
        }
        if let Some(agent) = trimmed.strip_prefix("agent: ") {
            if let Some(task) = tasks.last_mut() {
                task.agent = Some(agent.trim().to_string());
            } else {
                passthrough.push(trimmed.to_string());
            }
            continue;
        }
        if trimmed.starts_with('…') && trimmed.contains("more") {
            more_line = Some(trimmed.to_string());
            continue;
        }
        passthrough.push(trimmed.to_string());
    }

    if tasks.is_empty() {
        return if rest.is_empty() {
            first.to_string()
        } else {
            format!("{}\n{}", first, rest.join("\n"))
        };
    }

    let mut out = vec![first.to_string()];
    let visible_count = tasks.len().min(3);
    for task in tasks.iter().take(visible_count) {
        let mut line = format!("  {}. {}", task.number, truncate_text(&task.prompt, 96));
        if let Some(agent) = task.agent.as_deref().filter(|agent| !agent.is_empty()) {
            line.push_str(" · ");
            line.push_str(&truncate_text(agent, 48));
        }
        out.push(line);
    }

    let hidden_count = tasks.len().saturating_sub(visible_count);
    if hidden_count > 0 {
        out.push(format!("  … {hidden_count} more"));
    } else if let Some(more_line) = more_line {
        out.push(format!("  {more_line}"));
    }
    out.extend(
        passthrough
            .into_iter()
            .take(2)
            .map(|line| format!("  {line}")),
    );
    out.join("\n")
}

fn parse_plan_task_line(line: &str) -> Option<(String, String)> {
    let (number, prompt) = line.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some((number.to_string(), prompt.trim().to_string()))
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

fn meta_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            format!("{label:<6}"),
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::DarkGray)),
    ])
}

fn next_line(command: &str, detail: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "  next",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            command.to_string(),
            Style::default()
                .fg(TIFFANY_SOFT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(detail.to_string(), Style::default().fg(Color::DarkGray)),
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
    fn doctor_command_args_accepts_read_only_diagnostics() {
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
    fn doctor_output_dims_hints_but_not_issue_lines() {
        assert!(doctor_line_is_dim("  hint: configure provider auth"));
        assert!(doctor_line_is_dim("Next steps:"));
        assert!(doctor_line_is_dim("1. Run `orchestrator setup`"));
        assert!(!doctor_line_is_dim("✗ openai: api key missing"));
        assert!(!doctor_line_is_dim("⚠ 3 issue(s) found"));
        assert!(!doctor_line_is_dim("✓ config parsed"));
    }

    #[test]
    fn doctor_repair_lines_summarize_actionable_fixes() {
        let lines = doctor_repair_lines(
            "✗ google: api key missing\n\
             ✗ planner: missing role config\n\
             ✗ codex: `codex` not found on PATH\n\
             ✗ worker-codex stderr: [1211] 模型不存在",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("repair  /provider  configure provider auth or endpoint"));
        assert!(text.contains("repair  /role  fix role model/runtime binding"));
        assert!(text.contains("repair  install runtime  install claude/codex"));
        assert_eq!(text.matches("/role").count(), 1);
    }

    #[test]
    fn doctor_repair_lines_suggest_launch_when_ready() {
        let lines = doctor_repair_lines("✓ all required checks passed");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("repair  tiffany-loop  start an orchestration run"));
    }

    #[test]
    fn provider_summary_lines_render_status_rows_and_next_steps() {
        let lines = provider_summary_lines(
            "Providers (/Users/me/.orchestrator/config.yaml)\n\
               provider     type       api_key                  endpoint                      models                 roles\n\
               anthropic    anthropic  sk-c...aslE              https://api.minimaxi.com/anthropic sonnet,opus           planner,reviewer\n\
               google       google     -                        -                             gemini                critic\n\
               ollama       ollama     -                        http://localhost:11434        local                 worker-local\n",
        )
        .expect("provider summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ provider  3 configured"));
        assert!(text.contains("anthropic"));
        assert!(text.contains("auth set"));
        assert!(text.contains("ready"));
        assert!(text.contains("⚠ google"));
        assert!(text.contains("no key"));
        assert!(text.contains("needs auth"));
        assert!(text.contains("● ollama"));
        assert!(text.contains("local"));
        assert!(text.contains("models sonnet,opus"));
        assert!(text.contains("roles planner,reviewer"));
        assert!(text.contains("next  /provider edit <name>  fix auth or endpoint"));
        assert!(text.contains("next  /role  bind roles to provider models"));
        assert!(text.contains("next  /doctor  verify provider/model/runtime wiring"));
        assert!(!text.contains("provider     type"));
    }

    #[test]
    fn provider_summary_lines_parse_config_show_provider_section() {
        let lines = provider_summary_lines(
            "=== Orchestrator config ===\n\
             Providers (2):\n\
               - anthropic  type=anthropic  api_key=✓ set base_url=https://api.anthropic.com models=sonnet roles=planner\n\
               - google     type=google     api_key=— base_url=— models=gemini roles=critic\n\
             Models (1):\n\
               - sonnet claude-sonnet (provider: anthropic)\n",
        )
        .expect("provider summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ provider  2 configured"));
        assert!(text.contains("anthropic"));
        assert!(text.contains("auth set"));
        assert!(text.contains("https://api.anthropic.com"));
        assert!(text.contains("models sonnet"));
        assert!(text.contains("roles planner"));
        assert!(text.contains("⚠ google"));
        assert!(text.contains("no key"));
        assert!(!text.contains("Models (1)"));
    }

    #[test]
    fn role_summary_lines_render_registered_roles() {
        let lines = role_summary_lines(
            "Registered roles:\n\
               critic         model=minimax-m3-claude provider=minimax api_model=MiniMax-M3 runtime=claude-code teams=false health=ready\n\
               worker-cc      model=minimax-m3-claude provider=minimax api_model=MiniMax-M3 runtime=claude-code teams=true health=ready\n\
               worker-codex   model=minimax-m3-codex provider=minimax api_model=MiniMax-M3 runtime=codex teams=false health=runtime-missing:codex\n\
             \n\
             Register: orchestrator roles register <role> --model <model-id> --runtime <runtime-id>\n",
            "roles",
        )
        .expect("role summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ roles  3 registered"));
        assert!(text.contains("critic"));
        assert!(text.contains("MiniMax-M3"));
        assert!(text.contains("minimax/MiniMax-M3"));
        assert!(text.contains("worker-cc"));
        assert!(text.contains("teams on"));
        assert!(text.contains("worker-codex"));
        assert!(text.contains("codex"));
        assert!(text.contains("runtime-missing:codex"));
        assert!(text.contains("next  /doctor  verify the orchestration chain"));
        assert!(!text.contains("Registered roles:"));
        assert!(!text.contains("Register: orchestrator"));
    }

    #[test]
    fn role_summary_lines_render_single_role() {
        let lines = role_summary_lines(
            "worker-codex   model=minimax-m3-codex (MiniMax-M3) runtime=codex        teams=false\n",
            "role",
        )
        .expect("role summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ role  worker-codex configured"));
        assert!(text.contains("worker-codex"));
        assert!(text.contains("MiniMax-M3"));
        assert!(text.contains("teams off"));
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

        assert_eq!(
            line_text(&lines[0]),
            "│ worker output · claude-code · 12345678"
        );
        assert_eq!(line_text(&lines[1]), "  │ done");
        assert_eq!(lines[0].spans[0].style.fg, Some(TIFFANY_DARK));
        assert_eq!(lines[0].spans[2].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[1].spans[0].style.fg, Some(TIFFANY_DARK));
    }

    #[test]
    fn status_lines_render_ordered_waterfall_steps() {
        let planner = TiffanyProgressEvent {
            role: "planner".to_string(),
            status: "running".to_string(),
            message: "planning".to_string(),
            task_id: None,
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let worker = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let reviewer = TiffanyProgressEvent {
            role: "reviewer".to_string(),
            status: "warning".to_string(),
            message: "review needs fixes - 2 issue(s)".to_string(),
            task_id: Some("87654321-0000-0000-0000-000000000000".to_string()),
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: None,
            approved: Some(false),
            issues: Some(2),
            count: None,
            duration_ms: None,
        };

        assert_eq!(
            line_text(&waterfall_status_line(&planner)),
            "● step  01 planner  planning"
        );
        assert_eq!(
            line_text(&waterfall_status_line(&worker)),
            "● worker  worker-cc started · claude-code · minimax/MiniMax-M3 · 12345678"
        );
        assert_eq!(
            line_text(&waterfall_status_line(&reviewer)),
            "⚠ review  needs fixes · 87654321 · 2 issue(s)"
        );
    }

    #[test]
    fn reviewer_status_lines_track_worker_review_lifecycle() {
        let checking = TiffanyProgressEvent {
            role: "reviewer".to_string(),
            status: "running".to_string(),
            message: "reviewing worker output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: None,
            worker_role: None,
            runtime: None,
            model: None,
            provider: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        assert_eq!(
            line_text(&waterfall_status_line(&checking)),
            "● review  checking worker output · 12345678"
        );

        let passed = TiffanyProgressEvent {
            status: "done".to_string(),
            message: "review approved".to_string(),
            approved: Some(true),
            issues: Some(0),
            ..checking.clone()
        };
        assert_eq!(
            line_text(&waterfall_status_line(&passed)),
            "✓ review  passed · 12345678"
        );

        let needs_fixes = TiffanyProgressEvent {
            status: "warning".to_string(),
            message: "review needs fixes - 3 issue(s)".to_string(),
            approved: Some(false),
            issues: Some(3),
            ..checking
        };
        assert_eq!(
            line_text(&waterfall_status_line(&needs_fixes)),
            "⚠ review  needs fixes · 12345678 · 3 issue(s)"
        );
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

        assert!(text.contains("↳ 02 critic · critique"));
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
            "│ worker output · worker-cc · claude-code · 12345678"
        );
    }

    #[test]
    fn worker_metadata_carries_model_into_output_and_done_titles() {
        let mut state = BridgeState::default();
        let started = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            task_prompt: Some("do the work".to_string()),
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        state.remember_worker_metadata(&started);

        let mut output = TiffanyProgressEvent {
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
            content: Some("done".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        state.apply_worker_metadata(&mut output);
        assert_eq!(
            output_title(&output),
            "worker output · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
        );

        let mut done = TiffanyProgressEvent {
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
        state.apply_worker_metadata(&mut done);
        assert_eq!(
            event_title(&done),
            "worker-cc done · claude-code · minimax/MiniMax-M3 · 12345678 · 1.2s"
        );
        assert_eq!(
            state.worker_context_lines(),
            vec!["worker: worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"]
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

        let mut lines = vec![waterfall_status_line(&event)];
        lines.extend(event_detail_lines(&event));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker"));
        assert!(text.contains("worker-cc started"));
        assert!(text.contains("claude-code · minimax/MiniMax-M3 · 12345678"));
        assert!(!text.contains("route "));
        assert!(text.contains("task  回答用户的问题"));
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
        let visible = visible_content(&event).expect("visible approval summary");
        assert!(is_redundant_role_output(&event.role, &visible));
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
        assert!(!is_redundant_role_output(&event.role, &visible));
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
        assert_eq!(
            output_title(&event),
            "worker output · claude-code · 12345678"
        );
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
    fn captures_final_result_heading_for_followup_memory() {
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
            content: Some("Final result\n你好！\n我可以帮你写代码。".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let visible = visible_content(&event).expect("visible final result");

        assert_eq!(
            final_output_from_event(&event, &visible).as_deref(),
            Some("你好！\n我可以帮你写代码。")
        );
    }

    #[test]
    fn captures_assistant_output_but_not_worker_process_noise_for_memory() {
        let assistant = TiffanyProgressEvent {
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
            content: Some("claude-code assistant: 你好！".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };
        let assistant_visible = visible_content(&assistant).expect("assistant visible");
        assert_eq!(
            final_output_from_event(&assistant, &assistant_visible).as_deref(),
            Some("你好！")
        );

        let stderr = TiffanyProgressEvent {
            content: Some("claude-code stderr: model not found".to_string()),
            ..assistant.clone()
        };
        let stderr_visible = visible_content(&stderr).expect("stderr visible");
        assert_eq!(final_output_from_event(&stderr, &stderr_visible), None);

        let raw_status = TiffanyProgressEvent {
            content: Some("running tests".to_string()),
            ..assistant
        };
        let raw_visible = visible_content(&raw_status).expect("status visible");
        assert_eq!(final_output_from_event(&raw_status, &raw_visible), None);
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
        assert!(is_redundant_role_output(
            "reviewer",
            "approved - 0 issue(s)\n  suggestions:\n  - tighten wording"
        ));
        assert!(is_redundant_role_output(
            "reviewer",
            "needs changes (2 issue(s))"
        ));
        assert!(is_redundant_role_output(
            "critic",
            "needs changes - 1 issue(s)\n  suggestions:"
        ));
        assert!(!is_redundant_role_output(
            "reviewer",
            "needs changes - 1 issue(s)\n  - missing final answer"
        ));
        assert!(!is_redundant_role_output(
            "critic",
            "needs changes - 1 issue(s)\n  suggestions:\n  - answer in Chinese"
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
        assert!(!planner_visible.contains("agent:"));
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
    fn planner_summary_compacts_tasks_and_inlines_agents() {
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
                r#"{"sub_tasks":[{"prompt":"Audit the TUI output path and identify noisy controller messages that are repeated in the visible transcript","agent_hint":"worker-cc"},{"prompt":"Update the forked TUI renderer to keep worker output visible while compacting planner details","agent_hint":"worker-codex"},{"prompt":"Add regression tests for compact planner rendering and preserved reviewer issues","agent_hint":"worker-cc"},{"prompt":"Run the smoke check and summarize the result","agent_hint":"worker-cc"}]}"#
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
        };

        let visible = visible_content(&planner).expect("visible compact plan");

        assert!(visible.contains("plan ready - 4 sub-task(s)"));
        assert!(visible.contains("1. Audit the TUI output path"));
        assert!(visible.contains(" · worker-cc"));
        assert!(visible.contains("2. Update the forked TUI renderer"));
        assert!(visible.contains(" · worker-codex"));
        assert!(visible.contains("… 1 more"));
        assert!(!visible.contains("agent:"));
        assert!(!visible.contains("4. Run the smoke check"));
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
            &[],
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
            &[],
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
            &["worker: worker-codex · codex · minimax/MiniMax-M3 · abcdef12".to_string()],
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker context"));
        assert!(text.contains("worker-codex · codex · minimax/MiniMax-M3"));
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
            provider_command_args("edit minimax --env MINIMAX_API_KEY").expect("provider args"),
            vec![strings(&[
                "config",
                "provider",
                "setup",
                "minimax",
                "--env",
                "MINIMAX_API_KEY"
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
