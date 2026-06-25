use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use serde::Deserialize;
use serde::Serialize;
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
pub(crate) const TIFFANY_BLUE: Color = Color::Rgb(10, 186, 181);
#[allow(clippy::disallowed_methods)]
const TIFFANY_DARK: Color = Color::Rgb(7, 94, 91);
#[allow(clippy::disallowed_methods)]
const TIFFANY_SOFT: Color = Color::Rgb(76, 210, 204);
const CONTROL_SUMMARY_MAX_CHARS: usize = 240_000;
const FULL_MESSAGE_STREAM_MAX_CHARS: usize = usize::MAX;
const FAILURE_DETAIL_MAX_LINES: usize = 96;
const FAILURE_DETAIL_HEAD_LINES: usize = 24;
const MEMORY_FILE: &str = "tiffany-orchestrator/memory.json";
const MEMORY_SCHEMA_VERSION: u32 = 1;
const MEMORY_MAX_TURNS: usize = 8;
const NATIVE_SESSIONS_FILE: &str = "tiffany-orchestrator/native-sessions.json";
const NATIVE_SESSIONS_SCHEMA_VERSION: u32 = 1;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TiffanyOrchestratorRuntimeStatus {
    pub(crate) requested: String,
    pub(crate) resolved: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TiffanyStartupReadiness {
    pub(crate) runtime: TiffanyOrchestratorRuntimeStatus,
    config: TiffanyConfigReadiness,
}

#[derive(Debug, Clone)]
enum TiffanyConfigReadiness {
    Missing { path: String },
    Invalid { path: String, message: String },
    Ready(TiffanyConfigSummary),
}

#[derive(Debug, Clone)]
struct TiffanyConfigSummary {
    providers: usize,
    models: usize,
    roles: usize,
    runtimes: usize,
    default_worker: Option<TiffanyDefaultWorkerSummary>,
}

#[derive(Debug, Clone)]
struct TiffanyDefaultWorkerSummary {
    role: String,
    runtime: String,
    binary: String,
    status: TiffanyWorkerReadinessStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TiffanyWorkerReadinessStatus {
    Ready,
    ModelMissing,
    ProviderMissing,
    RuntimeMissing,
}

#[derive(Clone, Debug)]
pub(crate) struct TiffanyOrchestratorTurn {
    pub(crate) user_prompt: String,
    pub(crate) result: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct TiffanyOrchestratorMemory {
    #[serde(default = "memory_schema_version")]
    version: u32,
    #[serde(default)]
    conversations: Vec<TiffanyOrchestratorMemoryConversation>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct TiffanyOrchestratorMemoryConversation {
    cwd: String,
    turns: Vec<TiffanyOrchestratorMemoryTurn>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TiffanyOrchestratorMemoryTurn {
    user_prompt: String,
    result: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct TiffanyNativeChatStore {
    #[serde(default = "native_sessions_schema_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) conversations: Vec<TiffanyNativeChatConversation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct TiffanyNativeChatConversation {
    pub(crate) id: String,
    pub(crate) cwd: String,
    pub(crate) created_at_unix: u64,
    pub(crate) updated_at_unix: u64,
    #[serde(default)]
    pub(crate) turns: Vec<TiffanyNativeChatTurn>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct TiffanyNativeChatTurn {
    pub(crate) user_prompt: String,
    pub(crate) result: String,
    pub(crate) captured_at_unix: u64,
    #[serde(default)]
    pub(crate) events: Vec<TiffanyNativeChatEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct TiffanyNativeChatEvent {
    pub(crate) role: String,
    pub(crate) status: String,
    pub(crate) title: String,
    pub(crate) content: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) worker_role: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) native_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyOrchestratorConfig {
    #[serde(default)]
    providers: HashMap<String, RawTiffanyProviderConfig>,
    #[serde(default)]
    runtimes: HashMap<String, RawTiffanyRuntimeConfig>,
    #[serde(default)]
    models: Vec<RawTiffanyModelConfig>,
    #[serde(default)]
    roles: HashMap<String, RawTiffanyRoleConfig>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyProviderConfig {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyRuntimeConfig {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: Option<String>,
    binary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyModelConfig {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    provider: String,
    #[allow(dead_code)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyRoleConfig {
    #[allow(dead_code)]
    model: String,
    runtime: String,
}

fn memory_schema_version() -> u32 {
    MEMORY_SCHEMA_VERSION
}

fn native_sessions_schema_version() -> u32 {
    NATIVE_SESSIONS_SCHEMA_VERSION
}

pub(crate) fn memory_max_turns() -> usize {
    MEMORY_MAX_TURNS
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

#[derive(Clone, Debug, Default, Deserialize)]
struct TiffanyProgressEvent {
    role: String,
    status: String,
    message: String,
    task_id: Option<String>,
    agent: Option<String>,
    worker_role: Option<String>,
    runtime: Option<String>,
    cc_agent: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    worker_thread_id: Option<String>,
    native_session_id: Option<String>,
    reused: Option<bool>,
    task_prompt: Option<String>,
    content: Option<String>,
    approved: Option<bool>,
    issues: Option<usize>,
    count: Option<usize>,
    duration_ms: Option<u64>,
    reason: Option<String>,
    route: Option<String>,
    route_label: Option<String>,
    route_reason_label: Option<String>,
    flow_steps: Option<String>,
}

#[derive(Default)]
struct BridgeState {
    seen_outputs: HashSet<String>,
    seen_statuses: HashSet<String>,
    malformed_stdout: VecDeque<String>,
    final_output: Option<String>,
    worker_metadata: HashMap<String, WorkerMeta>,
    pending_timeline: Vec<TiffanyProgressEvent>,
    native_events: Vec<TiffanyNativeChatEvent>,
}

#[derive(Clone, Debug, Default)]
struct WorkerMeta {
    agent: Option<String>,
    worker_role: Option<String>,
    runtime: Option<String>,
    cc_agent: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    worker_thread_id: Option<String>,
    native_session_id: Option<String>,
    reused: Option<bool>,
}

const TIFFANY_ROLE_USAGE: &str = "Usage: /roles [list|show <role>|register <role> --provider <provider> --model-name <api-model> --runtime <runtime>|profile <name> --planner model@runtime --worker-cc provider/model@runtime]";

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
                        body_line(TIFFANY_ROLE_USAGE, true),
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

pub(crate) fn spawn_thread_command(
    app_event_tx: AppEventSender,
    config: TiffanyOrchestratorConfig,
    args: String,
) {
    tokio::spawn(async move {
        let command_args = match thread_command_args(&args) {
            Ok(command_args) => command_args,
            Err(err) => {
                emit_lines(
                    &app_event_tx,
                    vec![
                        status_line("✗", Color::Red, "thread", &err),
                        body_line(
                            "Usage: /thread [list|show <role>|clear <role>|export <role>]",
                            true,
                        ),
                    ],
                );
                return;
            }
        };

        match run_orchestrator_command(&config, &command_args).await {
            Ok(output) => emit_command_output(&app_event_tx, "thread", &command_args, output),
            Err(err) => emit_lines(
                &app_event_tx,
                spawn_error_lines(
                    "thread",
                    "✗",
                    Color::Red,
                    &format!("failed to run thread command - {err}"),
                    &err.to_string(),
                ),
            ),
        }
    });
}

pub(crate) fn idle_intro_lines_with_readiness(
    context_turn_count: usize,
    readiness: Option<&TiffanyStartupReadiness>,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        brand_line("orchestration shell"),
        idle_status_line(context_turn_count),
    ];
    if let Some(readiness) = readiness {
        lines.extend(startup_readiness_lines(readiness));
    }
    lines.extend([
        workflow_line(),
        command_hint_line(
            "setup",
            &["/provider", "/role", "/roles", "/thread", "/history", "/doctor"],
        ),
        command_hint_line(
            "tools",
            &["/status", "/copy", "/raw", "/diff", "/clear", "/exit"],
        ),
    ]);
    lines
}

pub(crate) fn startup_readiness(
    config: Option<&TiffanyOrchestratorConfig>,
) -> TiffanyStartupReadiness {
    let bin = config
        .map(|config| config.bin.as_str())
        .filter(|bin| !bin.trim().is_empty())
        .unwrap_or("orchestrator");
    let runtime = runtime_status(bin);
    let config_path = config
        .and_then(|config| config.config_path.as_deref())
        .filter(|path| !path.trim().is_empty())
        .map(expand_home_path)
        .or_else(|| dirs::home_dir().map(|home| home.join(".orchestrator/config.yaml")));
    let config = match config_path {
        Some(path) => config_readiness_from_path(path),
        None => TiffanyConfigReadiness::Invalid {
            path: "~/.orchestrator/config.yaml".to_string(),
            message: "home directory unavailable".to_string(),
        },
    };
    TiffanyStartupReadiness { runtime, config }
}

fn config_readiness_from_path(path: std::path::PathBuf) -> TiffanyConfigReadiness {
    let display_path = path.to_string_lossy().into_owned();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return TiffanyConfigReadiness::Missing { path: display_path };
        }
        Err(err) => {
            return TiffanyConfigReadiness::Invalid {
                path: display_path,
                message: err.to_string(),
            };
        }
    };
    let config = match serde_yaml::from_str::<RawTiffanyOrchestratorConfig>(&raw) {
        Ok(config) => config,
        Err(err) => {
            return TiffanyConfigReadiness::Invalid {
                path: display_path,
                message: err.to_string(),
            };
        }
    };
    let default_worker = default_worker_summary(&config);
    TiffanyConfigReadiness::Ready(TiffanyConfigSummary {
        providers: config.providers.len(),
        models: config.models.len(),
        roles: config.roles.len(),
        runtimes: config.runtimes.len(),
        default_worker,
    })
}

fn default_worker_summary(
    config: &RawTiffanyOrchestratorConfig,
) -> Option<TiffanyDefaultWorkerSummary> {
    let role = default_worker_role(&config.roles)?;
    let role_cfg = config.roles.get(&role)?;
    let runtime_cfg = runtime_config(config, &role_cfg.runtime);
    let binary = runtime_cfg
        .and_then(|runtime| runtime.binary.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| default_binary_for_runtime(&role_cfg.runtime));
    let status = default_worker_status(config, role_cfg, runtime_cfg, &binary);
    Some(TiffanyDefaultWorkerSummary {
        role,
        runtime: role_cfg.runtime.clone(),
        binary,
        status,
    })
}

fn default_worker_status(
    config: &RawTiffanyOrchestratorConfig,
    role: &RawTiffanyRoleConfig,
    runtime: Option<&RawTiffanyRuntimeConfig>,
    binary: &str,
) -> TiffanyWorkerReadinessStatus {
    let Some(model) = config.models.iter().find(|model| model.id == role.model) else {
        return TiffanyWorkerReadinessStatus::ModelMissing;
    };
    if !config.providers.contains_key(&model.provider) {
        return TiffanyWorkerReadinessStatus::ProviderMissing;
    }
    if runtime.is_none() || runtime_status(binary).resolved.is_none() {
        return TiffanyWorkerReadinessStatus::RuntimeMissing;
    }
    TiffanyWorkerReadinessStatus::Ready
}

fn runtime_config<'a>(
    config: &'a RawTiffanyOrchestratorConfig,
    runtime: &str,
) -> Option<&'a RawTiffanyRuntimeConfig> {
    config.runtimes.get(runtime).or_else(|| {
        runtime_aliases(runtime)
            .iter()
            .find_map(|alias| config.runtimes.get(*alias))
    })
}

fn runtime_aliases(runtime: &str) -> &'static [&'static str] {
    match runtime {
        "codex" => &["codex"],
        "claude-code" | "claude" | "cc" => &["claude-code", "claude", "cc"],
        "gemini" | "gemini-cli" => &["gemini", "gemini-cli"],
        _ => &[],
    }
}

fn default_binary_for_runtime(runtime: &str) -> String {
    if is_claude_runtime(runtime) {
        "claude".to_string()
    } else if is_codex_runtime(runtime) {
        "codex".to_string()
    } else if is_gemini_runtime(runtime) {
        "gemini".to_string()
    } else {
        runtime.to_string()
    }
}

fn default_worker_role(roles: &HashMap<String, RawTiffanyRoleConfig>) -> Option<String> {
    default_worker_role_for_runtime(roles, is_claude_runtime, "worker-cc")
        .or_else(|| default_worker_role_for_runtime(roles, is_codex_runtime, "worker-codex"))
        .or_else(|| default_worker_role_for_runtime(roles, is_gemini_runtime, "worker-gemini"))
}

fn default_worker_role_for_runtime(
    roles: &HashMap<String, RawTiffanyRoleConfig>,
    runtime_matches: impl Fn(&str) -> bool,
    preferred: &str,
) -> Option<String> {
    if roles.contains_key(preferred) {
        return Some(preferred.to_string());
    }
    let mut candidates = roles
        .iter()
        .filter(|(name, role)| is_worker_role_name(name) && runtime_matches(&role.runtime))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

fn is_claude_runtime(runtime: &str) -> bool {
    matches!(runtime, "claude-code" | "claude" | "cc")
}

fn is_codex_runtime(runtime: &str) -> bool {
    runtime == "codex"
}

fn is_gemini_runtime(runtime: &str) -> bool {
    matches!(runtime, "gemini" | "gemini-cli")
}

fn is_worker_role_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    (name.contains("worker") || name.contains("executor")) && !name.contains("reviewer")
}

fn startup_readiness_lines(readiness: &TiffanyStartupReadiness) -> Vec<Line<'static>> {
    let runtime_state = if readiness.runtime.resolved.is_some() {
        "ok"
    } else {
        "missing"
    };
    let mut lines = vec![readiness_line(
        "runtime",
        runtime_state,
        readiness.runtime.requested.clone(),
        if readiness.runtime.resolved.is_some() {
            TIFFANY_SOFT
        } else {
            Color::Red
        },
    )];
    match &readiness.config {
        TiffanyConfigReadiness::Missing { path } => {
            lines.push(readiness_line(
                "config",
                "missing",
                path.clone(),
                Color::Red,
            ));
            lines.push(readiness_line(
                "next",
                "/provider",
                "/role then /doctor".to_string(),
                TIFFANY_SOFT,
            ));
        }
        TiffanyConfigReadiness::Invalid { path, message } => {
            lines.push(readiness_line(
                "config",
                "invalid",
                path.clone(),
                Color::Red,
            ));
            lines.push(readiness_line(
                "error",
                "parse",
                truncate_for_status(message, 80),
                Color::Red,
            ));
        }
        TiffanyConfigReadiness::Ready(summary) => {
            lines.push(readiness_line(
                "config",
                "ok",
                format!(
                    "{} providers · {} models · {} roles · {} runtimes",
                    summary.providers, summary.models, summary.roles, summary.runtimes
                ),
                TIFFANY_SOFT,
            ));
            let (state, detail, color) = match &summary.default_worker {
                Some(worker) => (
                    worker.status.label(),
                    format!("{} · {} · {}", worker.role, worker.runtime, worker.binary),
                    worker.status.color(),
                ),
                None => (
                    "missing",
                    "register worker-cc or worker-codex".to_string(),
                    Color::Red,
                ),
            };
            lines.push(readiness_line("worker", state, detail, color));
        }
    }
    lines
}

impl TiffanyWorkerReadinessStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::ModelMissing => "model-missing",
            Self::ProviderMissing => "provider-missing",
            Self::RuntimeMissing => "runtime-missing",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ready => TIFFANY_SOFT,
            Self::ModelMissing | Self::ProviderMissing | Self::RuntimeMissing => Color::Red,
        }
    }
}

fn readiness_line(
    label: &'static str,
    state: &'static str,
    detail: String,
    state_color: Color,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            label,
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(state, Style::default().fg(state_color)),
        Span::raw("  "),
        Span::styled(detail, Style::default().fg(TIFFANY_DARK)),
    ])
}

fn truncate_for_status(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
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

pub(crate) fn load_memory_turns(
    codex_home: &Path,
    cwd: &Path,
) -> anyhow::Result<VecDeque<TiffanyOrchestratorTurn>> {
    let path = memory_path(codex_home);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(VecDeque::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let memory: TiffanyOrchestratorMemory =
        serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    let cwd_key = cwd_key(cwd);
    let turns = memory
        .conversations
        .into_iter()
        .find(|conversation| conversation.cwd == cwd_key)
        .map(|conversation| conversation.turns)
        .unwrap_or_default()
        .into_iter()
        .filter_map(memory_turn_into_turn)
        .take(MEMORY_MAX_TURNS)
        .collect::<VecDeque<_>>();
    Ok(turns)
}

pub(crate) fn save_memory_turns(
    codex_home: &Path,
    cwd: &Path,
    turns: &VecDeque<TiffanyOrchestratorTurn>,
) -> anyhow::Result<()> {
    let path = memory_path(codex_home);
    let mut memory = match std::fs::read_to_string(&path) {
        Ok(body) => serde_json::from_str::<TiffanyOrchestratorMemory>(&body)
            .with_context(|| format!("parsing {}", path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            TiffanyOrchestratorMemory::default()
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    memory.version = MEMORY_SCHEMA_VERSION;
    let cwd_key = cwd_key(cwd);
    let memory_turns = turns
        .iter()
        .rev()
        .take(MEMORY_MAX_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(|turn| turn_into_memory_turn(turn))
        .collect::<Vec<_>>();

    if let Some(conversation) = memory
        .conversations
        .iter_mut()
        .find(|conversation| conversation.cwd == cwd_key)
    {
        conversation.turns = memory_turns;
    } else {
        memory
            .conversations
            .push(TiffanyOrchestratorMemoryConversation {
                cwd: cwd_key,
                turns: memory_turns,
            });
    }

    memory
        .conversations
        .retain(|conversation| !conversation.turns.is_empty());

    let body = serde_json::to_string_pretty(&memory)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_native_chat_conversation(
    codex_home: &Path,
    cwd: &Path,
) -> anyhow::Result<Option<TiffanyNativeChatConversation>> {
    let store = load_native_chat_store(codex_home)?;
    let cwd_key = cwd_key(cwd);
    Ok(store
        .conversations
        .into_iter()
        .find(|conversation| conversation.cwd == cwd_key))
}

pub(crate) fn load_native_memory_turns(
    codex_home: &Path,
    cwd: &Path,
) -> anyhow::Result<VecDeque<TiffanyOrchestratorTurn>> {
    let turns = load_native_chat_conversation(codex_home, cwd)?
        .map(|conversation| conversation.turns)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .take(MEMORY_MAX_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(native_chat_turn_into_turn)
        .collect::<VecDeque<_>>();
    Ok(turns)
}

pub(crate) fn native_history_lines(codex_home: &Path, cwd: &Path, args: &str) -> Vec<Line<'static>> {
    let command = match NativeHistoryCommand::parse(args) {
        Ok(command) => command,
        Err(err) => {
            return vec![
                status_line("✗", Color::Red, "history", &err),
                body_line("Usage: /history [last|full|<count>|export [--out path]]", true),
            ];
        }
    };
    match load_native_chat_conversation(codex_home, cwd) {
        Ok(Some(conversation)) => match command {
            NativeHistoryCommand::Show(opts) => native_history_conversation_lines(&conversation, opts),
            NativeHistoryCommand::Export { out } => {
                native_history_export_lines(codex_home, cwd, &conversation, out)
            }
        },
        Ok(None) => vec![
            status_line(
                "⚠",
                Color::Yellow,
                "history",
                "no Tiffany native conversation saved for this directory",
            ),
            next_line("run a prompt", "create a native conversation turn"),
        ],
        Err(err) => vec![
            status_line("✗", Color::Red, "history", "could not read native history"),
            body_line(&format!("{err:#}"), true),
        ],
    }
}

pub(crate) fn append_native_chat_turn(
    codex_home: &Path,
    cwd: &Path,
    turn: &TiffanyOrchestratorTurn,
    events: Vec<TiffanyNativeChatEvent>,
) -> anyhow::Result<()> {
    let Some(native_turn) = native_chat_turn_from_turn(turn, events) else {
        return Ok(());
    };

    let path = native_sessions_path(codex_home);
    let mut store = load_native_chat_store(codex_home)?;
    store.version = NATIVE_SESSIONS_SCHEMA_VERSION;

    let cwd_key = cwd_key(cwd);
    let now = native_turn.captured_at_unix;
    if let Some(conversation) = store
        .conversations
        .iter_mut()
        .find(|conversation| conversation.cwd == cwd_key)
    {
        conversation.updated_at_unix = now;
        conversation.turns.push(native_turn);
    } else {
        store.conversations.push(TiffanyNativeChatConversation {
            id: native_conversation_id(&cwd_key),
            cwd: cwd_key,
            created_at_unix: now,
            updated_at_unix: now,
            turns: vec![native_turn],
        });
    }

    store
        .conversations
        .retain(|conversation| !conversation.turns.is_empty());

    let body = serde_json::to_string_pretty(&store)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct NativeHistoryOptions {
    turn_limit: usize,
    event_limit_per_turn: usize,
}

#[derive(Clone, Debug)]
enum NativeHistoryCommand {
    Show(NativeHistoryOptions),
    Export { out: Option<PathBuf> },
}

impl NativeHistoryCommand {
    fn parse(args: &str) -> Result<Self, String> {
        let parts = args.split_whitespace().collect::<Vec<_>>();
        match parts.as_slice() {
            [] | ["last"] | ["summary"] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 5,
                event_limit_per_turn: 6,
            })),
            ["full"] | ["all"] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 10,
                event_limit_per_turn: 20,
            })),
            [count] if count.parse::<usize>().is_ok() => {
                let count = count.parse::<usize>().unwrap_or_default();
                if count == 0 {
                    return Err("history count must be greater than zero".to_string());
                }
                Ok(Self::Show(NativeHistoryOptions {
                    turn_limit: count.min(50),
                    event_limit_per_turn: 10,
                }))
            }
            ["export"] => Ok(Self::Export { out: None }),
            ["export", "--out" | "-o", path] => Ok(Self::Export {
                out: Some(PathBuf::from(path)),
            }),
            ["export", flag, ..] if matches!(*flag, "--out" | "-o") => {
                Err("history export --out needs a file path".to_string())
            }
            ["export", flag, ..] => Err(format!("unknown /history export option '{flag}'")),
            [unknown, ..] => Err(format!("unknown /history option '{unknown}'")),
        }
    }
}

fn native_history_conversation_lines(
    conversation: &TiffanyNativeChatConversation,
    opts: NativeHistoryOptions,
) -> Vec<Line<'static>> {
    let total_turns = conversation.turns.len();
    let shown_turns = total_turns.min(opts.turn_limit);
    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "history",
        &format!(
            "{} turn(s) saved · showing latest {shown_turns}",
            total_turns
        ),
    )];
    lines.push(thread_meta_line("cwd", &conversation.cwd));
    lines.push(thread_meta_line("session", &conversation.id));

    for (idx, turn) in conversation
        .turns
        .iter()
        .rev()
        .take(opts.turn_limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .enumerate()
    {
        let turn_number = total_turns.saturating_sub(shown_turns) + idx + 1;
        lines.push(body_line(
            &format!(
                "{}. user  {}",
                turn_number,
                truncate_text(&one_line(&turn.user_prompt), 120)
            ),
            false,
        ));
        lines.push(body_line(
            &format!("   answer {}", truncate_text(&one_line(&turn.result), 140)),
            true,
        ));
        append_native_history_event_lines(&mut lines, turn, opts.event_limit_per_turn);
    }

    if total_turns > opts.turn_limit {
        lines.push(body_line(
            &format!("… {} older turn(s) hidden", total_turns - opts.turn_limit),
            true,
        ));
    }
    lines.push(next_line("/history full", "show more native process events"));
    lines.push(next_line("/thread <role>", "inspect native CLI resume command"));
    lines
}

fn native_history_export_lines(
    codex_home: &Path,
    cwd: &Path,
    conversation: &TiffanyNativeChatConversation,
    out: Option<PathBuf>,
) -> Vec<Line<'static>> {
    let path = out.unwrap_or_else(|| default_native_history_export_path(codex_home, cwd));
    let body = render_native_history_markdown(conversation);
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return vec![
            status_line("✗", Color::Red, "history", "could not create export directory"),
            body_line(&format!("{err:#}"), true),
        ];
    }
    match std::fs::write(&path, body.as_bytes()) {
        Ok(()) => vec![
            status_line(
                "✓",
                TIFFANY_BLUE,
                "history",
                &format!("exported {} turn(s)", conversation.turns.len()),
            ),
            thread_meta_line("target", &path.display().to_string()),
            thread_meta_line("bytes", &body.len().to_string()),
            next_line("open export", &path.display().to_string()),
        ],
        Err(err) => vec![
            status_line("✗", Color::Red, "history", "could not write export"),
            body_line(&format!("{err:#}"), true),
        ],
    }
}

fn default_native_history_export_path(codex_home: &Path, cwd: &Path) -> PathBuf {
    codex_home
        .join("tiffany-orchestrator")
        .join("history-exports")
        .join(format!("{}.md", native_conversation_id(&cwd_key(cwd))))
}

fn render_native_history_markdown(conversation: &TiffanyNativeChatConversation) -> String {
    let mut out = String::new();
    out.push_str("# Tiffany Native Conversation History\n\n");
    out.push_str(&format!("- Conversation: `{}`\n", conversation.id));
    out.push_str(&format!("- CWD: `{}`\n", conversation.cwd));
    out.push_str(&format!("- Turns: {}\n", conversation.turns.len()));
    out.push_str(&format!("- Created: {}\n", conversation.created_at_unix));
    out.push_str(&format!("- Updated: {}\n\n", conversation.updated_at_unix));

    for (idx, turn) in conversation.turns.iter().enumerate() {
        out.push_str(&format!("## Turn {}\n\n", idx + 1));
        out.push_str("### User\n\n");
        out.push_str(turn.user_prompt.trim());
        out.push_str("\n\n### Assistant Result\n\n");
        out.push_str(turn.result.trim());
        out.push_str("\n\n### Native Events\n\n");
        if turn.events.is_empty() {
            out.push_str("_No native events captured._\n\n");
            continue;
        }
        for event in &turn.events {
            out.push_str(&format!(
                "- **{} {}**",
                status_glyph(&event.status),
                markdown_escape_inline(&event.title)
            ));
            let meta = native_event_markdown_meta(event);
            if !meta.is_empty() {
                out.push_str(&format!("  \n  {}", meta.join(" · ")));
            }
            out.push('\n');
            if let Some(content) = event.content.as_deref().and_then(nonempty_trimmed) {
                out.push_str("\n```text\n");
                out.push_str(content.trim_end());
                out.push_str("\n```\n");
            }
        }
        out.push('\n');
    }
    out
}

fn native_event_markdown_meta(event: &TiffanyNativeChatEvent) -> Vec<String> {
    let mut meta = Vec::new();
    if let Some(role) = event.worker_role.as_deref().and_then(nonempty_trimmed) {
        meta.push(format!("role `{}`", markdown_escape_inline(role)));
    }
    if let Some(agent) = event.agent.as_deref().and_then(nonempty_trimmed) {
        meta.push(format!("agent `{}`", markdown_escape_inline(agent)));
    }
    if let Some(provider) = event.provider.as_deref().and_then(nonempty_trimmed) {
        meta.push(format!("provider `{}`", markdown_escape_inline(provider)));
    }
    if let Some(model) = event.model.as_deref().and_then(nonempty_trimmed) {
        meta.push(format!("model `{}`", markdown_escape_inline(model)));
    }
    if let Some(native) = event.native_session_id.as_deref().and_then(nonempty_trimmed) {
        meta.push(format!("native `{}`", markdown_escape_inline(native)));
    }
    meta
}

fn markdown_escape_inline(value: &str) -> String {
    value.replace('`', "\\`")
}

fn append_native_history_event_lines(
    lines: &mut Vec<Line<'static>>,
    turn: &TiffanyNativeChatTurn,
    limit: usize,
) {
    if turn.events.is_empty() {
        lines.push(body_line("   native events none captured", true));
        return;
    }
    for event in turn.events.iter().take(limit) {
        let mut label = format!("   {} {}", status_glyph(&event.status), event.title);
        if let Some(native) = event.native_session_id.as_deref().and_then(nonempty_trimmed) {
            label.push_str(" · native ");
            label.push_str(short_task_id(Some(native)).unwrap_or(native));
        }
        lines.push(body_line(&truncate_text(&label, 180), true));
        if let Some(content) = event.content.as_deref().and_then(nonempty_trimmed) {
            for line in content.lines().take(4) {
                lines.push(detail_continuation_line(&truncate_text(line, 180)));
            }
            let hidden = content.lines().count().saturating_sub(4);
            if hidden > 0 {
                lines.push(detail_continuation_line(&format!("… {hidden} more line(s)")));
            }
        }
    }
    if turn.events.len() > limit {
        lines.push(body_line(
            &format!("   … {} more native event(s)", turn.events.len() - limit),
            true,
        ));
    }
}

fn memory_path(codex_home: &Path) -> std::path::PathBuf {
    codex_home.join(MEMORY_FILE)
}

fn native_sessions_path(codex_home: &Path) -> std::path::PathBuf {
    codex_home.join(NATIVE_SESSIONS_FILE)
}

fn load_native_chat_store(codex_home: &Path) -> anyhow::Result<TiffanyNativeChatStore> {
    let path = native_sessions_path(codex_home);
    let store = match std::fs::read_to_string(&path) {
        Ok(body) => serde_json::from_str::<TiffanyNativeChatStore>(&body)
            .with_context(|| format!("parsing {}", path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            TiffanyNativeChatStore::default()
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    Ok(TiffanyNativeChatStore {
        version: NATIVE_SESSIONS_SCHEMA_VERSION,
        conversations: store
            .conversations
            .into_iter()
            .filter_map(normalize_native_conversation)
            .collect(),
    })
}

fn cwd_key(cwd: &Path) -> String {
    cwd.canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn native_conversation_id(cwd_key: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in cwd_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("tiffany-native-{hash:016x}")
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn normalize_native_conversation(
    conversation: TiffanyNativeChatConversation,
) -> Option<TiffanyNativeChatConversation> {
    let cwd = conversation.cwd.trim().to_string();
    if cwd.is_empty() {
        return None;
    }

    let turns = conversation
        .turns
        .into_iter()
        .filter_map(normalize_native_turn)
        .collect::<Vec<_>>();
    if turns.is_empty() {
        return None;
    }

    let id = if conversation.id.trim().is_empty() {
        native_conversation_id(&cwd)
    } else {
        conversation.id.trim().to_string()
    };
    let first_turn_ts = turns
        .first()
        .map(|turn| turn.captured_at_unix)
        .unwrap_or_default();
    let last_turn_ts = turns
        .last()
        .map(|turn| turn.captured_at_unix)
        .unwrap_or(first_turn_ts);
    let created_at_unix = if conversation.created_at_unix == 0 {
        first_turn_ts
    } else {
        conversation.created_at_unix
    };
    let updated_at_unix = conversation.updated_at_unix.max(last_turn_ts);

    Some(TiffanyNativeChatConversation {
        id,
        cwd,
        created_at_unix,
        updated_at_unix,
        turns,
    })
}

fn normalize_native_turn(turn: TiffanyNativeChatTurn) -> Option<TiffanyNativeChatTurn> {
    let user_prompt = turn.user_prompt.trim().to_string();
    let result = turn.result.trim().to_string();
    if user_prompt.is_empty() || result.is_empty() {
        return None;
    }
    Some(TiffanyNativeChatTurn {
        user_prompt,
        result,
        captured_at_unix: turn.captured_at_unix,
        events: turn
            .events
            .into_iter()
            .filter_map(normalize_native_event)
            .collect(),
    })
}

fn normalize_native_event(event: TiffanyNativeChatEvent) -> Option<TiffanyNativeChatEvent> {
    let role = event.role.trim().to_string();
    let status = event.status.trim().to_string();
    let title = event.title.trim().to_string();
    if role.is_empty() || status.is_empty() || title.is_empty() {
        return None;
    }
    Some(TiffanyNativeChatEvent {
        role,
        status,
        title,
        content: event.content.and_then(trimmed_string),
        agent: event.agent.and_then(trimmed_string),
        worker_role: event.worker_role.and_then(trimmed_string),
        model: event.model.and_then(trimmed_string),
        provider: event.provider.and_then(trimmed_string),
        task_id: event.task_id.and_then(trimmed_string),
        native_session_id: event.native_session_id.and_then(trimmed_string),
    })
}

fn trimmed_string(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn memory_turn_into_turn(turn: TiffanyOrchestratorMemoryTurn) -> Option<TiffanyOrchestratorTurn> {
    let user_prompt = turn.user_prompt.trim().to_string();
    let result = turn.result.trim().to_string();
    if user_prompt.is_empty() || result.is_empty() {
        return None;
    }
    Some(TiffanyOrchestratorTurn {
        user_prompt,
        result,
    })
}

fn turn_into_memory_turn(turn: &TiffanyOrchestratorTurn) -> Option<TiffanyOrchestratorMemoryTurn> {
    let user_prompt = turn.user_prompt.trim();
    let result = turn.result.trim();
    if user_prompt.is_empty() || result.is_empty() {
        return None;
    }
    Some(TiffanyOrchestratorMemoryTurn {
        user_prompt: user_prompt.to_string(),
        result: result.to_string(),
    })
}

fn native_chat_turn_from_turn(
    turn: &TiffanyOrchestratorTurn,
    events: Vec<TiffanyNativeChatEvent>,
) -> Option<TiffanyNativeChatTurn> {
    let user_prompt = turn.user_prompt.trim();
    let result = turn.result.trim();
    if user_prompt.is_empty() || result.is_empty() {
        return None;
    }
    Some(TiffanyNativeChatTurn {
        user_prompt: user_prompt.to_string(),
        result: result.to_string(),
        captured_at_unix: now_unix_seconds(),
        events,
    })
}

fn native_chat_turn_into_turn(turn: TiffanyNativeChatTurn) -> Option<TiffanyOrchestratorTurn> {
    let user_prompt = turn.user_prompt.trim().to_string();
    let result = turn.result.trim().to_string();
    if user_prompt.is_empty() || result.is_empty() {
        return None;
    }
    Some(TiffanyOrchestratorTurn {
        user_prompt,
        result,
    })
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
    let bin = normalized_orchestrator_bin(&launch.bin);
    let mut command = orchestrator_command(bin, launch.config_path.as_deref());
    let mut child = command
        .arg("events")
        .arg(&launch.prompt)
        .args(&launch.extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            anyhow::anyhow!(
                "failed to start `{}` - {err}; install the `orchestrator` binary next to `tiffany-loop`, pass `--bin /path/to/orchestrator`, or set TIFFANY_ORCHESTRATOR_BIN",
                bin
            )
        })?;

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
    state.flush_timeline(&app_event_tx);
    if status.success() {
        if let Some(result) = state.final_output.take() {
            app_event_tx.send(AppEvent::TiffanyOrchestratorTurnCaptured {
                user_prompt: launch.user_prompt,
                result,
                native_events: state.native_events,
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
    let mut command = Command::new(normalized_orchestrator_bin(bin));
    if let Some(config_path) = config_path.filter(|path| !path.trim().is_empty()) {
        command.arg("--config").arg(config_path);
    }
    command
}

fn normalized_orchestrator_bin(bin: &str) -> &str {
    let bin = bin.trim();
    if bin.is_empty() { "orchestrator" } else { bin }
}

pub(crate) fn runtime_status(bin: &str) -> TiffanyOrchestratorRuntimeStatus {
    let requested = normalized_orchestrator_bin(bin).to_string();
    let resolved =
        resolve_orchestrator_runtime(&requested).map(|path| path.to_string_lossy().into_owned());
    TiffanyOrchestratorRuntimeStatus {
        requested,
        resolved,
    }
}

fn resolve_orchestrator_runtime(bin: &str) -> Option<std::path::PathBuf> {
    let path = expand_home_path(bin);
    if bin_has_path_separator(bin) || path.is_absolute() {
        return is_launchable_file(&path).then_some(path);
    }

    if bin == "orchestrator"
        && let Ok(current_exe) = std::env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        let adjacent = parent.join(executable_name("orchestrator"));
        if is_launchable_file(&adjacent) {
            return Some(adjacent);
        }
    }

    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .flat_map(|dir| executable_candidates(&dir, bin))
        .find(|path| is_launchable_file(path))
}

pub(crate) fn expand_home_path(path: &str) -> std::path::PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(path)
}

fn bin_has_path_separator(bin: &str) -> bool {
    bin.contains('/') || bin.contains('\\')
}

fn executable_candidates(dir: &std::path::Path, bin: &str) -> Vec<std::path::PathBuf> {
    if cfg!(windows) && std::path::Path::new(bin).extension().is_none() {
        let pathext = std::env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
        pathext
            .split(';')
            .filter(|ext| !ext.trim().is_empty())
            .map(|ext| dir.join(format!("{bin}{ext}")))
            .collect()
    } else {
        vec![dir.join(bin)]
    }
}

#[cfg(unix)]
fn is_launchable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_launchable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
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
        "list" | "show" | "register" | "profile" => {
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

fn thread_command_args(args: &str) -> Result<Vec<String>, String> {
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
            "install claude/codex/gemini or change the role runtime",
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
        "thread" => concise_thread_success(command_args, output),
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThreadSummary {
    role: String,
    active: bool,
    runtime: String,
    model: String,
    thread: Option<String>,
    native: Option<String>,
    last: Option<String>,
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
        Some("profile") => role_profile_summary_lines(command_args, output),
        Some("list") => role_summary_lines(&String::from_utf8_lossy(&output.stdout), "roles"),
        Some("show") => role_summary_lines(&String::from_utf8_lossy(&output.stdout), "role"),
        _ => None,
    }
}

fn role_profile_summary_lines(
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let name = command_args
        .get(2)
        .map(String::as_str)
        .or_else(|| parse_prefixed_field(&stdout, "Role profile saved:"))
        .or_else(|| parse_prefixed_field(&stdout, "Role profile dry-run:"))
        .unwrap_or("profile");
    let rows = parse_role_profile_rows(&stdout);
    if rows.is_empty() {
        return None;
    }

    let dry_run = stdout.contains("Role profile dry-run:");
    let title = if dry_run {
        format!("{name} preview")
    } else {
        format!("{name} saved")
    };
    let mut lines = vec![status_line(
        if dry_run { "●" } else { "✓" },
        TIFFANY_BLUE,
        "profile",
        &title,
    )];
    for row in rows.iter().take(8) {
        lines.push(role_profile_row_line(row));
    }
    if rows.len() > 8 {
        lines.push(body_line(&format!("… {} more", rows.len() - 8), true));
    }
    lines.push(next_line("/roles", "review active role bindings"));
    lines.push(next_line("/doctor", "verify provider/model/runtime wiring"));
    Some(lines)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RoleProfileRow {
    role: String,
    model: String,
    runtime: String,
    agent_teams: String,
}

fn parse_role_profile_rows(text: &str) -> Vec<RoleProfileRow> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix('✓')?.trim();
            let mut parts = line.split_whitespace();
            let role = parts.next()?.to_string();
            let mut model = None;
            let mut runtime = None;
            let mut agent_teams = None;
            for part in parts {
                if let Some(value) = part.strip_prefix("model=") {
                    model = Some(value.to_string());
                } else if let Some(value) = part.strip_prefix("runtime=") {
                    runtime = Some(value.to_string());
                } else if let Some(value) = part.strip_prefix("agent_teams=") {
                    agent_teams = Some(value.to_string());
                }
            }
            Some(RoleProfileRow {
                role,
                model: model?,
                runtime: runtime?,
                agent_teams: agent_teams.unwrap_or_else(|| "false".to_string()),
            })
        })
        .collect()
}

fn parse_prefixed_field<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(prefix).map(str::trim))
        .filter(|value| !value.is_empty())
}

fn role_profile_row_line(row: &RoleProfileRow) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "  ✓ ",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<13}", row.role),
            Style::default()
                .fg(TIFFANY_SOFT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(row.model.clone(), Style::default()),
        Span::styled(" · ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(row.runtime.clone(), Style::default().fg(Color::DarkGray)),
        Span::styled(" · teams ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            row.agent_teams.clone(),
            Style::default().fg(Color::DarkGray),
        ),
    ])
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

fn concise_thread_success(
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    if command_args.first().map(String::as_str) != Some("thread") {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    match command_args.get(1).map(String::as_str) {
        Some("list") => thread_list_summary_lines(&stdout),
        Some("show") => thread_detail_summary_lines(&stdout),
        Some("clear") => thread_clear_summary_lines(&stdout),
        Some("export") => thread_export_summary_lines(&stdout),
        _ => None,
    }
}

fn thread_list_summary_lines(text: &str) -> Option<Vec<Line<'static>>> {
    let rows = parse_thread_list_summaries(text);
    if rows.is_empty() {
        if text.contains("no worker roles configured") {
            return Some(vec![
                status_line("⚠", Color::Yellow, "thread", "no worker roles configured"),
                next_line("/role", "register worker roles before running tasks"),
            ]);
        }
        return None;
    }

    let active_count = rows.iter().filter(|row| row.active).count();
    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "thread",
        &format!("{active_count}/{} role(s) have worker sessions", rows.len()),
    )];
    for row in rows.iter().take(10) {
        lines.push(thread_summary_line(row));
    }
    if rows.len() > 10 {
        lines.push(body_line(&format!("… {} more", rows.len() - 10), true));
    }
    lines.push(next_line(
        "/thread <role>",
        "inspect native session and manual resume command",
    ));
    lines.push(next_line(
        "/thread export <role>",
        "write full selectable history for handoff",
    ));
    lines.push(next_line(
        "/thread clear <role>",
        "start the next run with a fresh native session",
    ));
    Some(lines)
}

fn thread_detail_summary_lines(text: &str) -> Option<Vec<Line<'static>>> {
    let fields = parse_thread_fields(text);
    let role = fields
        .get("role")
        .cloned()
        .or_else(|| parse_worker_thread_title(text))
        .unwrap_or_else(|| "worker".to_string());
    let status = fields
        .get("status")
        .cloned()
        .unwrap_or_else(|| "ready".to_string());
    let native = fields
        .get("native session")
        .cloned()
        .unwrap_or_else(|| "none".to_string());
    let symbol = if native == "none" { "⚠" } else { "✓" };
    let color = if native == "none" {
        Color::Yellow
    } else {
        TIFFANY_BLUE
    };
    let mut lines = vec![status_line(
        symbol,
        color,
        "thread",
        &format!("{role} · {status}"),
    )];

    push_thread_meta(&mut lines, "runtime", fields.get("runtime"));
    push_thread_meta(&mut lines, "agent", fields.get("agent"));
    push_thread_meta(&mut lines, "model", fields.get("model"));
    push_thread_meta(&mut lines, "native", Some(&native));
    push_thread_meta(&mut lines, "resume", fields.get("native resume"));
    push_thread_meta(&mut lines, "thread", fields.get("tiffany thread"));
    push_thread_meta(&mut lines, "last", fields.get("last tiffany session"));
    push_thread_meta(&mut lines, "work", fields.get("worktree"));

    if native == "none" {
        next_line_into(
            &mut lines,
            "/thread clear <role>",
            "already fresh; run a task to capture native session",
        );
    } else {
        next_line_into(
            &mut lines,
            &format!("/thread export {role}"),
            "write last Tiffany session for handoff",
        );
        next_line_into(
            &mut lines,
            &format!("/thread clear {role}"),
            "clear only native CLI session id",
        );
    }
    Some(lines)
}

fn thread_clear_summary_lines(text: &str) -> Option<Vec<Line<'static>>> {
    let fields = parse_thread_fields(text);
    let role = fields.get("role")?.clone();
    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "thread",
        &format!("{role} native session cleared"),
    )];
    push_thread_meta(&mut lines, "thread", fields.get("tiffany thread"));
    push_thread_meta(&mut lines, "cleared", fields.get("cleared native session"));
    push_thread_meta(&mut lines, "next", fields.get("next run"));
    lines.push(body_line(
        "kept Tiffany worker thread, last session pointer, and conversation context",
        true,
    ));
    lines.push(next_line(
        &format!("/thread {role}"),
        "confirm the next run will start fresh",
    ));
    Some(lines)
}

fn thread_export_summary_lines(text: &str) -> Option<Vec<Line<'static>>> {
    let fields = parse_thread_fields(text);
    let role = fields.get("role")?.clone();
    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "thread",
        &format!("{role} session exported"),
    )];
    push_thread_meta(&mut lines, "thread", fields.get("tiffany thread"));
    push_thread_meta(&mut lines, "session", fields.get("session"));
    push_thread_meta(&mut lines, "target", fields.get("target"));
    push_thread_meta(&mut lines, "bytes", fields.get("bytes"));
    if let Some(target) = fields
        .get("target")
        .and_then(|value| nonempty_trimmed(value))
    {
        if target != "clipboard" {
            lines.push(next_line("open export", target));
        }
    }
    lines.push(next_line(
        "/thread <role>",
        "inspect the native resume command for this worker",
    ));
    Some(lines)
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
    let actions = format!(
        "edit /role {}  thread /thread {}  profile /roles profile",
        role.name, role.name
    );
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
        Span::styled(
            format!("{:<34}", truncate_text(&health, 34)),
            Style::default().fg(color),
        ),
        Span::styled(
            truncate_text(&actions, 96),
            Style::default().fg(TIFFANY_SOFT),
        ),
    ])
}

fn thread_summary_line(thread: &ThreadSummary) -> Line<'static> {
    let (symbol, color) = if thread.active {
        ("✓", TIFFANY_BLUE)
    } else {
        ("○", Color::DarkGray)
    };
    let native = thread.native.as_deref().unwrap_or("none");
    let session = if thread.active {
        format!(
            "thread {}  native {}  last {}",
            thread.thread.as_deref().unwrap_or("none"),
            native,
            thread.last.as_deref().unwrap_or("none")
        )
    } else {
        "no worker thread yet".to_string()
    };
    let actions = if thread.active {
        format!(
            "inspect /thread {}  export /thread export {}  clear /thread clear {}",
            thread.role, thread.role, thread.role
        )
    } else {
        format!(
            "run task to create session  inspect /thread {}",
            thread.role
        )
    };
    Line::from(vec![
        Span::styled(
            format!("  {symbol} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<18}", thread.role),
            Style::default().fg(TIFFANY_SOFT),
        ),
        Span::styled(
            format!("{:<13}", truncate_text(&thread.runtime, 13)),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("{:<28}", truncate_text(&thread.model, 28)),
            Style::default(),
        ),
        Span::styled(
            format!("{:<72}", truncate_text(&session, 72)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            truncate_text(&actions, 128),
            Style::default().fg(TIFFANY_SOFT),
        ),
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

fn parse_thread_list_summaries(text: &str) -> Vec<ThreadSummary> {
    text.lines()
        .filter_map(parse_thread_list_line)
        .collect::<Vec<_>>()
}

fn parse_thread_list_line(line: &str) -> Option<ThreadSummary> {
    let trimmed = line.trim();
    let active = if let Some(rest) = trimmed.strip_prefix("● ") {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("○ ") {
        (false, rest)
    } else {
        return None;
    };
    let (active, rest) = active;
    let parts = rest.split_whitespace().collect::<Vec<_>>();
    let role = parts.first()?.to_string();
    let joined = parts.get(1..).unwrap_or(&[]).join(" ");
    if !active {
        let detail = joined
            .split(" · ")
            .next()
            .unwrap_or("not configured")
            .trim()
            .to_string();
        return Some(ThreadSummary {
            role,
            active,
            runtime: detail,
            model: String::new(),
            thread: None,
            native: None,
            last: None,
        });
    }

    let segments = joined.split(" · ").map(str::trim).collect::<Vec<_>>();
    let runtime = segments.first().copied().unwrap_or("runtime").to_string();
    let model = segments.get(1).copied().unwrap_or("model").to_string();
    Some(ThreadSummary {
        role,
        active,
        runtime,
        model,
        thread: segment_value(&segments, "thread"),
        native: segment_value(&segments, "native"),
        last: segment_value(&segments, "last"),
    })
}

fn segment_value(segments: &[&str], prefix: &str) -> Option<String> {
    let needle = format!("{prefix} ");
    segments
        .iter()
        .find_map(|segment| segment.strip_prefix(&needle))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_thread_fields(text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            fields.insert(key, value.to_string());
        }
    }
    fields
}

fn parse_worker_thread_title(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Worker thread "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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
    for line in diagnostic_detail_lines(&detail_text, FAILURE_DETAIL_MAX_LINES) {
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
            "hint: install the selected runtime binary (`claude`, `codex`, or `gemini`) or update the role runtime",
        );
    }
    if lower.contains("permission denied") || lower.contains("operation not permitted") {
        return Some(
            "hint: check file permissions, executable bit, macOS privacy access, or run /doctor",
        );
    }
    if lower.contains("codex")
        && (lower.contains("unexpected argument")
            || lower.contains("unknown option")
            || lower.contains("unrecognized option"))
        && (lower.contains("--cwd") || lower.contains("--cd"))
    {
        return Some(
            "hint: upgrade Codex CLI or set the role runtime binary to one that supports `codex exec --cd`; then run `/doctor`",
        );
    }
    if lower.contains("model not found")
        || lower.contains("unknown model")
        || lower.contains("invalid model")
        || lower.contains("模型不存在")
        || lower.contains("1211")
    {
        return Some(
            "hint: open `/role <role>` and set the provider API model name, or run `orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime>`; then run `/doctor`",
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

fn diagnostic_detail_lines(text: &str, max_lines: usize) -> Vec<String> {
    let lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(|line| event_format::humanize_user_visible_text(line, CONTROL_SUMMARY_MAX_CHARS))
        .collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return lines;
    }

    let head = FAILURE_DETAIL_HEAD_LINES.min(max_lines);
    let tail = max_lines.saturating_sub(head + 1);
    let hidden = lines.len().saturating_sub(head + tail);
    let mut out = Vec::with_capacity(max_lines);
    out.extend(lines.iter().take(head).cloned());
    out.push(format!(
        "... {hidden} earlier/middle line(s) hidden; use /doctor for full diagnostics"
    ));
    out.extend(
        lines
            .into_iter()
            .rev()
            .take(tail)
            .collect::<Vec<_>>()
            .into_iter()
            .rev(),
    );
    out
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

        let captured_result = event.role == "orchestrator"
            && event.status == "done"
            && self
                .final_output
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty());

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

        if let Some(native_event) = native_chat_event_from_progress(&event, visible.as_deref()) {
            self.native_events.push(native_event);
        }

        if event.status == "output" {
            self.flush_timeline(app_event_tx);
            emit_event_lines(app_event_tx, &event, visible);
        } else {
            self.pending_timeline.push(event);
            if should_flush_timeline_now(self.pending_timeline.last().expect("event just pushed")) {
                self.flush_timeline(app_event_tx);
            }
        }
        if captured_result {
            self.flush_timeline(app_event_tx);
            emit_lines(app_event_tx, vec![memory_capture_line()]);
        }
    }

    fn flush_timeline(&mut self, app_event_tx: &AppEventSender) {
        if self.pending_timeline.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.pending_timeline);
        emit_lines(app_event_tx, timeline_event_lines(&events));
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
        fill_present(&mut meta.cc_agent, event.cc_agent.as_deref());
        fill_present(&mut meta.model, event.model.as_deref());
        fill_present(&mut meta.provider, event.provider.as_deref());
        fill_present(
            &mut meta.worker_thread_id,
            event.worker_thread_id.as_deref(),
        );
        fill_present(
            &mut meta.native_session_id,
            event.native_session_id.as_deref(),
        );
        if event.reused.is_some() {
            meta.reused = event.reused;
        }
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
        fill_missing(&mut event.cc_agent, meta.cc_agent.as_deref());
        fill_missing(&mut event.model, meta.model.as_deref());
        fill_missing(&mut event.provider, meta.provider.as_deref());
        fill_missing(
            &mut event.worker_thread_id,
            meta.worker_thread_id.as_deref(),
        );
        fill_missing(
            &mut event.native_session_id,
            meta.native_session_id.as_deref(),
        );
        if event.reused.is_none() {
            event.reused = meta.reused;
        }
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

fn native_chat_event_from_progress(
    event: &TiffanyProgressEvent,
    visible: Option<&str>,
) -> Option<TiffanyNativeChatEvent> {
    let title = if event.status == "output" {
        output_title(event)
    } else {
        event_title(event)
    };
    normalize_native_event(TiffanyNativeChatEvent {
        role: event.role.clone(),
        status: event.status.clone(),
        title,
        content: visible
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        agent: event.agent.clone(),
        worker_role: event.worker_role.clone(),
        model: event.model.clone(),
        provider: event.provider.clone(),
        task_id: event.task_id.clone(),
        native_session_id: event.native_session_id.clone(),
    })
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
    if let Some(agent) = meta.cc_agent.as_deref().and_then(nonempty_trimmed) {
        parts.push(format!("claude agent {agent}"));
    }
    if let Some(provider_model) = worker_meta_provider_model_label(meta) {
        parts.push(provider_model);
    }
    if let Some(thread_id) = meta.worker_thread_id.as_deref().and_then(nonempty_trimmed) {
        let state = if meta.reused == Some(true) {
            "reused"
        } else {
            "thread"
        };
        parts.push(format!(
            "{state} {}",
            short_task_id(Some(thread_id)).unwrap_or(thread_id)
        ));
    }
    if let Some(native) = meta.native_session_id.as_deref().and_then(nonempty_trimmed) {
        parts.push(format!(
            "native {}",
            short_task_id(Some(native)).unwrap_or(native)
        ));
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
            run_reason_line(launch),
            run_workflow_line(launch),
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

fn idle_status_line(context_turn_count: usize) -> Line<'static> {
    let mut spans = vec![
        status_chip("status", "ready", TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("mode", "native", TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("memory", "on", TIFFANY_BLUE),
    ];
    if context_turn_count > 0 {
        spans.push(Span::raw("  "));
        spans.push(status_chip(
            "context",
            format!("{context_turn_count} turn(s)"),
            TIFFANY_SOFT,
        ));
    }
    Line::from(spans)
}

fn run_status_line(launch: &TiffanyOrchestratorLaunch) -> Line<'static> {
    let context = if launch.context_turn_count == 0 {
        "new".to_string()
    } else {
        format!("{} turn(s)", launch.context_turn_count)
    };
    let flow = launch_route(launch).display_label();
    Line::from(vec![
        status_chip("status", "running", TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("route", route_label(&launch.extra_args), TIFFANY_BLUE),
        Span::raw("  "),
        status_chip("flow", flow, TIFFANY_SOFT),
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
        Span::styled("direct", Style::default().fg(TIFFANY_SOFT)),
        Span::styled(" / ", Style::default().fg(TIFFANY_DARK)),
        Span::styled("single", Style::default().fg(TIFFANY_SOFT)),
        Span::styled(" / ", Style::default().fg(TIFFANY_DARK)),
        Span::styled("full", Style::default().fg(TIFFANY_SOFT)),
        Span::styled("  →  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            "selected per prompt",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn run_workflow_line(launch: &TiffanyOrchestratorLaunch) -> Line<'static> {
    match launch_route(launch) {
        event_format::OrchestrationRoute::DirectAnswer => direct_workflow_line("direct"),
        event_format::OrchestrationRoute::SingleWorker => direct_workflow_line("single"),
        event_format::OrchestrationRoute::FullPipeline => full_workflow_line(),
    }
}

fn run_reason_line(launch: &TiffanyOrchestratorLaunch) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "reason",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            launch_route(launch).reason(),
            Style::default().fg(TIFFANY_SOFT),
        ),
    ])
}

fn full_workflow_line() -> Line<'static> {
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

fn direct_workflow_line(label: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "flow",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(label, Style::default().fg(TIFFANY_SOFT)),
        Span::styled(" → ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            "worker",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" → ", Style::default().fg(TIFFANY_DARK)),
        Span::styled("answer", Style::default().fg(TIFFANY_SOFT)),
    ])
}

fn launch_route(launch: &TiffanyOrchestratorLaunch) -> event_format::OrchestrationRoute {
    event_format::classify_orchestration_route(&launch.prompt)
}

fn command_hint_line(label: &'static str, commands: &[&'static str]) -> Line<'static> {
    let mut spans = vec![Span::styled(
        label,
        Style::default()
            .fg(TIFFANY_BLUE)
            .add_modifier(Modifier::BOLD),
    )];
    for command in commands {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(*command, Style::default().fg(TIFFANY_SOFT)));
    }
    Line::from(spans)
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

fn should_flush_timeline_now(event: &TiffanyProgressEvent) -> bool {
    matches!(event.status.as_str(), "failed" | "warning")
        || (event.role == "orchestrator" && event.status == "done")
}

fn timeline_event_lines(events: &[TiffanyProgressEvent]) -> Vec<Line<'static>> {
    if events.len() == 1 {
        return event_timeline_detail_lines(&events[0]);
    }

    let mut lines = vec![timeline_header_line(events)];
    for event in events {
        for line in event_timeline_detail_lines(event) {
            lines.push(timeline_body_line(line));
        }
    }
    lines
}

fn event_timeline_detail_lines(event: &TiffanyProgressEvent) -> Vec<Line<'static>> {
    let mut lines = vec![waterfall_status_line(event)];
    lines.extend(event_detail_lines(event));
    lines
}

fn timeline_header_line(events: &[TiffanyProgressEvent]) -> Line<'static> {
    let (symbol, color) = timeline_header_symbol(events);
    let mut spans = vec![
        Span::styled(
            symbol,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "flow",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("run timeline", Style::default()),
    ];
    if let Some(flow) = timeline_flow_label(events) {
        spans.push(Span::styled(" · ", Style::default().fg(TIFFANY_DARK)));
        spans.push(Span::styled(flow, Style::default().fg(TIFFANY_SOFT)));
    }
    spans.push(Span::styled(" · ", Style::default().fg(TIFFANY_DARK)));
    spans.push(Span::styled(
        format!("{} updates", events.len()),
        Style::default().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn timeline_header_symbol(events: &[TiffanyProgressEvent]) -> (&'static str, Color) {
    if events.iter().any(|event| event.status == "failed") {
        return ("✗", Color::Red);
    }
    if events.iter().any(|event| event.status == "warning") {
        return ("⚠", Color::Yellow);
    }
    if events
        .last()
        .is_some_and(|event| matches!(event.status.as_str(), "done" | "ready" | "skipped"))
    {
        return ("✓", TIFFANY_BLUE);
    }
    ("●", TIFFANY_BLUE)
}

fn timeline_flow_label(events: &[TiffanyProgressEvent]) -> Option<String> {
    for event in events {
        if let Some(flow) = event.flow_steps.as_deref().and_then(nonempty_trimmed) {
            return Some(flow.to_string());
        }
    }
    None
}

fn timeline_body_line(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::styled("  ", Style::default().fg(TIFFANY_DARK))];
    spans.extend(line.spans);
    Line::from(spans)
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
    if let Some(raw) = event.content.as_deref()
        && let Some(hint) = event_format::agent_failure_hint(raw, CONTROL_SUMMARY_MAX_CHARS)
    {
        lines.push(failure_hint_line(hint.title(), hint.action()));
    }
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
        "ready" => "✓",
        "done" => "✓",
        "skipped" => "✓",
        "failed" => "✗",
        "warning" => "⚠",
        "output" => "↳",
        _ => "●",
    }
}

fn status_color(event: &TiffanyProgressEvent) -> Color {
    match event.status.as_str() {
        "ready" => TIFFANY_BLUE,
        "done" => TIFFANY_BLUE,
        "skipped" => TIFFANY_BLUE,
        "failed" => Color::Red,
        "warning" => Color::Yellow,
        "output" => Color::Gray,
        _ => TIFFANY_BLUE,
    }
}

fn waterfall_status_line(event: &TiffanyProgressEvent) -> Line<'static> {
    if event.role == "worker" && (event.task_id.is_some() || event.status == "ready") {
        return worker_status_line(event);
    }
    if event.role == "orchestrator" && event.route.is_some() {
        return route_status_line(event);
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

fn route_status_line(event: &TiffanyProgressEvent) -> Line<'static> {
    let color = status_color(event);
    Line::from(vec![
        Span::styled(
            status_symbol(event),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "route",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(route_lifecycle_title(event), Style::default()),
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
    let title = if event.status == "ready" && event.task_id.is_none() {
        worker_ready_title(event)
    } else {
        worker_lifecycle_title(event)
    };
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
        Span::styled(title, Style::default()),
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
    if event.role == "orchestrator" && event.route.is_some() {
        return route_lifecycle_title(event);
    }
    if event.role == "worker" && event.status == "ready" && event.task_id.is_none() {
        return worker_ready_title(event);
    }
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

fn worker_ready_title(event: &TiffanyProgressEvent) -> String {
    let mut parts = vec!["ready".to_string()];
    if let Some(count) = event.count {
        parts.push(format!("{count} run(s)"));
    }
    if let Some(route) = event
        .route_label
        .as_deref()
        .and_then(nonempty_trimmed)
        .or_else(|| event.route.as_deref().and_then(nonempty_trimmed))
    {
        parts.push(route.to_string());
    }
    if let Some(flow) = event.flow_steps.as_deref().and_then(nonempty_trimmed) {
        parts.push(flow.to_string());
    }
    parts.join(" · ")
}

fn route_lifecycle_title(event: &TiffanyProgressEvent) -> String {
    let route = event
        .route_label
        .as_deref()
        .and_then(nonempty_trimmed)
        .or_else(|| event.route.as_deref().and_then(nonempty_trimmed))
        .unwrap_or("selected");
    let action = if event.message.contains("updated") {
        "updated"
    } else {
        "selected"
    };
    let mut parts = vec![format!("{action} · {route}")];
    if let Some(reason) = event
        .route_reason_label
        .as_deref()
        .and_then(nonempty_trimmed)
        .or_else(|| event.reason.as_deref().and_then(nonempty_trimmed))
    {
        parts.push(reason.to_string());
    }
    if let Some(flow) = event.flow_steps.as_deref().and_then(nonempty_trimmed) {
        parts.push(flow.to_string());
    }
    parts.join(" · ")
}

fn output_title(event: &TiffanyProgressEvent) -> String {
    let suffix = match event.role.as_str() {
        "planner" => "plan",
        "critic" => "critique",
        "reviewer" => "review",
        "worker" => worker_output_suffix(event),
        _ => "details",
    };
    if event.role == "worker" {
        let label = output_label(event);
        format!("worker {suffix} · {label}")
    } else {
        format!("{} · {suffix}", stage_label(event))
    }
}

fn worker_output_suffix(event: &TiffanyProgressEvent) -> &'static str {
    let Some(raw) = event.content.as_deref() else {
        return "output";
    };
    if looks_like_native_session_recovery(raw) {
        return "session recovery";
    }
    match event_format::visible_agent_output(raw, CONTROL_SUMMARY_MAX_CHARS).map(|view| view.kind) {
        _ if event_format::runtime_output_kind(raw).is_some_and(|kind| kind == "diff") => "diff",
        _ if event_format::runtime_output_kind(raw).is_some_and(|kind| kind == "patch") => "patch",
        _ if event_format::runtime_output_kind(raw)
            .is_some_and(|kind| matches!(kind, "file_change" | "file_update")) =>
        {
            "file update"
        }
        Some(event_format::VisibleAgentOutputKind::Final) => "final",
        Some(event_format::VisibleAgentOutputKind::Question) => "question",
        Some(event_format::VisibleAgentOutputKind::ToolCall) => "tool call",
        Some(event_format::VisibleAgentOutputKind::ToolResult) => "tool result",
        Some(event_format::VisibleAgentOutputKind::Stderr) => "stderr",
        Some(event_format::VisibleAgentOutputKind::Actionable) => "alert",
        Some(event_format::VisibleAgentOutputKind::Normal) | None => "output",
    }
}

fn looks_like_native_session_recovery(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("native session busy")
        && lower.contains("cleared saved session")
        && lower.contains("retrying once")
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
        lines.extend(labeled_detail_block("task", &visible_task_prompt(prompt)));
    }
    lines
}

fn visible_task_prompt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let request = event_format::current_user_request(trimmed);
    if request != trimmed {
        return request.trim().to_string();
    }

    for marker in ["User message:", "Current request:", "Task:"] {
        if let Some(idx) = trimmed.rfind(marker) {
            let visible = trimmed[idx + marker.len()..].trim();
            if !visible.is_empty() {
                return visible.to_string();
            }
        }
    }

    trimmed.to_string()
}

fn reviewer_lifecycle_title(event: &TiffanyProgressEvent) -> String {
    let action = match event.status.as_str() {
        "running" => "checking worker output".to_string(),
        "done" if event.approved == Some(true) => "passed".to_string(),
        "skipped" => "skipped".to_string(),
        "warning" if event.approved == Some(false) => "needs fixes".to_string(),
        "warning" if event.message.starts_with("review unavailable") => "unavailable".to_string(),
        _ => normalize_event_message(&event.message),
    };
    let mut parts = vec![action];
    if let Some(id) = short_task_id(event.task_id.as_deref()) {
        parts.push(id.to_string());
    }
    if event.status == "skipped"
        && let Some(reason) = event.reason.as_deref().and_then(nonempty_trimmed)
    {
        parts.push(reason.to_string());
    }
    if matches!(event.status.as_str(), "warning" | "failed")
        && let Some(issues) = event.issues
    {
        parts.push(format!("{issues} issue(s)"));
    }
    if event.status == "warning"
        && event.message.starts_with("review unavailable")
        && let Some(reason) = event.reason.as_deref().and_then(nonempty_trimmed)
    {
        parts.push(reason.to_string());
    }
    parts.join(" · ")
}

fn worker_lifecycle_title(event: &TiffanyProgressEvent) -> String {
    let role = worker_role_label(event);
    let action = match event.status.as_str() {
        "ready" if event.worker_thread_id.is_some() && event.reused == Some(true) => {
            "thread reused"
        }
        "ready" if event.worker_thread_id.is_some() => "thread created",
        "ready" => "ready",
        "running" if event.worker_thread_id.is_some() && event.reused == Some(true) => {
            "started with reused session"
        }
        "running" if event.worker_thread_id.is_some() => "started with new session",
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
    if let Some(agent) = event.cc_agent.as_deref().and_then(nonempty_trimmed) {
        parts.push(format!("agent {agent}"));
    }
    if let Some(model) = provider_model_label(event) {
        parts.push(model);
    }
    if let Some(thread_id) = event.worker_thread_id.as_deref().and_then(nonempty_trimmed) {
        parts.push(format!(
            "thread {}",
            short_task_id(Some(thread_id)).unwrap_or(thread_id)
        ));
    }
    if let Some(native) = event
        .native_session_id
        .as_deref()
        .and_then(nonempty_trimmed)
    {
        parts.push(format!(
            "native {}",
            short_task_id(Some(native)).unwrap_or(native)
        ));
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
    event_format::humanize_user_visible_text(
        &message.replace(" - ", " · "),
        CONTROL_SUMMARY_MAX_CHARS,
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

    let display = if event.role == "worker" {
        event_format::visible_agent_output(content, visible_content_max(event))
            .map(|view| view.display)?
    } else {
        event_format::clean_visible_agent_output(content, visible_content_max(event))?
    };
    let display = strip_redundant_role_prefix(&event.role, &display);
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
    event_format::final_output_candidate(content, FULL_MESSAGE_STREAM_MAX_CHARS)
        .filter(|result| !result.trim().is_empty())
}

fn final_output_from_event(event: &TiffanyProgressEvent, visible: &str) -> Option<String> {
    if event.role != "worker" || event.status != "output" {
        return None;
    }
    if event
        .content
        .as_deref()
        .is_some_and(looks_like_native_session_recovery)
    {
        return None;
    }
    if let Some(result) = event.content.as_deref().and_then(final_output_candidate) {
        return Some(result);
    }
    let raw = event.content.as_deref()?;
    if event_format::runtime_output_kind(raw).is_some_and(|kind| kind == "assistant") {
        let cleaned = event_format::strip_known_final_heading(visible);
        if !cleaned.trim().is_empty() {
            return Some(cleaned);
        }
    }
    None
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
    event_format::normalized_output_key(content, CONTROL_SUMMARY_MAX_CHARS)
        .unwrap_or_else(|| content.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn visible_content_max(event: &TiffanyProgressEvent) -> usize {
    if event.role == "worker" {
        FULL_MESSAGE_STREAM_MAX_CHARS
    } else {
        CONTROL_SUMMARY_MAX_CHARS
    }
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

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn status_glyph(status: &str) -> &'static str {
    match status {
        "ready" | "done" | "skipped" => "✓",
        "failed" => "✗",
        "warning" => "⚠",
        "output" => "↳",
        _ => "●",
    }
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

fn push_thread_meta(lines: &mut Vec<Line<'static>>, label: &str, value: Option<&String>) {
    if let Some(value) = value.map(String::as_str).and_then(nonempty_trimmed) {
        lines.push(thread_meta_line(label, value));
    }
}

fn thread_meta_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(value.to_string(), Style::default().fg(Color::DarkGray)),
    ])
}

fn labeled_detail_block(label: &str, value: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (idx, line) in value.trim().lines().enumerate() {
        let line = line.trim_end();
        if idx == 0 {
            lines.push(meta_line(label, line));
        } else {
            lines.push(detail_continuation_line(line));
        }
    }
    lines
}

fn detail_continuation_line(line: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  │ ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
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

fn next_line_into(lines: &mut Vec<Line<'static>>, command: &str, detail: &str) {
    lines.push(next_line(command, detail));
}

fn output_body_line(line: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  │ ", Style::default().fg(TIFFANY_DARK)),
        Span::raw(line.to_string()),
    ])
}

fn failure_hint_line(title: &str, action: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "  fix ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(title.to_string(), Style::default().fg(Color::Yellow)),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(action.to_string(), Style::default().fg(Color::DarkGray)),
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

    const WORKER_TASK_ID: &str = "12345678-0000-0000-0000-000000000000";
    fn test_event(role: &str, status: &str, message: &str) -> TiffanyProgressEvent {
        TiffanyProgressEvent {
            role: role.to_string(),
            status: status.to_string(),
            message: message.to_string(),
            ..TiffanyProgressEvent::default()
        }
    }

    fn worker_event(status: &str, message: &str) -> TiffanyProgressEvent {
        TiffanyProgressEvent {
            task_id: Some(WORKER_TASK_ID.to_string()),
            ..test_event("worker", status, message)
        }
    }

    fn worker_output_event(message: &str, agent: &str, content: &str) -> TiffanyProgressEvent {
        TiffanyProgressEvent {
            agent: Some(agent.to_string()),
            content: Some(content.to_string()),
            ..worker_event("output", message)
        }
    }

    fn role_output_event(role: &str, content: &str) -> TiffanyProgressEvent {
        TiffanyProgressEvent {
            content: Some(content.to_string()),
            ..test_event(role, "output", &format!("{role} output"))
        }
    }

    #[test]
    fn intro_lines_have_tiffany_blue_structure() {
        let lines = idle_intro_lines_with_readiness(2, None);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("◆ T>_  tiffany-loop orchestrator"));
        assert!(text.contains("status ready  mode native  memory on  context 2 turn(s)"));
        assert!(text.contains("flow  direct / single / full  →  selected per prompt"));
        assert!(!text.contains("status ready\nflow  planner → critic"));
        assert!(text.contains("setup  /provider  /role  /roles  /thread  /history  /doctor"));
        assert!(text.contains("tools  /status  /copy  /raw  /diff  /clear  /exit"));
        assert_eq!(lines[0].spans[0].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[0].spans[2].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[0].spans[3].style.fg, Some(TIFFANY_SOFT));
        assert_eq!(lines[2].spans[0].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(lines[3].spans[2].style.fg, Some(TIFFANY_SOFT));
        assert_eq!(lines[4].spans[2].style.fg, Some(TIFFANY_SOFT));
    }

    #[test]
    fn intro_lines_show_missing_startup_readiness() {
        let missing = std::env::temp_dir().join(format!(
            "definitely-missing-tiffany-config-{}",
            std::process::id()
        ));
        let config = TiffanyOrchestratorConfig {
            bin: "definitely-missing-orchestrator".to_string(),
            extra_args: Vec::new(),
            config_path: Some(missing.display().to_string()),
        };
        let readiness = startup_readiness(Some(&config));

        let lines = idle_intro_lines_with_readiness(0, Some(&readiness));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("runtime  missing  definitely-missing-orchestrator"));
        assert!(text.contains("config  missing"));
        assert!(text.contains("next  /provider  /role then /doctor"));
        assert!(text.contains("flow  direct / single / full"));
    }

    #[test]
    fn intro_lines_show_role_runtime_readiness() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join("config.yaml");
        let runtime = temp.path().join(executable_name("claude"));
        std::fs::write(&runtime, "")?;
        make_launchable(&runtime)?;
        std::fs::write(
            &config_path,
            format!(
                "providers:\n  minimax:\n    type: openai\nruntimes:\n  claude-code:\n    type: subprocess\n    binary: {}\nmodels:\n  - id: sonnet\n    provider: minimax\n    name: MiniMax-M3\nroles:\n  worker-cc:\n    model: sonnet\n    runtime: claude-code\nbehavior: {{}}\n",
                runtime.display()
            ),
        )?;
        let config = TiffanyOrchestratorConfig {
            bin: "orchestrator".to_string(),
            extra_args: Vec::new(),
            config_path: Some(config_path.display().to_string()),
        };
        let readiness = startup_readiness(Some(&config));

        let lines = idle_intro_lines_with_readiness(1, Some(&readiness));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("config  ok  1 providers · 1 models · 1 roles · 1 runtimes"));
        assert!(text.contains("worker  ready  worker-cc · claude-code"));
        Ok(())
    }

    #[test]
    fn intro_lines_show_worker_runtime_missing() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  minimax:\n    type: openai\nruntimes:\n  codex:\n    type: subprocess\n    binary: definitely-missing-codex\nmodels:\n  - id: minimax-m3-codex\n    provider: minimax\n    name: MiniMax-M3\nroles:\n  worker-codex:\n    model: minimax-m3-codex\n    runtime: codex\nbehavior: {}\n",
        )?;
        let config = TiffanyOrchestratorConfig {
            bin: "orchestrator".to_string(),
            extra_args: Vec::new(),
            config_path: Some(config_path.display().to_string()),
        };
        let readiness = startup_readiness(Some(&config));

        let lines = idle_intro_lines_with_readiness(0, Some(&readiness));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(
            text.contains(
                "worker  runtime-missing  worker-codex · codex · definitely-missing-codex"
            )
        );
        Ok(())
    }

    #[test]
    fn intro_lines_show_gemini_worker_runtime_readiness() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join("config.yaml");
        let runtime = temp.path().join(executable_name("gemini"));
        std::fs::write(&runtime, "")?;
        make_launchable(&runtime)?;
        std::fs::write(
            &config_path,
            format!(
                "providers:\n  google:\n    type: google\nruntimes:\n  gemini:\n    type: subprocess\n    binary: {}\nmodels:\n  - id: gemini-pro\n    provider: google\n    name: gemini-2.5-pro\nroles:\n  worker-gemini:\n    model: gemini-pro\n    runtime: gemini\nbehavior: {{}}\n",
                runtime.display()
            ),
        )?;
        let config = TiffanyOrchestratorConfig {
            bin: "orchestrator".to_string(),
            extra_args: Vec::new(),
            config_path: Some(config_path.display().to_string()),
        };
        let readiness = startup_readiness(Some(&config));

        let lines = idle_intro_lines_with_readiness(0, Some(&readiness));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker  ready  worker-gemini · gemini"));
        Ok(())
    }

    #[test]
    fn intro_lines_show_worker_provider_missing() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers: {}\nruntimes:\n  codex:\n    type: subprocess\nmodels:\n  - id: minimax-m3-codex\n    provider: minimax\n    name: MiniMax-M3\nroles:\n  worker-codex:\n    model: minimax-m3-codex\n    runtime: codex\nbehavior: {}\n",
        )?;
        let config = TiffanyOrchestratorConfig {
            bin: "orchestrator".to_string(),
            extra_args: Vec::new(),
            config_path: Some(config_path.display().to_string()),
        };
        let readiness = startup_readiness(Some(&config));

        let lines = idle_intro_lines_with_readiness(0, Some(&readiness));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker  provider-missing  worker-codex · codex · codex"));
        Ok(())
    }

    #[test]
    fn orchestrator_memory_round_trips_recent_turns_by_cwd() {
        let home = tempfile::tempdir().expect("home");
        let cwd_a = tempfile::tempdir().expect("cwd a");
        let cwd_b = tempfile::tempdir().expect("cwd b");
        let mut turns = VecDeque::new();
        for idx in 0..10 {
            turns.push_back(TiffanyOrchestratorTurn {
                user_prompt: format!("prompt {idx}"),
                result: format!("result {idx}"),
            });
        }

        save_memory_turns(home.path(), cwd_a.path(), &turns).expect("save cwd a");
        let loaded_a = load_memory_turns(home.path(), cwd_a.path()).expect("load cwd a");
        let loaded_b = load_memory_turns(home.path(), cwd_b.path()).expect("load cwd b");

        assert_eq!(loaded_a.len(), MEMORY_MAX_TURNS);
        assert_eq!(loaded_a.front().unwrap().user_prompt, "prompt 2");
        assert_eq!(loaded_a.back().unwrap().result, "result 9");
        assert!(loaded_b.is_empty());

        let mut cwd_b_turns = VecDeque::new();
        cwd_b_turns.push_back(TiffanyOrchestratorTurn {
            user_prompt: "other prompt".into(),
            result: "other result".into(),
        });
        save_memory_turns(home.path(), cwd_b.path(), &cwd_b_turns).expect("save cwd b");

        assert_eq!(
            load_memory_turns(home.path(), cwd_a.path())
                .expect("reload cwd a")
                .len(),
            MEMORY_MAX_TURNS
        );
        assert_eq!(
            load_memory_turns(home.path(), cwd_b.path())
                .expect("reload cwd b")
                .front()
                .unwrap()
                .result,
            "other result"
        );
    }

    #[test]
    fn orchestrator_memory_ignores_empty_turns() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let mut turns = VecDeque::new();
        turns.push_back(TiffanyOrchestratorTurn {
            user_prompt: " ".into(),
            result: "ignored".into(),
        });
        turns.push_back(TiffanyOrchestratorTurn {
            user_prompt: "kept".into(),
            result: " done ".into(),
        });

        save_memory_turns(home.path(), cwd.path(), &turns).expect("save");
        let loaded = load_memory_turns(home.path(), cwd.path()).expect("load");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.front().unwrap().user_prompt, "kept");
        assert_eq!(loaded.front().unwrap().result, "done");
    }

    #[test]
    fn native_chat_sessions_append_full_turn_history_by_cwd() {
        let home = tempfile::tempdir().expect("home");
        let cwd_a = tempfile::tempdir().expect("cwd a");
        let cwd_b = tempfile::tempdir().expect("cwd b");

        for idx in 0..(MEMORY_MAX_TURNS + 3) {
            append_native_chat_turn(
                home.path(),
                cwd_a.path(),
                &TiffanyOrchestratorTurn {
                    user_prompt: format!("prompt {idx}"),
                    result: format!("result {idx}"),
                },
                Vec::new(),
            )
            .expect("append cwd a");
        }
        append_native_chat_turn(
            home.path(),
            cwd_b.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "other prompt".into(),
                result: "other result".into(),
            },
            Vec::new(),
        )
        .expect("append cwd b");

        let loaded_a = load_native_chat_conversation(home.path(), cwd_a.path())
            .expect("load cwd a")
            .expect("cwd a conversation");
        let loaded_b = load_native_chat_conversation(home.path(), cwd_b.path())
            .expect("load cwd b")
            .expect("cwd b conversation");

        assert_eq!(loaded_a.turns.len(), MEMORY_MAX_TURNS + 3);
        assert_eq!(loaded_a.turns.first().unwrap().user_prompt, "prompt 0");
        assert_eq!(
            loaded_a.turns.last().unwrap().result,
            format!("result {}", MEMORY_MAX_TURNS + 2)
        );
        assert_eq!(loaded_b.turns.len(), 1);
        assert_eq!(loaded_b.turns[0].result, "other result");
        assert_ne!(loaded_a.id, loaded_b.id);

        let memory_turns =
            load_native_memory_turns(home.path(), cwd_a.path()).expect("native memory turns");
        assert_eq!(memory_turns.len(), MEMORY_MAX_TURNS);
        assert_eq!(memory_turns.front().unwrap().user_prompt, "prompt 3");
        assert_eq!(
            memory_turns.back().unwrap().result,
            format!("result {}", MEMORY_MAX_TURNS + 2)
        );
    }

    #[test]
    fn native_chat_sessions_ignore_empty_turns() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: " ".into(),
                result: "ignored".into(),
            },
            Vec::new(),
        )
        .expect("append empty");

        let loaded = load_native_chat_conversation(home.path(), cwd.path()).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn native_chat_sessions_persist_visible_native_events() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "change a file".into(),
                result: "changed".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker diff · worker-cc · claude-code · abc12345".into(),
                content: Some("files changed:\n  - src/lib.rs\n\ndiff --git a/src/lib.rs b/src/lib.rs".into()),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: Some("claude-sonnet-4-6".into()),
                provider: Some("anthropic".into()),
                task_id: Some("abc12345".into()),
                native_session_id: Some("native-1".into()),
            }],
        )
        .expect("append native event");

        let loaded = load_native_chat_conversation(home.path(), cwd.path())
            .expect("load")
            .expect("conversation");
        let events = &loaded.turns[0].events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "worker diff · worker-cc · claude-code · abc12345");
        assert!(events[0].content.as_deref().unwrap().contains("diff --git"));
        assert_eq!(events[0].agent.as_deref(), Some("claude-code"));
        assert_eq!(events[0].native_session_id.as_deref(), Some("native-1"));
    }

    #[test]
    fn native_history_lines_show_saved_turns_and_events() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "改一下 README".into(),
                result: "已更新 README".into(),
            },
            vec![
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker diff · worker-cc · claude-code · abc12345".into(),
                    content: Some(
                        "files changed:\n  - README.md\n\ndiff --git a/README.md b/README.md"
                            .into(),
                    ),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("abc12345".into()),
                    native_session_id: Some("native-1".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "done".into(),
                    title: "worker-cc done · claude-code · 1s".into(),
                    content: None,
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: None,
                    provider: None,
                    task_id: Some("abc12345".into()),
                    native_session_id: Some("native-1".into()),
                },
            ],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  1 turn(s) saved"));
        assert!(text.contains("user  改一下 README"));
        assert!(text.contains("answer 已更新 README"));
        assert!(text.contains("worker diff"));
        assert!(text.contains("diff --git a/README.md b/README.md"));
        assert!(text.contains("/history full"));
        assert!(text.contains("/thread <role>"));
    }

    #[test]
    fn native_history_export_writes_markdown() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let out = home.path().join("handoff.md");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "写测试".into(),
                result: "测试已通过".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker tool result · worker-cc · claude-code".into(),
                content: Some("cargo test -q\n\nok".into()),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: Some("claude-sonnet-4-6".into()),
                provider: Some("anthropic".into()),
                task_id: Some("abc12345".into()),
                native_session_id: Some("native-1".into()),
            }],
        )
        .expect("append");

        let lines = native_history_lines(
            home.path(),
            cwd.path(),
            &format!("export --out {}", out.display()),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("history  exported 1 turn(s)"));
        assert!(text.contains(out.to_string_lossy().as_ref()));

        let markdown = std::fs::read_to_string(&out).expect("export body");
        assert!(markdown.contains("# Tiffany Native Conversation History"));
        assert!(markdown.contains("## Turn 1"));
        assert!(markdown.contains("写测试"));
        assert!(markdown.contains("测试已通过"));
        assert!(markdown.contains("worker tool result"));
        assert!(markdown.contains("```text\ncargo test -q"));
    }

    #[test]
    fn native_history_lines_report_empty_and_bad_args() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        let empty = native_history_lines(home.path(), cwd.path(), "");
        let empty_text = empty.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(empty_text.contains("no Tiffany native conversation saved"));

        let bad = native_history_lines(home.path(), cwd.path(), "wat");
        let bad_text = bad.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(bad_text.contains("unknown /history option 'wat'"));
        assert!(bad_text.contains("Usage: /history [last|full|<count>|export [--out path]]"));
    }

    #[test]
    fn thread_command_args_maps_native_tui_shortcuts() {
        assert_eq!(
            thread_command_args("").unwrap(),
            strings(&["thread", "list"])
        );
        assert_eq!(
            thread_command_args("list").unwrap(),
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
            thread_command_args("clear worker-cc").unwrap(),
            strings(&["thread", "clear", "worker-cc"])
        );
        assert_eq!(
            thread_command_args("fresh worker-cc").unwrap(),
            strings(&["thread", "clear", "worker-cc"])
        );
        assert_eq!(
            thread_command_args("export worker-cc").unwrap(),
            strings(&["thread", "export", "worker-cc"])
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
                "/tmp/session.html"
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
    fn run_intro_uses_direct_flow_for_chat_requests() {
        let launch = TiffanyOrchestratorLaunch {
            bin: "orchestrator".into(),
            user_prompt: "你好".into(),
            prompt: "你好".into(),
            extra_args: vec![],
            config_path: None,
            context_turn_count: 0,
        };
        let lines = [
            run_status_line(&launch),
            run_reason_line(&launch),
            run_workflow_line(&launch),
        ];
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("flow direct"));
        assert!(text.contains("reason  simple conversational or explanatory request"));
        assert!(text.contains("flow  direct → worker → answer"));
        assert!(!text.contains("planner → critic"));
    }

    #[test]
    fn run_intro_uses_single_flow_for_link_inspection() {
        let launch = TiffanyOrchestratorLaunch {
            bin: "orchestrator".into(),
            user_prompt: "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle".into(),
            prompt: "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle".into(),
            extra_args: vec![],
            config_path: None,
            context_turn_count: 0,
        };
        let lines = [
            run_status_line(&launch),
            run_reason_line(&launch),
            run_workflow_line(&launch),
        ];
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("flow single"));
        assert!(
            text.contains("reason  atomic request; planner, critic, and reviewer are not needed")
        );
        assert!(text.contains("flow  single → worker → answer"));
        assert!(!text.contains("planner → critic"));
    }

    #[test]
    fn run_intro_uses_single_flow_for_atomic_worker_requests() {
        let launch = TiffanyOrchestratorLaunch {
            bin: "orchestrator".into(),
            user_prompt: "写参赛 agent".into(),
            prompt: "写参赛 agent".into(),
            extra_args: vec![],
            config_path: None,
            context_turn_count: 0,
        };
        let lines = [
            run_status_line(&launch),
            run_reason_line(&launch),
            run_workflow_line(&launch),
        ];
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("flow single"));
        assert!(
            text.contains("reason  atomic request; planner, critic, and reviewer are not needed")
        );
        assert!(text.contains("flow  single → worker → answer"));
        assert!(!text.contains("planner → critic"));
    }

    #[test]
    fn run_intro_uses_single_flow_for_external_atomic_followups() {
        let prompt = "Previous turns:\nuser:\n优化 tiffany-loop 编排流程\n\nassistant result:\n已完成。\n\n---\nCurrent user request:\n写参赛 agent";
        let launch = TiffanyOrchestratorLaunch {
            bin: "orchestrator".into(),
            user_prompt: prompt.into(),
            prompt: prompt.into(),
            extra_args: vec![],
            config_path: None,
            context_turn_count: 1,
        };
        let lines = [
            run_status_line(&launch),
            run_reason_line(&launch),
            run_workflow_line(&launch),
        ];
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("flow single"));
        assert!(
            text.contains("reason  atomic request; planner, critic, and reviewer are not needed")
        );
        assert!(text.contains("context 1 turn(s)"));
        assert!(text.contains("flow  single → worker → answer"));
    }

    #[test]
    fn run_intro_uses_full_flow_for_engineering_requests() {
        let launch = TiffanyOrchestratorLaunch {
            bin: "orchestrator".into(),
            user_prompt: "优化 TUI 显示".into(),
            prompt: "优化 TUI 显示".into(),
            extra_args: vec![],
            config_path: None,
            context_turn_count: 2,
        };
        let lines = [
            run_status_line(&launch),
            run_reason_line(&launch),
            run_workflow_line(&launch),
        ];
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("flow full"));
        assert!(text.contains(
            "reason  project or implementation work; planner, critic, worker, and reviewer will run"
        ));
        assert!(text.contains("context 2 turn(s)"));
        assert!(text.contains("flow  planner → critic → worker → reviewer"));
    }

    #[test]
    fn run_intro_uses_contextual_route_for_continuations() {
        let prompt = "Previous turns:\nuser:\n优化 tiffany-loop 编排流程\n\nassistant result:\n已完成。\n\n---\nCurrent user request:\n继续";
        let launch = TiffanyOrchestratorLaunch {
            bin: "orchestrator".into(),
            user_prompt: "继续".into(),
            prompt: prompt.into(),
            extra_args: vec![],
            config_path: None,
            context_turn_count: 1,
        };
        let lines = [
            run_status_line(&launch),
            run_reason_line(&launch),
            run_workflow_line(&launch),
        ];
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("flow full"));
        assert!(text.contains("context 1 turn(s)"));
        assert!(text.contains("flow  planner → critic → worker → reviewer"));
        assert!(!text.contains("flow single"));
    }

    #[test]
    fn shared_direct_classifier_ignores_previous_context_noise() {
        let contextual = "Previous turns:\nuser:\n优化 TUI 显示\n\nassistant result:\n已提交。\n\n---\nCurrent user request:\n你叫啥";

        assert!(event_format::request_looks_direct_answer(contextual));
        assert_eq!(event_format::current_user_request(contextual), "你叫啥");
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
    fn roles_command_args_accept_profile_save() {
        assert_eq!(
            roles_command_args("profile dev --planner sonnet@claude-code --worker-cc minimax/MiniMax-M3@claude-code")
                .unwrap(),
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
    fn role_profile_summary_lines_render_profile_rows() {
        let rows = parse_role_profile_rows(
            "Role profile saved: dev\n  ✓ planner       model=sonnet runtime=claude-code agent_teams=false\n  ✓ worker-cc     model=minimax-m3 runtime=claude-code agent_teams=true\n\nNext: orchestrator roles list\n",
        );
        let lines = rows.iter().map(role_profile_row_line).collect::<Vec<_>>();
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(
            parse_prefixed_field("Role profile saved: dev", "Role profile saved:"),
            Some("dev")
        );
        assert!(text.contains("✓ planner      sonnet · claude-code · teams false"));
        assert!(text.contains("✓ worker-cc    minimax-m3 · claude-code · teams true"));
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
             Register: orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime>\n",
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
        assert!(text.contains("edit /role worker-codex"));
        assert!(text.contains("thread /thread worker-codex"));
        assert!(text.contains("profile /roles profile"));
        assert!(text.contains("next  /doctor  verify the orchestration chain"));
        assert!(!text.contains("Registered roles:"));
        assert!(!text.contains("Register: orchestrator"));
    }

    #[test]
    fn failure_lines_suggest_codex_exec_compatibility_fix() {
        use std::os::unix::process::ExitStatusExt;

        let lines = orchestrator_failure_lines(
            ExitStatus::from_raw(1 << 8),
            "codex stderr: error: unexpected argument '--cwd' found\n\
             Usage: codex exec --cd <DIR> [PROMPT]",
            "",
            &[],
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("unexpected argument '--cwd'"));
        assert!(text.contains("upgrade Codex CLI"));
        assert!(text.contains("codex exec --cd"));
        assert!(text.contains("/doctor"));
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
    fn thread_list_summary_lines_render_role_sessions_and_actions() {
        let lines = thread_list_summary_lines(
            "Worker threads\n\
               stored threads: 1\n\
             \n\
             Roles:\n\
               ○ worker-cc          claude-code · sonnet · no worker thread yet\n\
               ● worker-codex       codex · openai/gpt-4o · thread 00000000 · native codex-native-session · last 11111111\n\
             \n\
             Details: orchestrator thread show <role>  Fresh start: orchestrator thread clear <role>\n",
        )
        .expect("thread list summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ thread  1/2 role(s) have worker sessions"));
        assert!(text.contains("worker-cc"));
        assert!(text.contains("no worker thread yet"));
        assert!(text.contains("worker-codex"));
        assert!(text.contains("codex-native-session"));
        assert!(text.contains("last 11111111"));
        assert!(text.contains("inspect /thread worker-codex"));
        assert!(text.contains("export /thread export worker-codex"));
        assert!(text.contains("clear /thread clear worker-codex"));
        assert!(text.contains("run task to create session"));
        assert!(text.contains("next  /thread <role>"));
        assert!(text.contains("next  /thread clear <role>"));
        assert!(!text.contains("Worker threads"));
        assert!(!text.contains("orchestrator thread show"));
    }

    #[test]
    fn thread_detail_summary_lines_render_native_resume_command() {
        let lines = thread_detail_summary_lines(
            "Worker thread 00000000\n\
               role: worker-codex\n\
               runtime: codex\n\
               agent: codex\n\
               model: openai/gpt-4o\n\
               provider: openai\n\
               Tiffany thread: 00000000-0000-0000-0000-000000000123\n\
               native session: codex-native-session\n\
               native resume: codex exec resume codex-native-session\n\
               last Tiffany session: 00000000-0000-0000-0000-000000000456\n\
               worktree: /tmp/tiffany-worker\n\
             \n\
             Status: ready for native resume; Tiffany will reuse this session for the same role\n\
             Action: orchestrator thread clear worker-codex resets only the native CLI session id for a fresh next run.\n",
        )
        .expect("thread detail summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains(
            "✓ thread  worker-codex · ready for native resume; Tiffany will reuse this session for the same role"
        ));
        assert!(text.contains("runtime  codex"));
        assert!(text.contains("model  openai/gpt-4o"));
        assert!(text.contains("native  codex-native-session"));
        assert!(text.contains("resume  codex exec resume codex-native-session"));
        assert!(text.contains("last  00000000-0000-0000-0000-000000000456"));
        assert!(text.contains("next  /thread clear worker-codex"));
        assert!(!text.contains("Worker thread 00000000"));
        assert!(!text.contains("Action: orchestrator"));
    }

    #[test]
    fn thread_clear_summary_lines_render_fresh_next_run() {
        let lines = thread_clear_summary_lines(
            "Worker thread reset\n\
               role: worker-cc\n\
               Tiffany thread: 00000000-0000-0000-0000-000000000123\n\
               cleared native session: claude-native-session\n\
               next run: starts a fresh native claude-code session\n\
             \n\
             Kept: worker thread id, last Tiffany session, and conversation context.\n",
        )
        .expect("thread clear summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ thread  worker-cc native session cleared"));
        assert!(text.contains("cleared  claude-native-session"));
        assert!(text.contains("next  starts a fresh native claude-code session"));
        assert!(text.contains("kept Tiffany worker thread"));
        assert!(text.contains("next  /thread worker-cc"));
        assert!(!text.contains("Worker thread reset"));
        assert!(!text.contains("Kept: worker thread id"));
    }

    #[test]
    fn thread_export_summary_lines_render_export_target() {
        let lines = thread_export_summary_lines(
            "Worker thread session exported\n\
               role: worker-cc\n\
               Tiffany thread: 00000000-0000-0000-0000-000000000123\n\
               session: 11111111\n\
               target: /tmp/tiffany-session.md\n\
             \n\
             Action: open the export for full selectable history.\n",
        )
        .expect("thread export summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ thread  worker-cc session exported"));
        assert!(text.contains("thread  00000000-0000-0000-0000-000000000123"));
        assert!(text.contains("session  11111111"));
        assert!(text.contains("target  /tmp/tiffany-session.md"));
        assert!(text.contains("next  open export"));
        assert!(text.contains("next  /thread <role>"));
    }

    #[test]
    fn output_lines_use_tiffany_blue_source_and_gutter() {
        let event = worker_output_event("claude output", "claude-code", "done");

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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };
        let worker = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            cc_agent: None,
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };
        let reviewer = TiffanyProgressEvent {
            role: "reviewer".to_string(),
            status: "warning".to_string(),
            message: "review needs fixes - 2 issue(s)".to_string(),
            task_id: Some("87654321-0000-0000-0000-000000000000".to_string()),
            agent: None,
            worker_role: None,
            runtime: None,
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: None,
            approved: Some(false),
            issues: Some(2),
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
    fn timeline_groups_control_events_without_hiding_details() {
        let route = TiffanyProgressEvent {
            role: "orchestrator".to_string(),
            status: "running".to_string(),
            message: "route selected - direct-answer".to_string(),
            route: Some("direct-answer".to_string()),
            route_label: Some("direct".to_string()),
            route_reason_label: Some("chat/explain".to_string()),
            flow_steps: Some("worker -> answer".to_string()),
            ..TiffanyProgressEvent::default()
        };
        let worker = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some("anthropic".to_string()),
            task_prompt: Some("Current user request:\n你好".to_string()),
            ..TiffanyProgressEvent::default()
        };
        let lines = timeline_event_lines(&[route, worker]);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(
            line_text(&lines[0]),
            "● flow  run timeline · worker -> answer · 2 updates"
        );
        assert!(text.contains("  ● route  selected · direct · chat/explain · worker -> answer"));
        assert!(text.contains(
            "  ● worker  worker-cc started · claude-code · anthropic/claude-sonnet-4-6 · 12345678"
        ));
        assert!(text.contains("  task  你好"));
    }

    #[test]
    fn timeline_single_event_keeps_original_status_shape() {
        let event = TiffanyProgressEvent {
            role: "planner".to_string(),
            status: "running".to_string(),
            message: "planning".to_string(),
            ..TiffanyProgressEvent::default()
        };
        let lines = timeline_event_lines(&[event]);

        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "● step  01 planner  planning");
    }

    #[test]
    fn timeline_header_reflects_warning_or_failure() {
        let warning = TiffanyProgressEvent {
            role: "critic".to_string(),
            status: "warning".to_string(),
            message: "plan needs fixes - 2 issue(s)".to_string(),
            issues: Some(2),
            ..TiffanyProgressEvent::default()
        };
        let failed = TiffanyProgressEvent {
            role: "orchestrator".to_string(),
            status: "failed".to_string(),
            message: "planner returned no sub_tasks".to_string(),
            ..TiffanyProgressEvent::default()
        };

        assert!(
            line_text(
                &timeline_event_lines(&[test_event("planner", "running", "planning"), warning,])[0]
            )
            .starts_with("⚠ flow")
        );
        assert!(
            line_text(
                &timeline_event_lines(&[test_event("planner", "running", "planning"), failed,])[0]
            )
            .starts_with("✗ flow")
        );
    }

    #[test]
    fn timeline_flushes_immediately_for_warnings_failures_and_done() {
        assert!(!should_flush_timeline_now(&test_event(
            "planner", "running", "planning"
        )));
        assert!(should_flush_timeline_now(&test_event(
            "critic",
            "warning",
            "plan needs fixes"
        )));
        assert!(should_flush_timeline_now(&test_event(
            "orchestrator",
            "failed",
            "planner failed"
        )));
        assert!(should_flush_timeline_now(&test_event(
            "orchestrator",
            "done",
            "done"
        )));
    }

    #[test]
    fn route_status_line_uses_route_metadata_without_result_step_noise() {
        let selected = TiffanyProgressEvent {
            role: "orchestrator".to_string(),
            status: "running".to_string(),
            message: "route selected - direct-answer".to_string(),
            route: Some("direct-answer".to_string()),
            route_label: Some("direct".to_string()),
            route_reason_label: Some("chat/explain".to_string()),
            flow_steps: Some("worker -> answer".to_string()),
            ..TiffanyProgressEvent::default()
        };
        let updated = TiffanyProgressEvent {
            status: "warning".to_string(),
            message: "route updated - single-worker".to_string(),
            route: Some("single-worker".to_string()),
            route_label: Some("single".to_string()),
            route_reason_label: Some("atomic worker".to_string()),
            flow_steps: Some("worker -> answer".to_string()),
            ..selected.clone()
        };

        assert_eq!(
            line_text(&waterfall_status_line(&selected)),
            "● route  selected · direct · chat/explain · worker -> answer"
        );
        assert_eq!(
            line_text(&waterfall_status_line(&updated)),
            "⚠ route  updated · single · atomic worker · worker -> answer"
        );
        assert!(!line_text(&waterfall_status_line(&selected)).contains("05 result"));
    }

    #[test]
    fn worker_ready_lines_show_route_and_thread_reuse_metadata() {
        let route_ready = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "ready".to_string(),
            message: "worker ready - 1 run(s)".to_string(),
            count: Some(1),
            route: Some("single-worker".to_string()),
            route_label: Some("single".to_string()),
            flow_steps: Some("worker -> answer".to_string()),
            ..TiffanyProgressEvent::default()
        };
        let thread_ready = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "ready".to_string(),
            message: "worker-cc worker thread reused".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            worker_role: Some("worker-cc".to_string()),
            worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".to_string()),
            native_session_id: Some("5cee8032-e580-49f9-b0b8-95b0940c5692".to_string()),
            reused: Some(true),
            ..TiffanyProgressEvent::default()
        };

        assert_eq!(
            line_text(&waterfall_status_line(&route_ready)),
            "✓ worker  ready · 1 run(s) · single · worker -> answer"
        );
        assert_eq!(
            line_text(&waterfall_status_line(&thread_ready)),
            "✓ worker  worker-cc thread reused · thread abcdef12 · native 5cee8032 · 12345678"
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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

        let skipped = TiffanyProgressEvent {
            status: "skipped".to_string(),
            message: "review skipped - conversational answer".to_string(),
            reason: Some("conversational answer".to_string()),
            ..checking.clone()
        };
        assert_eq!(
            line_text(&waterfall_status_line(&skipped)),
            "✓ review  skipped · 12345678 · conversational answer"
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

        let unavailable = TiffanyProgressEvent {
            status: "warning".to_string(),
            message: "review unavailable - no JSON found in CLI response".to_string(),
            approved: None,
            issues: None,
            reason: Some("no JSON found in CLI response".to_string()),
            ..needs_fixes
        };
        assert_eq!(
            line_text(&waterfall_status_line(&unavailable)),
            "⚠ review  unavailable · 12345678 · no JSON found in CLI response"
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some(
                "critic>\n{\"approved\":false,\"issues\":[\"missing test\"]}".to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "running tests")
        };

        let lines = output_event_lines(&event, "running tests");

        assert_eq!(
            line_text(&lines[0]),
            "│ worker output · worker-cc · claude-code · 12345678"
        );
    }

    #[test]
    fn worker_output_titles_show_runtime_event_kind() {
        let base = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "output".to_string(),
            message: "worker output".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: None,
            cc_agent: None,
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some("claude-code assistant: 你好".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };

        assert_eq!(
            output_title(&base),
            "worker output · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
        );
        assert_eq!(
            output_title(&TiffanyProgressEvent {
                content: Some("claude-code result: 完成".to_string()),
                ..base.clone()
            }),
            "worker final · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
        );
        assert_eq!(
            output_title(&TiffanyProgressEvent {
                content: Some("claude-code tool_use: tool Bash: cargo test".to_string()),
                ..base.clone()
            }),
            "worker tool call · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
        );
        assert_eq!(
            output_title(&TiffanyProgressEvent {
                content: Some("claude-code tool_use: tool AskUserQuestion".to_string()),
                ..base.clone()
            }),
            "worker question · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
        );
        assert_eq!(
            output_title(&TiffanyProgressEvent {
                content: Some("claude-code stderr: [1211] 模型不存在".to_string()),
                ..base.clone()
            }),
            "worker stderr · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
        );
        assert_eq!(
            output_title(&TiffanyProgressEvent {
                content: Some(
                    "claude-code diff: files changed:\n  - src/lib.rs\n\ndiff --git a/src/lib.rs b/src/lib.rs"
                        .to_string()
                ),
                ..base
            }),
            "worker diff · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
        );
    }

    #[test]
    fn worker_error_output_adds_actionable_fix_line() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-codex".to_string()),
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            ..worker_output_event(
                "worker output",
                "worker-codex",
                "worker-codex stderr: API Error: 400 [1211][模型不存在]",
            )
        };

        let visible = visible_content(&event).expect("visible worker error");
        let lines = output_event_lines(&event, &visible);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker stderr · worker-codex · minimax/MiniMax-M3 · 12345678"));
        assert!(text.contains("fix model not found"));
        assert!(text.contains("Check the role's provider/model in /role"));
    }

    #[test]
    fn worker_native_session_recovery_output_is_labeled_and_not_captured_as_final() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some("anthropic".to_string()),
            worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".to_string()),
            native_session_id: Some("native-fresh".to_string()),
            reused: Some(true),
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code status: native session busy · cleared saved session native-busy and retrying once with a fresh native session",
            )
        };

        let visible = visible_content(&event).expect("visible recovery output");
        let lines = output_event_lines(&event, &visible);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains(
            "worker session recovery · worker-cc · claude-code · anthropic/claude-sonnet-4-6 · 12345678"
        ));
        assert!(text.contains("native session busy"));
        assert!(text.contains("retrying once with a fresh native session"));
        assert_eq!(final_output_from_event(&event, &visible), None);
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
            cc_agent: Some("reviewer".to_string()),
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: Some("do the work".to_string()),
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some("done".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: Some(1_250),
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };
        state.apply_worker_metadata(&mut done);
        assert_eq!(
            event_title(&done),
            "worker-cc done · claude-code · agent reviewer · minimax/MiniMax-M3 · 12345678 · 1.2s"
        );
        assert_eq!(
            state.worker_context_lines(),
            vec![
                "worker: worker-cc · claude-code · claude agent reviewer · minimax/MiniMax-M3 · 12345678"
            ]
        );
    }

    #[test]
    fn worker_started_lines_show_route_model_and_task() {
        let task_prompt = "回答用户的问题，并给出清晰结论。\n\
            保留上下文里的关键约束：不要输出原始 JSON，展示 worker 过程。\n\
            最后用中文总结。";
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            cc_agent: Some("reviewer".to_string()),
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: Some(task_prompt.to_string()),
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };

        let mut lines = vec![waterfall_status_line(&event)];
        lines.extend(event_detail_lines(&event));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker"));
        assert!(text.contains("worker-cc started"));
        assert!(text.contains("claude-code · agent reviewer · minimax/MiniMax-M3 · 12345678"));
        assert!(!text.contains("route "));
        assert!(text.contains("task  回答用户的问题"));
        assert!(text.contains("  │ 保留上下文里的关键约束"));
        assert!(text.contains("  │ 最后用中文总结。"));
        assert!(!text.contains('…'));
    }

    #[test]
    fn worker_started_task_hides_internal_context_prompt() {
        let task_prompt = "Answer the user's message directly in the same language.\n\
            Do not inspect repository state.\n\n\
            User message:\n\
            You are continuing a multi-turn tiffany-loop orchestrator conversation.\n\
            Use the previous turns to resolve follow-ups.\n\n\
            Previous turns:\n\
            user:\n\
            你好\n\n\
            assistant result:\n\
            你好！有什么我可以帮你的吗？\n\n\
            ---\n\
            Current user request:\n\
            你叫啥\n\
            你能干啥";
        let event = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            cc_agent: None,
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some("anthropic".to_string()),
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: Some(task_prompt.to_string()),
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };

        let details = event_detail_lines(&event);
        let text = details.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("task  你叫啥"));
        assert!(text.contains("  │ 你能干啥"));
        assert!(!text.contains("Previous turns"));
        assert!(!text.contains("assistant result"));
        assert!(!text.contains("Do not inspect repository state"));
    }

    #[test]
    fn worker_started_task_hides_direct_answer_instruction() {
        let task_prompt = "Answer the user's message directly in the same language.\n\
            This is a conversational or explanatory request.\n\n\
            User message:\n\
            你好";

        assert_eq!(visible_task_prompt(task_prompt), "你好");
    }

    #[test]
    fn worker_started_title_shows_reused_native_session_after_metadata_fill() {
        let mut state = BridgeState::default();
        let thread_ready = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "ready".to_string(),
            message: "worker-cc worker thread reused".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            worker_role: Some("worker-cc".to_string()),
            worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".to_string()),
            native_session_id: Some("5cee8032-e580-49f9-b0b8-95b0940c5692".to_string()),
            reused: Some(true),
            ..TiffanyProgressEvent::default()
        };
        state.remember_worker_metadata(&thread_ready);

        let mut started = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "running".to_string(),
            message: "worker-cc started".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            runtime: Some("claude-code".to_string()),
            model: Some("MiniMax-M3".to_string()),
            provider: Some("minimax".to_string()),
            ..TiffanyProgressEvent::default()
        };
        state.apply_worker_metadata(&mut started);

        assert_eq!(
            line_text(&waterfall_status_line(&started)),
            "● worker  worker-cc started with reused session · claude-code · minimax/MiniMax-M3 · thread abcdef12 · native 5cee8032 · 12345678"
        );
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: None,
            approved: None,
            issues: None,
            count: None,
            duration_ms: Some(1_250),
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some("{\"approved\":true}".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
        let event = role_output_event(
            "critic",
            r#"{"approved":false,"issues":["worker prompt conflicts with direct answer"],"suggestions":["answer in Chinese"]}"#,
        );

        let visible = visible_content(&event).expect("visible critic output");

        assert!(visible.contains("needs changes - 1 issue(s)"));
        assert!(visible.contains("worker prompt conflicts with direct answer"));
        assert!(visible.contains("suggestions:"));
        assert!(!is_redundant_role_output(&event.role, &visible));
    }

    #[test]
    fn shows_actionable_stderr_instead_of_hiding_model_errors() {
        let event = worker_output_event(
            "codex output",
            "worker-codex",
            "worker-codex stderr: [1211] 模型不存在",
        );

        assert_eq!(
            visible_content(&event).as_deref(),
            Some("[1211] 模型不存在")
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some("结果\n- done".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };
        assert_eq!(visible_content(&event).as_deref(), Some("结果\n- done"));
        assert_eq!(
            event_title(&event),
            "claude-code - claude output · 12345678"
        );
    }

    #[test]
    fn keeps_full_worker_message_stream_without_truncation() {
        let long_message = format!("{}END", "x".repeat(CONTROL_SUMMARY_MAX_CHARS + 128));
        let event = worker_output_event(
            "claude output",
            "claude-code",
            &format!("claude-code assistant: {long_message}"),
        );

        let visible = visible_content(&event).expect("worker output visible");

        assert_eq!(visible, long_message);
        assert!(!visible.contains('…'));
    }

    #[test]
    fn strips_runtime_prefix_for_manual_style_output() {
        let event = worker_output_event(
            "claude output",
            "claude-code",
            "claude-code assistant: done",
        );

        assert_eq!(visible_content(&event).as_deref(), Some("done"));
        assert_eq!(output_label(&event), "claude-code · 12345678");
        assert_eq!(
            output_title(&event),
            "worker output · claude-code · 12345678"
        );
    }

    #[test]
    fn strips_runtime_prefix_for_tool_output() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event(
                "claude output",
                "claude-code",
                "claude-code tool_use: tool Bash: cargo test",
            )
        };

        assert_eq!(
            visible_content(&event).as_deref(),
            Some("tool Bash: cargo test")
        );

        let event = TiffanyProgressEvent {
            worker_role: Some("worker-codex".to_string()),
            ..worker_output_event(
                "codex output",
                "worker-codex",
                "codex local_shell_call: tool shell: cargo test --all",
            )
        };

        assert_eq!(
            visible_content(&event).as_deref(),
            Some("tool shell: cargo test --all")
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some("Final result\n你好！\n我可以帮你写代码。".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some("Final result\n你好！\n我可以帮你写代码。".to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code assistant: 你好！",
            )
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
        let assistant = worker_output_event(
            "claude output",
            "claude-code",
            "claude-code assistant: useful summary",
        );
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
            "plan ready - 1 worker run(s)"
        ));
        assert!(!is_redundant_role_output(
            "planner",
            "plan ready - 1 worker run(s)\n  1. answer the user"
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some(
                r#"{"sub_tasks":[{"prompt":"answer in Chinese","agent_hint":"worker-cc"}]}"#
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some(r#"{"approved":false,"issues":["missing final answer"],"suggestions":["return a concise result"]}"#.to_string()),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some(
                r#"{"sub_tasks":[{"prompt":"Audit the TUI output path and identify noisy controller messages that are repeated in the visible transcript","agent_hint":"worker-cc"},{"prompt":"Update the forked TUI renderer to keep worker output visible while compacting planner details","agent_hint":"worker-codex"},{"prompt":"Add regression tests for compact planner rendering and preserved reviewer issues","agent_hint":"worker-cc"},{"prompt":"Run the smoke check and summarize the result","agent_hint":"worker-cc"}]}"#
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };

        let visible = visible_content(&planner).expect("visible compact plan");

        assert!(visible.contains("plan ready - 4 worker run(s)"));
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some(
                "critic>\n  {\"approved\": false, \"issues\": [\"Self-contradictory: delegated despite direct-answer instruction\",\n  \"Unnecessary delegation"
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
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
            cc_agent: None,
            model: None,
            provider: None,
            worker_thread_id: None,
            native_session_id: None,
            reused: None,
            task_prompt: None,
            content: Some(
                "planner>\n  {\"sub_tasks\": [{\"prompt\": \"Answer in Chinese and keep the result concise"
                    .to_string(),
            ),
            approved: None,
            issues: None,
            count: None,
            duration_ms: None,
            reason: None,
            route: None,
            route_label: None,
            route_reason_label: None,
            flow_steps: None,
        };

        let visible = visible_content(&planner).expect("visible planner output");

        assert!(visible.contains("plan ready - 1 worker run(s)"));
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

        assert!(text.contains("planner returned no worker runs"));
        assert!(!text.contains("sub_tasks"));
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
        assert!(text.contains("provider API model name"));
        assert!(text.contains("--model-name <api-model>"));
        assert!(text.contains("/doctor"));
    }

    #[cfg(unix)]
    #[test]
    fn failure_lines_keep_start_and_end_of_long_diagnostics() {
        use std::os::unix::process::ExitStatusExt;

        let detail = (0..140)
            .map(|idx| {
                if idx == 0 {
                    "first line with root cause".to_string()
                } else if idx == 139 {
                    "last line with final error".to_string()
                } else {
                    format!("detail line {idx}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let lines = orchestrator_failure_lines(ExitStatus::from_raw(1 << 8), &detail, "", &[]);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("first line with root cause"));
        assert!(text.contains("last line with final error"));
        assert!(text.contains("hidden"));
        assert!(!text.contains("detail line 40"));
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
    fn orchestrator_command_defaults_empty_bin() {
        assert_eq!(
            orchestrator_command("", None).as_std().get_program(),
            "orchestrator"
        );
        assert_eq!(
            orchestrator_command("   ", None).as_std().get_program(),
            "orchestrator"
        );
    }

    #[test]
    fn runtime_status_normalizes_empty_bin() {
        let status = runtime_status("   ");

        assert_eq!(status.requested, "orchestrator");
    }

    #[test]
    fn runtime_status_rejects_missing_explicit_path() {
        let missing = std::env::temp_dir().join(format!(
            "definitely-missing-tiffany-orchestrator-runtime-{}",
            std::process::id()
        ));

        let status = runtime_status(&missing.to_string_lossy());

        assert_eq!(status.resolved, None);
    }

    #[test]
    fn runtime_status_accepts_launchable_explicit_path() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let runtime = temp.path().join(executable_name("orchestrator"));
        std::fs::write(&runtime, "")?;
        make_launchable(&runtime)?;

        let status = runtime_status(&runtime.to_string_lossy());

        assert_eq!(
            status.resolved.as_deref(),
            Some(runtime.to_string_lossy().as_ref())
        );
        Ok(())
    }

    #[cfg(unix)]
    fn make_launchable(path: &std::path::Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)
    }

    #[cfg(not(unix))]
    fn make_launchable(_path: &std::path::Path) -> std::io::Result<()> {
        Ok(())
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
