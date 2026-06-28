use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
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
use tiffany_bridge::ConfigSummary as TiffanyConfigSummary;
use tiffany_bridge::ContextPromptTurn as TiffanyContextPromptTurn;
#[cfg(test)]
use tiffany_bridge::ContinueRequest;
use tiffany_bridge::JobSummary;
use tiffany_bridge::NATIVE_CHAT_STORE_SCHEMA_VERSION as NATIVE_SESSIONS_SCHEMA_VERSION;
use tiffany_bridge::NativeHistoryCommand;
use tiffany_bridge::NativeHistoryEventView;
use tiffany_bridge::NativeHistoryFilter;
use tiffany_bridge::NativeHistoryMarkdownTurn;
use tiffany_bridge::NativeHistoryOptions;
use tiffany_bridge::NativeSessionCommand as TiffanyBridgeNativeSessionCommand;
use tiffany_bridge::NativeSessionRuntime as TiffanyNativeTranscriptRuntime;
use tiffany_bridge::NativeTranscriptEvent as TiffanyBridgeNativeTranscriptEvent;
use tiffany_bridge::PendingVisibleOutput as TiffanyBridgePendingVisibleOutput;
use tiffany_bridge::ProviderSummary;
use tiffany_bridge::RoleOptionSummary;
use tiffany_bridge::RoleProfileRow;
use tiffany_bridge::RoleSummary;
use tiffany_bridge::ThreadSummary;
use tiffany_bridge::VisibleOutputEvent as TiffanyBridgeVisibleOutputEvent;
use tiffany_bridge::WorkerReadinessStatus as TiffanyWorkerReadinessStatus;
use tiffany_bridge::continue_request;
#[cfg(test)]
use tiffany_bridge::continue_target_role;
use tiffany_bridge::doctor_command_args;
use tiffany_bridge::job_actions;
use tiffany_bridge::job_state_summary;
use tiffany_bridge::jobs_command_args;
use tiffany_bridge::native_conversation_id;
use tiffany_bridge::native_history_kind_matches;
use tiffany_bridge::native_history_tool_event_label;
use tiffany_bridge::normalize_native_chat_store;
use tiffany_bridge::normalize_native_event as bridge_normalize_native_event;
use tiffany_bridge::parse_claude_native_transcript_events;
use tiffany_bridge::parse_codex_native_transcript_events;
use tiffany_bridge::parse_gemini_native_transcript_events;
use tiffany_bridge::parse_job_summaries;
use tiffany_bridge::parse_jobs_header_counts;
use tiffany_bridge::parse_jobs_retry_handoff;
use tiffany_bridge::parse_native_cli_handoff;
use tiffany_bridge::parse_prefixed_field;
use tiffany_bridge::parse_provider_summaries;
use tiffany_bridge::parse_recovered_jobs_failed_count;
use tiffany_bridge::parse_recovered_jobs_ids;
use tiffany_bridge::parse_role_option_summaries;
use tiffany_bridge::parse_role_profile_rows;
use tiffany_bridge::parse_role_summaries;
use tiffany_bridge::parse_thread_fields;
use tiffany_bridge::parse_thread_list_summaries;
use tiffany_bridge::parse_worker_thread_title;
use tiffany_bridge::provider_command_args;
use tiffany_bridge::render_native_history_markdown;
use tiffany_bridge::render_native_history_mermaid;
use tiffany_bridge::render_native_history_text_graph;
use tiffany_bridge::retry_prompt_from_jobs_retry_output;
use tiffany_bridge::roles_command_args;
use tiffany_bridge::thread_command_args;
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

pub(crate) type TiffanyNativeChatStore = tiffany_bridge::NativeChatStore;
pub(crate) type TiffanyNativeChatConversation = tiffany_bridge::NativeChatConversation;
pub(crate) type TiffanyNativeChatTurn = tiffany_bridge::NativeChatTurn;
pub(crate) type TiffanyNativeChatEvent = tiffany_bridge::NativeChatEvent;

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

#[derive(Clone, Debug)]
pub struct TiffanyOrchestratorLaunch {
    pub bin: String,
    pub user_prompt: String,
    pub prompt: String,
    pub extra_args: Vec<String>,
    pub config_path: Option<String>,
    pub context_turn_count: usize,
    pub worker_scope: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TiffanyOrchestratorConfig {
    pub(crate) bin: String,
    pub(crate) extra_args: Vec<String>,
    pub(crate) config_path: Option<String>,
    pub(crate) worker_scope: String,
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

#[derive(Clone, Debug)]
pub(crate) struct TiffanyOrchestratorTurn {
    pub(crate) user_prompt: String,
    pub(crate) result: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TiffanyNativeCliCommand {
    pub(crate) role: String,
    pub(crate) runtime: Option<String>,
    pub(crate) worker_thread_id: Option<String>,
    pub(crate) native_session: String,
    pub(crate) command: String,
    pub(crate) worktree: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TiffanyNativeCliReturn {
    pub(crate) status: Option<String>,
    pub(crate) diff_stat: Option<String>,
    pub(crate) diff_patch: Option<String>,
    pub(crate) staged_diff_stat: Option<String>,
    pub(crate) staged_diff_patch: Option<String>,
    pub(crate) transcript_event_count: usize,
    pub(crate) transcript_preview: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TiffanyNativeCliTranscriptCursor {
    path: PathBuf,
    byte_len: u64,
    runtime: TiffanyNativeTranscriptRuntime,
    message_count: usize,
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

fn memory_schema_version() -> u32 {
    MEMORY_SCHEMA_VERSION
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
            worker_scope: self.worker_scope.clone(),
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
            worker_scope: self.worker_scope.clone(),
        }
    }
}

pub(crate) fn new_tiffany_worker_scope(cwd: &Path, tui_session_id: &str) -> String {
    let cwd_key = cwd_key(cwd);
    let session = tui_session_id.trim();
    if session.is_empty() {
        return format!("tui:{cwd_key}");
    }
    format!("tui:{cwd_key}:session:{session}")
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
    recovery: Option<String>,
    event_kind: Option<String>,
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
    final_output_visible: bool,
    streamed_answer_text: Option<String>,
    answer_stream_active: bool,
    worker_metadata: HashMap<String, WorkerMeta>,
    pending_timeline: Vec<TiffanyProgressEvent>,
    pending_tool_call_output: Option<(TiffanyProgressEvent, String)>,
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

pub(crate) fn spawn_jobs_command(
    app_event_tx: AppEventSender,
    config: TiffanyOrchestratorConfig,
    args: String,
) {
    tokio::spawn(async move {
        let command_args = match jobs_command_args(&args) {
            Ok(command_args) => command_args,
            Err(err) => {
                emit_lines(
                    &app_event_tx,
                    vec![
                        status_line("✗", Color::Red, "jobs", &err),
                        body_line(
                            "Usage: /jobs [limit] | /jobs show <id> | /jobs cancel <id> | /jobs retry <id> | /jobs recover [minutes]",
                            true,
                        ),
                    ],
                );
                return;
            }
        };

        match run_orchestrator_command(&config, &command_args).await {
            Ok(output) => {
                let retry_prompt = if output.status.success()
                    && is_jobs_retry_command(&command_args)
                {
                    retry_prompt_from_jobs_retry_output(&String::from_utf8_lossy(&output.stdout))
                } else {
                    None
                };
                emit_command_output(&app_event_tx, "jobs", &command_args, output);
                if let Some(prompt) = retry_prompt {
                    app_event_tx.send(AppEvent::TiffanyOrchestratorQueuePrompt {
                        prompt,
                        source: "/jobs retry".to_string(),
                    });
                }
            }
            Err(err) => emit_lines(
                &app_event_tx,
                spawn_error_lines(
                    "jobs",
                    "✗",
                    Color::Red,
                    &format!("failed to run jobs command - {err}"),
                    &err.to_string(),
                ),
            ),
        }
    });
}

pub(crate) fn spawn_continue_command(
    app_event_tx: AppEventSender,
    config: TiffanyOrchestratorConfig,
    args: String,
) {
    tokio::spawn(async move {
        let request = match continue_request(&args) {
            Ok(request) => request,
            Err(err) => {
                emit_lines(
                    &app_event_tx,
                    vec![
                        status_line("✗", Color::Red, "continue", &err),
                        body_line("Usage: /continue [open] <role|claude|codex|gemini>", true),
                    ],
                );
                return;
            }
        };
        let command_args = vec![
            "thread".to_string(),
            "show".to_string(),
            request.role.clone(),
        ];

        match run_orchestrator_command(&config, &command_args).await {
            Ok(output) => {
                if request.open {
                    emit_continue_open_output(&app_event_tx, output);
                } else {
                    emit_command_output(&app_event_tx, "continue", &command_args, output);
                }
            }
            Err(err) => emit_lines(
                &app_event_tx,
                spawn_error_lines(
                    "continue",
                    "✗",
                    Color::Red,
                    &format!("failed to inspect worker handoff - {err}"),
                    &err.to_string(),
                ),
            ),
        }
    });
}

pub(crate) fn spawn_history_command(
    app_event_tx: AppEventSender,
    config: TiffanyOrchestratorConfig,
    codex_home: PathBuf,
    cwd: PathBuf,
    args: String,
) {
    tokio::spawn(async move {
        if matches!(
            NativeHistoryCommand::parse(&args),
            Ok(NativeHistoryCommand::Sync)
        ) {
            emit_lines(
                &app_event_tx,
                native_history_sync_lines(&config, &codex_home).await,
            );
            return;
        }
        if matches!(
            NativeHistoryCommand::parse(&args),
            Ok(NativeHistoryCommand::Status)
        ) {
            emit_lines(
                &app_event_tx,
                native_history_status_lines(&config, &codex_home, &cwd).await,
            );
            return;
        }

        let loaded = match load_native_history_from_store(&config, &cwd).await {
            Ok(Some(conversation)) => Ok(Some(NativeHistoryLoadedConversation {
                source: NativeHistorySource::SessionDb,
                conversation,
            })),
            Ok(None) => load_native_chat_conversation(&codex_home, &cwd)
                .map(|conversation| conversation.map(NativeHistoryLoadedConversation::local_json)),
            Err(err) => {
                tracing::warn!("failed to load Tiffany native history from session store: {err:#}");
                load_native_chat_conversation(&codex_home, &cwd).map(|conversation| {
                    conversation.map(NativeHistoryLoadedConversation::local_json)
                })
            }
        };
        emit_lines(
            &app_event_tx,
            native_history_lines_from_conversation(loaded, &codex_home, &cwd, &args),
        );
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
            &[
                "/provider",
                "/role",
                "/roles",
                "/thread",
                "/continue",
                "/history",
                "/doctor",
            ],
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
    let summary = match tiffany_bridge::summarize_orchestrator_config_yaml(&raw, |binary| {
        runtime_status(binary).resolved.is_some()
    }) {
        Ok(summary) => summary,
        Err(err) => {
            return TiffanyConfigReadiness::Invalid {
                path: display_path,
                message: err,
            };
        }
    };
    TiffanyConfigReadiness::Ready(summary)
}

pub(crate) fn startup_readiness_lines(readiness: &TiffanyStartupReadiness) -> Vec<Line<'static>> {
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
                    worker_readiness_label(worker.status),
                    format!("{} · {} · {}", worker.role, worker.runtime, worker.binary),
                    worker_readiness_color(worker.status),
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

pub(crate) fn startup_health_action_lines(
    readiness: &TiffanyStartupReadiness,
) -> Vec<Line<'static>> {
    let mut actions = startup_health_actions(readiness);
    if actions.is_empty() {
        return vec![health_action_line(
            "ok",
            "all configured checks passed",
            "/doctor for full diagnostics",
            TIFFANY_SOFT,
        )];
    }
    actions.sort_by_key(|action| action.priority);
    actions
        .into_iter()
        .map(|action| health_action_line(action.label, action.detail, action.fix, action.color))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StartupHealthAction {
    priority: u8,
    label: &'static str,
    detail: String,
    fix: &'static str,
    color: Color,
}

fn startup_health_actions(readiness: &TiffanyStartupReadiness) -> Vec<StartupHealthAction> {
    let mut actions = Vec::new();
    if readiness.runtime.resolved.is_none() {
        actions.push(StartupHealthAction {
            priority: 10,
            label: "runtime",
            detail: format!(
                "orchestrator binary '{}' is not launchable",
                readiness.runtime.requested
            ),
            fix: "brew reinstall tiffany-loop or set TIFFANY_ORCHESTRATOR_BIN",
            color: Color::Red,
        });
    }

    match &readiness.config {
        TiffanyConfigReadiness::Missing { path } => {
            actions.push(StartupHealthAction {
                priority: 20,
                label: "config",
                detail: format!("config missing at {path}"),
                fix: "/provider then /role",
                color: Color::Red,
            });
            actions.push(StartupHealthAction {
                priority: 30,
                label: "provider auth",
                detail: "no providers can be verified yet".to_string(),
                fix: "/provider",
                color: Color::Yellow,
            });
            actions.push(StartupHealthAction {
                priority: 40,
                label: "role wiring",
                detail: "no worker role is registered yet".to_string(),
                fix: "/role worker-cc or /roles profile",
                color: Color::Yellow,
            });
        }
        TiffanyConfigReadiness::Invalid { path, message } => {
            actions.push(StartupHealthAction {
                priority: 20,
                label: "config",
                detail: format!(
                    "config invalid at {path}: {}",
                    truncate_for_status(message, 90)
                ),
                fix: "/provider then /doctor",
                color: Color::Red,
            });
        }
        TiffanyConfigReadiness::Ready(summary) => {
            extend_config_summary_health_actions(&mut actions, summary);
        }
    }

    if !actions.is_empty() {
        actions.push(StartupHealthAction {
            priority: 90,
            label: "doctor",
            detail: "run full diagnostics before retrying a failed orchestration".to_string(),
            fix: "/doctor",
            color: TIFFANY_SOFT,
        });
    }
    actions
}

fn extend_config_summary_health_actions(
    actions: &mut Vec<StartupHealthAction>,
    summary: &TiffanyConfigSummary,
) {
    let default_status = summary.default_worker.as_ref().map(|worker| worker.status);
    if summary.providers == 0
        && default_status != Some(TiffanyWorkerReadinessStatus::ProviderMissing)
    {
        actions.push(StartupHealthAction {
            priority: 30,
            label: "provider auth",
            detail: "no provider configured".to_string(),
            fix: "/provider",
            color: Color::Red,
        });
    }
    if summary.models == 0 && default_status != Some(TiffanyWorkerReadinessStatus::ModelMissing) {
        actions.push(StartupHealthAction {
            priority: 35,
            label: "model",
            detail: "no model entry configured".to_string(),
            fix: "/provider then /role",
            color: Color::Red,
        });
    }
    if summary.roles == 0 {
        actions.push(StartupHealthAction {
            priority: 40,
            label: "role wiring",
            detail: "no role registered".to_string(),
            fix: "/role worker-cc or /roles profile",
            color: Color::Red,
        });
    }
    if summary.runtimes == 0 && default_status != Some(TiffanyWorkerReadinessStatus::RuntimeMissing)
    {
        actions.push(StartupHealthAction {
            priority: 45,
            label: "runtime",
            detail: "no CLI runtime configured".to_string(),
            fix: "/role worker-cc or /roles profile",
            color: Color::Red,
        });
    }

    match &summary.default_worker {
        Some(worker) => match worker.status {
            TiffanyWorkerReadinessStatus::Ready => {}
            TiffanyWorkerReadinessStatus::ModelMissing => actions.push(StartupHealthAction {
                priority: 41,
                label: "role wiring",
                detail: format!("{} points to a missing model", worker.role),
                fix: "/role worker-cc or /roles profile",
                color: Color::Red,
            }),
            TiffanyWorkerReadinessStatus::ProviderMissing => actions.push(StartupHealthAction {
                priority: 31,
                label: "provider auth",
                detail: format!("{} model provider is missing", worker.role),
                fix: "/provider then /role worker-cc",
                color: Color::Red,
            }),
            TiffanyWorkerReadinessStatus::RuntimeMissing => actions.push(StartupHealthAction {
                priority: 11,
                label: "runtime",
                detail: format!(
                    "{} runtime '{}' is not launchable",
                    worker.role, worker.binary
                ),
                fix: "install the selected CLI or update /role worker-cc",
                color: Color::Red,
            }),
        },
        None => actions.push(StartupHealthAction {
            priority: 40,
            label: "role wiring",
            detail: "default worker role is missing".to_string(),
            fix: "/role worker-cc or /roles profile",
            color: Color::Red,
        }),
    }
}

fn health_action_line(
    label: &'static str,
    detail: impl Into<String>,
    fix: &'static str,
    color: Color,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "health",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            truncate_for_status(&detail.into(), 120),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  fix "),
        Span::styled(fix, Style::default().fg(TIFFANY_SOFT)),
    ])
}

fn worker_readiness_label(status: TiffanyWorkerReadinessStatus) -> &'static str {
    match status {
        TiffanyWorkerReadinessStatus::Ready => "ready",
        TiffanyWorkerReadinessStatus::ModelMissing => "model-missing",
        TiffanyWorkerReadinessStatus::ProviderMissing => "provider-missing",
        TiffanyWorkerReadinessStatus::RuntimeMissing => "runtime-missing",
    }
}

fn worker_readiness_color(status: TiffanyWorkerReadinessStatus) -> Color {
    match status {
        TiffanyWorkerReadinessStatus::Ready => TIFFANY_SOFT,
        TiffanyWorkerReadinessStatus::ModelMissing
        | TiffanyWorkerReadinessStatus::ProviderMissing
        | TiffanyWorkerReadinessStatus::RuntimeMissing => Color::Red,
    }
}

fn is_worker_role_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    (name.contains("worker") || name.contains("executor")) && !name.contains("reviewer")
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
    let turns = turns
        .iter()
        .map(|turn| TiffanyContextPromptTurn {
            user_prompt: turn.user_prompt.as_str(),
            result: turn.result.as_str(),
        })
        .collect::<Vec<_>>();
    tiffany_bridge::contextual_prompt(&turns, current_prompt)
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

#[cfg(test)]
fn native_history_lines(codex_home: &Path, cwd: &Path, args: &str) -> Vec<Line<'static>> {
    native_history_lines_from_conversation(
        load_native_chat_conversation(codex_home, cwd)
            .map(|conversation| conversation.map(NativeHistoryLoadedConversation::local_json)),
        codex_home,
        cwd,
        args,
    )
}

fn native_history_lines_from_conversation(
    loaded: anyhow::Result<Option<NativeHistoryLoadedConversation>>,
    codex_home: &Path,
    cwd: &Path,
    args: &str,
) -> Vec<Line<'static>> {
    let command = match NativeHistoryCommand::parse(args) {
        Ok(command) => command,
        Err(err) => {
            return vec![
                status_line("✗", Color::Red, "history", &err),
                body_line(
                    "Usage: /history [last|full|<count>|role <role>|thread <id>|native <session-id>|kind <event-kind>|search <text>|status|sync|compact|graph|mermaid|export-graph [--out path]|export [role <role>|thread <id>|native <session-id>|kind <event-kind>] [--out path]]",
                    true,
                ),
            ];
        }
    };
    match loaded {
        Ok(Some(loaded)) => match command {
            NativeHistoryCommand::Approvals { turn_limit } => {
                native_history_approval_lines(&loaded.conversation, turn_limit, loaded.source)
            }
            NativeHistoryCommand::Show(opts) => {
                native_history_conversation_lines(&loaded.conversation, opts, loaded.source)
            }
            NativeHistoryCommand::Export { out, filter } => {
                native_history_export_lines(codex_home, cwd, &loaded.conversation, out, filter)
            }
            NativeHistoryCommand::Graph {
                mermaid,
                out,
                export,
            } => native_history_graph_lines(
                codex_home,
                cwd,
                &loaded.conversation,
                mermaid,
                out,
                export,
            ),
            NativeHistoryCommand::Compact { turn_limit, filter } => native_history_compact_lines(
                &loaded.conversation,
                turn_limit,
                filter,
                loaded.source,
            ),
            NativeHistoryCommand::Search { pattern } => {
                native_history_search_lines(&loaded.conversation, &pattern, loaded.source)
            }
            NativeHistoryCommand::Sync => vec![
                status_line(
                    "⚠",
                    Color::Yellow,
                    "history",
                    "sync must run before loading history",
                ),
                next_line(
                    "/history sync",
                    "mirror local native history into session DB",
                ),
            ],
            NativeHistoryCommand::Status => vec![
                status_line(
                    "⚠",
                    Color::Yellow,
                    "history",
                    "status must run before loading history",
                ),
                next_line(
                    "/history status",
                    "compare session DB and local native history",
                ),
            ],
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
) -> anyhow::Result<Option<PathBuf>> {
    let Some(native_turn) = native_chat_turn_from_turn(turn, events) else {
        return Ok(None);
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
    Ok(Some(path))
}

pub(crate) fn append_native_cli_return(
    codex_home: &Path,
    cwd: &Path,
    command: &TiffanyNativeCliCommand,
    outcome: &TiffanyNativeCliReturn,
    transcript_events: Vec<TiffanyNativeChatEvent>,
) -> anyhow::Result<Option<PathBuf>> {
    let has_changes = native_cli_return_has_changes(outcome);
    let mut events = transcript_events;
    if !has_changes && events.is_empty() {
        events.push(native_cli_return_event(
            command,
            "status",
            "native CLI return",
            "native CLI returned without captured transcript or workspace changes",
        ));
    }
    if let Some(status) = outcome.status.as_deref().and_then(nonempty_trimmed) {
        events.push(native_cli_return_event(
            command,
            "file_update",
            "git status --short",
            status,
        ));
    }
    if let Some(content) = native_cli_diff_event_content(
        "git diff --stat",
        outcome.diff_stat.as_deref(),
        "git diff",
        outcome.diff_patch.as_deref(),
    ) {
        events.push(native_cli_return_event(
            command, "diff", "git diff", &content,
        ));
    }
    if let Some(content) = native_cli_diff_event_content(
        "git diff --cached --stat",
        outcome.staged_diff_stat.as_deref(),
        "git diff --cached",
        outcome.staged_diff_patch.as_deref(),
    ) {
        events.push(native_cli_return_event(
            command,
            "diff",
            "git diff --cached",
            &content,
        ));
    }
    append_native_chat_turn(
        codex_home,
        cwd,
        &TiffanyOrchestratorTurn {
            user_prompt: format!("/continue open {}", command.role),
            result: outcome
                .transcript_preview
                .as_deref()
                .and_then(nonempty_trimmed)
                .map(str::to_string)
                .unwrap_or_else(|| "native CLI returned to Tiffany".to_string()),
        },
        events,
    )
}

pub(crate) fn native_cli_transcript_preview(events: &[TiffanyNativeChatEvent]) -> Option<String> {
    events
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event.kind.as_deref(),
                Some("final") | Some("answer") | Some("question")
            )
        })
        .and_then(|event| event.content.as_deref())
        .and_then(nonempty_trimmed)
        .map(|content| truncate_text(content, 4_000))
}

pub(crate) fn native_cli_transcript_cursor(
    command: &TiffanyNativeCliCommand,
) -> Option<TiffanyNativeCliTranscriptCursor> {
    let home = dirs::home_dir()?;
    let session_path = tiffany_bridge::native_session_path_in_home(
        &home,
        &bridge_native_session_command(command),
    )?;
    let byte_len = std::fs::metadata(&session_path.path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    Some(TiffanyNativeCliTranscriptCursor {
        path: session_path.path,
        byte_len,
        runtime: session_path.runtime,
        message_count: session_path.message_count,
    })
}

pub(crate) fn native_cli_transcript_events_since(
    command: &TiffanyNativeCliCommand,
    cursor: Option<&TiffanyNativeCliTranscriptCursor>,
) -> Vec<TiffanyNativeChatEvent> {
    let Some(cursor) = cursor else {
        return Vec::new();
    };
    let Ok(meta) = std::fs::metadata(&cursor.path) else {
        return Vec::new();
    };
    if cursor.runtime != TiffanyNativeTranscriptRuntime::Gemini && meta.len() <= cursor.byte_len {
        return Vec::new();
    }
    let Ok(mut file) = std::fs::File::open(&cursor.path) else {
        return Vec::new();
    };
    use std::io::Read as _;
    use std::io::Seek as _;
    if cursor.runtime != TiffanyNativeTranscriptRuntime::Gemini {
        if file
            .seek(std::io::SeekFrom::Start(cursor.byte_len))
            .is_err()
        {
            return Vec::new();
        }
    }
    let mut body = String::new();
    if file.read_to_string(&mut body).is_err() {
        return Vec::new();
    }
    match cursor.runtime {
        TiffanyNativeTranscriptRuntime::Claude => {
            native_cli_transcript_events_from_claude_jsonl(command, &body)
        }
        TiffanyNativeTranscriptRuntime::Codex => {
            native_cli_transcript_events_from_codex_jsonl(command, &body)
        }
        TiffanyNativeTranscriptRuntime::Gemini => {
            native_cli_transcript_events_from_gemini_json(command, &body, cursor.message_count)
        }
    }
}

pub(crate) fn native_cli_transcript_events_after_return(
    command: &TiffanyNativeCliCommand,
    cursor: Option<&TiffanyNativeCliTranscriptCursor>,
) -> Vec<TiffanyNativeChatEvent> {
    let events = native_cli_transcript_events_since(command, cursor);
    if !events.is_empty() || cursor.is_some() {
        return events;
    }

    let Some(new_cursor) = native_cli_transcript_cursor(command) else {
        return Vec::new();
    };
    native_cli_transcript_events_since(
        command,
        Some(&TiffanyNativeCliTranscriptCursor {
            byte_len: 0,
            message_count: 0,
            ..new_cursor
        }),
    )
}

#[cfg(test)]
fn native_cli_command_is_claude(command: &TiffanyNativeCliCommand) -> bool {
    tiffany_bridge::native_session_command_is_claude(&bridge_native_session_command(command))
}

fn native_cli_command_is_codex(command: &TiffanyNativeCliCommand) -> bool {
    tiffany_bridge::native_session_command_is_codex(&bridge_native_session_command(command))
}

fn native_cli_command_is_gemini(command: &TiffanyNativeCliCommand) -> bool {
    tiffany_bridge::native_session_command_is_gemini(&bridge_native_session_command(command))
}

fn bridge_native_session_command(
    command: &TiffanyNativeCliCommand,
) -> TiffanyBridgeNativeSessionCommand<'_> {
    TiffanyBridgeNativeSessionCommand {
        role: &command.role,
        runtime: command.runtime.as_deref(),
        native_session: &command.native_session,
        command: &command.command,
        worktree: command.worktree.as_deref(),
    }
}

#[cfg(test)]
fn gemini_session_json_path_in_home(
    home: &Path,
    command: &TiffanyNativeCliCommand,
) -> Option<PathBuf> {
    tiffany_bridge::gemini_session_json_path_in_home(home, &bridge_native_session_command(command))
}

#[cfg(test)]
fn gemini_project_hash(worktree: &str) -> Option<String> {
    tiffany_bridge::gemini_project_hash(worktree)
}

fn native_cli_transcript_events_from_claude_jsonl(
    command: &TiffanyNativeCliCommand,
    body: &str,
) -> Vec<TiffanyNativeChatEvent> {
    parse_claude_native_transcript_events(body)
        .into_iter()
        .map(|event| native_cli_transcript_event(command, event))
        .collect()
}

fn native_cli_transcript_event(
    command: &TiffanyNativeCliCommand,
    event: TiffanyBridgeNativeTranscriptEvent,
) -> TiffanyNativeChatEvent {
    let agent = if native_cli_command_is_codex(command) {
        "codex"
    } else if native_cli_command_is_gemini(command) {
        "gemini"
    } else {
        "claude-code"
    };
    normalize_native_event(TiffanyNativeChatEvent {
        role: "worker".to_string(),
        status: "output".to_string(),
        title: format!("worker {} · {}", event.title, command.role),
        kind: Some(event.kind),
        content: Some(event.content),
        agent: Some(agent.to_string()),
        worker_role: Some(command.role.clone()),
        model: None,
        provider: None,
        task_id: None,
        worker_thread_id: command
            .worker_thread_id
            .as_deref()
            .and_then(nonempty_trimmed)
            .map(str::to_string),
        native_session_id: Some(command.native_session.clone()),
    })
    .expect("native CLI transcript event should be valid")
}

fn native_cli_transcript_events_from_gemini_json(
    command: &TiffanyNativeCliCommand,
    body: &str,
    skip_messages: usize,
) -> Vec<TiffanyNativeChatEvent> {
    parse_gemini_native_transcript_events(body, skip_messages)
        .into_iter()
        .map(|event| native_cli_transcript_event(command, event))
        .collect()
}

fn native_cli_transcript_events_from_codex_jsonl(
    command: &TiffanyNativeCliCommand,
    body: &str,
) -> Vec<TiffanyNativeChatEvent> {
    parse_codex_native_transcript_events(body)
        .into_iter()
        .map(|event| native_cli_transcript_event(command, event))
        .collect()
}

fn native_cli_return_has_changes(outcome: &TiffanyNativeCliReturn) -> bool {
    [
        outcome.status.as_deref(),
        outcome.diff_stat.as_deref(),
        outcome.diff_patch.as_deref(),
        outcome.staged_diff_stat.as_deref(),
        outcome.staged_diff_patch.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
}

fn native_cli_diff_event_content(
    stat_title: &str,
    stat: Option<&str>,
    patch_title: &str,
    patch: Option<&str>,
) -> Option<String> {
    let stat = stat.and_then(nonempty_trimmed);
    let patch = patch.and_then(nonempty_trimmed);
    if stat.is_none() && patch.is_none() {
        return None;
    }

    let mut out = String::new();
    if let Some(stat) = stat {
        out.push_str(stat_title);
        out.push('\n');
        out.push_str(stat);
    }
    if let Some(patch) = patch {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(patch_title);
        out.push('\n');
        out.push_str(patch);
    }
    Some(out)
}

fn native_cli_return_event(
    command: &TiffanyNativeCliCommand,
    kind: &str,
    title: &str,
    content: &str,
) -> TiffanyNativeChatEvent {
    normalize_native_event(TiffanyNativeChatEvent {
        role: "worker".to_string(),
        status: "output".to_string(),
        title: format!("worker {kind} · {}", command.role),
        kind: Some(kind.to_string()),
        content: Some(format!("{title}\n{content}")),
        agent: None,
        worker_role: Some(command.role.clone()),
        model: None,
        provider: None,
        task_id: None,
        worker_thread_id: command.worker_thread_id.clone(),
        native_session_id: Some(command.native_session.clone()),
    })
    .expect("native CLI return event should be valid")
}

pub(crate) fn spawn_native_history_import(
    config: TiffanyOrchestratorConfig,
    native_sessions_path: PathBuf,
) {
    tokio::spawn(async move {
        if let Err(err) = import_native_history(&config, &native_sessions_path).await {
            tracing::warn!(
                "failed to mirror Tiffany native history into orchestrator session store: {err:#}"
            );
        }
    });
}

#[derive(Clone, Debug)]
struct NativeHistoryLoadedConversation {
    source: NativeHistorySource,
    conversation: TiffanyNativeChatConversation,
}

impl NativeHistoryLoadedConversation {
    fn local_json(conversation: TiffanyNativeChatConversation) -> Self {
        Self {
            source: NativeHistorySource::LocalJson,
            conversation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeHistorySource {
    SessionDb,
    LocalJson,
}

impl NativeHistorySource {
    fn label(self) -> &'static str {
        match self {
            Self::SessionDb => "session-db",
            Self::LocalJson => "local-json",
        }
    }
}

fn native_history_graph_lines(
    codex_home: &Path,
    cwd: &Path,
    conversation: &TiffanyNativeChatConversation,
    mermaid: bool,
    out: Option<PathBuf>,
    export: bool,
) -> Vec<Line<'static>> {
    if conversation.turns.is_empty() {
        return vec![
            status_line("⚠", Color::Yellow, "history", "no turns to graph"),
            next_line("/history full", "inspect saved native events"),
        ];
    }

    let body = if mermaid {
        render_native_history_mermaid(conversation, 16, export)
    } else {
        render_native_history_text_graph(conversation, 16)
    };

    if export {
        let path =
            out.unwrap_or_else(|| default_native_history_graph_path(codex_home, cwd, mermaid));
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            return vec![
                status_line(
                    "✗",
                    Color::Red,
                    "history",
                    "could not create graph export directory",
                ),
                body_line(&format!("{err:#}"), true),
            ];
        }
        return match std::fs::write(&path, body.as_bytes()) {
            Ok(()) => vec![
                status_line("✓", TIFFANY_BLUE, "history", "conversation graph exported"),
                thread_meta_line("target", &path.display().to_string()),
                thread_meta_line("bytes", &body.len().to_string()),
                next_line("open graph", &path.display().to_string()),
            ],
            Err(err) => vec![
                status_line("✗", Color::Red, "history", "could not write graph export"),
                body_line(&format!("{err:#}"), true),
            ],
        };
    }

    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "history",
        if mermaid {
            "conversation Mermaid graph"
        } else {
            "conversation flow graph"
        },
    )];
    for line in body.lines() {
        lines.push(body_line(line, false));
    }
    lines.push(next_line("/history export-graph", "save Mermaid flowchart"));
    lines
}

fn native_history_compact_lines(
    conversation: &TiffanyNativeChatConversation,
    turn_limit: usize,
    filter: Option<NativeHistoryFilter>,
    source: NativeHistorySource,
) -> Vec<Line<'static>> {
    let turns = filtered_native_history_turns(conversation, filter.as_ref());
    if turns.is_empty() {
        let filter = filter
            .as_ref()
            .map(NativeHistoryFilter::display)
            .unwrap_or_else(|| "history".to_string());
        return vec![
            status_line(
                "⚠",
                Color::Yellow,
                "history",
                &format!("no compactable events for {filter}"),
            ),
            next_line("/history full", "show all native process events"),
        ];
    }

    let total_turns = turns.len();
    let shown_turns = total_turns.min(turn_limit);
    let visible_turns = turns
        .iter()
        .rev()
        .take(turn_limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let summary = native_history_compact_summary(&visible_turns);

    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "history",
        &format!("compact storyline · showing latest {shown_turns}/{total_turns} turn(s)"),
    )];
    lines.push(thread_meta_line("cwd", &conversation.cwd));
    lines.push(thread_meta_line("session", &conversation.id));
    lines.push(thread_meta_line("source", source.label()));
    if let Some(filter) = filter.as_ref() {
        lines.push(thread_meta_line("filter", &filter.display()));
    }
    if !summary.roles.is_empty() {
        lines.push(thread_meta_line("roles", &summary.roles.join(", ")));
    }
    if !summary.event_kinds.is_empty() {
        lines.push(thread_meta_line("events", &summary.event_kinds.join(", ")));
    }
    let resume_hints = native_history_resume_hints(&visible_turns);
    if !resume_hints.is_empty() {
        lines.push(thread_meta_line("resume", &resume_hints.join("  ")));
    }

    lines.push(body_line("Task storyline", false));
    for turn in visible_turns {
        lines.push(body_line(
            &format!(
                "{}. user  {}",
                turn.turn_number,
                truncate_text(&one_line(turn.user_prompt), 110)
            ),
            false,
        ));
        lines.push(detail_continuation_line(&format!(
            "answer: {}",
            truncate_text(&one_line(turn.result), 130)
        )));
        let event_story = native_turn_compact_event_story(turn);
        if !event_story.is_empty() {
            lines.push(detail_continuation_line(&format!(
                "flow: {}",
                event_story.join(" -> ")
            )));
        }
        let artifacts = native_turn_compact_artifacts(turn);
        if !artifacts.is_empty() {
            lines.push(detail_continuation_line(&format!(
                "artifacts: {}",
                artifacts.join(", ")
            )));
        }
    }

    if total_turns > turn_limit {
        lines.push(body_line(
            &format!("… {} older turn(s) hidden", total_turns - turn_limit),
            true,
        ));
    }
    lines.push(next_line(
        "/history full",
        "expand complete native event stream",
    ));
    lines.push(next_line(
        "/history compact role <role>",
        "compact one worker role",
    ));
    lines.push(next_line(
        "/history native <session-id>",
        "inspect one Claude/Codex/Gemini native session",
    ));
    lines.push(next_line(
        "/history export",
        "write selectable Markdown handoff",
    ));
    lines
}

#[derive(Default)]
struct NativeHistoryCompactSummary {
    roles: Vec<String>,
    event_kinds: Vec<String>,
}

fn native_history_compact_summary(
    turns: &[&NativeHistoryTurnView<'_>],
) -> NativeHistoryCompactSummary {
    let mut roles = Vec::new();
    let mut kinds = Vec::new();
    for turn in turns {
        for event in &turn.events {
            let role = event
                .worker_role
                .as_deref()
                .or(event.agent.as_deref())
                .unwrap_or(event.role.as_str());
            push_unique_compact_label(&mut roles, role, 8);
            let kind = event.kind.as_deref().unwrap_or(event.status.as_str());
            push_unique_compact_label(&mut kinds, &kind.replace('_', " "), 10);
        }
    }
    NativeHistoryCompactSummary {
        roles,
        event_kinds: kinds,
    }
}

fn native_turn_compact_event_story(turn: &NativeHistoryTurnView<'_>) -> Vec<String> {
    let mut story = Vec::new();
    for event in &turn.events {
        let kind = event.kind.as_deref().unwrap_or(event.status.as_str());
        let label = match kind {
            "answer" | "final" => "answer".to_string(),
            "question" => "question".to_string(),
            "output" | "reasoning" => "reasoning".to_string(),
            "tool_call" => native_history_tool_event_label("tool", event),
            "tool_result" => "tool result".to_string(),
            "approval" => "approval needed".to_string(),
            "diff" => "diff captured".to_string(),
            "patch" => "patch captured".to_string(),
            "file_update" => "workspace changed".to_string(),
            "stderr" => "error".to_string(),
            other => other.replace('_', " "),
        };
        push_unique_compact_label(&mut story, &label, 8);
    }
    story
}

fn native_turn_compact_artifacts(turn: &NativeHistoryTurnView<'_>) -> Vec<String> {
    let mut artifacts = Vec::new();
    for event in &turn.events {
        if let Some(role) = event.worker_role.as_deref().and_then(nonempty_trimmed) {
            push_unique_compact_label(&mut artifacts, &format!("role {role}"), 6);
        }
        if let Some(thread) = event.worker_thread_id.as_deref().and_then(nonempty_trimmed) {
            let short = short_task_id(Some(thread)).unwrap_or(thread);
            push_unique_compact_label(&mut artifacts, &format!("thread {short}"), 6);
        }
        if let Some(native) = event
            .native_session_id
            .as_deref()
            .and_then(nonempty_trimmed)
        {
            let short = short_task_id(Some(native)).unwrap_or(native);
            push_unique_compact_label(&mut artifacts, &format!("native {short}"), 6);
        }
    }
    artifacts
}

fn native_history_resume_hints(turns: &[&NativeHistoryTurnView<'_>]) -> Vec<String> {
    let mut hints = Vec::new();
    for turn in turns {
        for event in &turn.events {
            let Some(role) = event.worker_role.as_deref().and_then(nonempty_trimmed) else {
                continue;
            };
            let command = if event
                .native_session_id
                .as_deref()
                .and_then(nonempty_trimmed)
                .is_some()
            {
                format!("/continue {role}")
            } else {
                format!("/thread {role}")
            };
            push_unique_compact_label(&mut hints, &command, 4);
        }
    }
    hints
}

#[derive(Clone, Debug)]
struct NativeApprovalRequest<'a> {
    turn_number: usize,
    user_prompt: &'a str,
    event: &'a TiffanyNativeChatEvent,
}

fn native_history_approval_lines(
    conversation: &TiffanyNativeChatConversation,
    turn_limit: usize,
    source: NativeHistorySource,
) -> Vec<Line<'static>> {
    let approvals = native_history_approval_requests(conversation, turn_limit);
    if approvals.is_empty() {
        return vec![
            status_line(
                "✓",
                TIFFANY_BLUE,
                "approvals",
                "no saved native approval requests",
            ),
            thread_meta_line("source", source.label()),
            next_line("/history kind approval", "inspect raw approval events"),
        ];
    }

    let mut lines = vec![status_line(
        "⚠",
        Color::Yellow,
        "approvals",
        &format!(
            "{} saved approval request(s) · latest {} turn(s)",
            approvals.len(),
            turn_limit.min(conversation.turns.len())
        ),
    )];
    lines.push(thread_meta_line("cwd", &conversation.cwd));
    lines.push(thread_meta_line("session", &conversation.id));
    lines.push(thread_meta_line("source", source.label()));

    for approval in approvals.iter().take(20) {
        let event = approval.event;
        let role = event
            .worker_role
            .as_deref()
            .and_then(nonempty_trimmed)
            .unwrap_or(event.role.as_str());
        let agent = event.agent.as_deref().and_then(nonempty_trimmed);
        let model = event.model.as_deref().and_then(nonempty_trimmed);
        let mut header = format!(
            "turn {} · {} · {}",
            approval.turn_number,
            role,
            truncate_text(&one_line(approval.user_prompt), 72)
        );
        if let Some(agent) = agent {
            header.push_str(" · ");
            header.push_str(agent);
        }
        if let Some(model) = model {
            header.push_str(" · ");
            header.push_str(model);
        }
        lines.push(body_line(&truncate_text(&header, 180), false));

        if let Some(thread) = event.worker_thread_id.as_deref().and_then(nonempty_trimmed) {
            lines.push(detail_continuation_line(&format!(
                "thread: {}",
                short_task_id(Some(thread)).unwrap_or(thread)
            )));
        }
        if let Some(native) = event
            .native_session_id
            .as_deref()
            .and_then(nonempty_trimmed)
        {
            lines.push(detail_continuation_line(&format!("native: {native}")));
        }
        if let Some(content) = event.content.as_deref().and_then(nonempty_trimmed) {
            for line in content.lines().take(4) {
                lines.push(detail_continuation_line(&truncate_text(line, 180)));
            }
            let hidden = content.lines().count().saturating_sub(4);
            if hidden > 0 {
                lines.push(detail_continuation_line(&format!(
                    "... {hidden} more line(s); /history kind approval shows full content"
                )));
            }
        }

        if event
            .native_session_id
            .as_deref()
            .and_then(nonempty_trimmed)
            .is_some()
        {
            lines.push(detail_continuation_line(&format!(
                "next: /continue open {role}  resume same native CLI conversation"
            )));
        } else {
            lines.push(detail_continuation_line(&format!(
                "next: /thread {role}  inspect worker session"
            )));
        }
    }

    if approvals.len() > 20 {
        lines.push(body_line(
            &format!(
                "... {} more approval request(s) hidden",
                approvals.len() - 20
            ),
            true,
        ));
    }
    lines.push(next_line(
        "/history kind approval",
        "show raw saved approval events",
    ));
    lines.push(next_line(
        "/continue open <role>",
        "answer or approve in the same native worker session",
    ));
    lines
}

fn native_history_approval_requests<'a>(
    conversation: &'a TiffanyNativeChatConversation,
    turn_limit: usize,
) -> Vec<NativeApprovalRequest<'a>> {
    let mut approvals = Vec::new();
    let start = conversation.turns.len().saturating_sub(turn_limit);
    for (turn_idx, turn) in conversation.turns.iter().enumerate().skip(start) {
        for event in &turn.events {
            if NativeHistoryFilter::Kind("approval".to_string())
                .matches_event(native_history_event_view(event))
            {
                approvals.push(NativeApprovalRequest {
                    turn_number: turn_idx + 1,
                    user_prompt: &turn.user_prompt,
                    event,
                });
            }
        }
    }
    approvals.reverse();
    approvals
}

fn push_unique_compact_label(labels: &mut Vec<String>, label: &str, limit: usize) {
    let label = label.trim();
    if label.is_empty() || labels.iter().any(|existing| existing == label) {
        return;
    }
    if labels.len() < limit {
        labels.push(label.to_string());
    }
}

fn native_history_conversation_lines(
    conversation: &TiffanyNativeChatConversation,
    opts: NativeHistoryOptions,
    source: NativeHistorySource,
) -> Vec<Line<'static>> {
    let turns = filtered_native_history_turns(conversation, opts.filter.as_ref());
    if turns.is_empty() {
        let filter = opts
            .filter
            .as_ref()
            .map(NativeHistoryFilter::display)
            .unwrap_or_else(|| "history".to_string());
        return vec![
            status_line(
                "⚠",
                Color::Yellow,
                "history",
                &format!("no events for {filter}"),
            ),
            next_line("/history full", "show all native process events"),
        ];
    }

    let total_turns = turns.len();
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
    lines.push(thread_meta_line("source", source.label()));
    if let Some(filter) = opts.filter.as_ref() {
        lines.push(thread_meta_line("filter", &filter.display()));
    }

    for turn in turns
        .iter()
        .rev()
        .take(opts.turn_limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        lines.push(body_line(
            &format!(
                "{}. user  {}",
                turn.turn_number,
                truncate_text(&one_line(&turn.user_prompt), 120)
            ),
            false,
        ));
        lines.push(body_line(
            &format!("   answer {}", truncate_text(&one_line(&turn.result), 140)),
            true,
        ));
        append_native_history_event_lines(
            &mut lines,
            turn,
            opts.event_limit_per_turn,
            opts.full_event_content,
        );
    }

    if total_turns > opts.turn_limit {
        lines.push(body_line(
            &format!("… {} older turn(s) hidden", total_turns - opts.turn_limit),
            true,
        ));
    }
    lines.push(next_line(
        "/history full",
        "show more native process events",
    ));
    lines.push(next_line(
        "/history kind answer|tool_result|diff|approval|session_recovery",
        "filter typed worker events",
    ));
    lines.push(next_line(
        "/history native <session-id>",
        "filter by Claude/Codex/Gemini native session id",
    ));
    lines.push(next_line(
        "/history export kind <event-kind>",
        "write a Markdown handoff for one event kind",
    ));
    lines.push(next_line(
        "/thread <role>",
        "inspect native CLI resume command",
    ));
    lines
}

fn filtered_native_history_turns<'a>(
    conversation: &'a TiffanyNativeChatConversation,
    filter: Option<&NativeHistoryFilter>,
) -> Vec<NativeHistoryTurnView<'a>> {
    conversation
        .turns
        .iter()
        .enumerate()
        .filter_map(|(turn_idx, turn)| {
            let events = match filter {
                Some(filter) => turn
                    .events
                    .iter()
                    .filter(|event| filter.matches_event(native_history_event_view(event)))
                    .collect::<Vec<_>>(),
                None => turn.events.iter().collect::<Vec<_>>(),
            };
            if filter.is_some() && events.is_empty() {
                return None;
            }
            Some(NativeHistoryTurnView {
                turn_number: turn_idx + 1,
                user_prompt: &turn.user_prompt,
                result: &turn.result,
                events,
            })
        })
        .collect()
}

fn native_history_event_view(event: &TiffanyNativeChatEvent) -> NativeHistoryEventView<'_> {
    NativeHistoryEventView {
        role: &event.role,
        agent: event.agent.as_deref(),
        worker_role: event.worker_role.as_deref(),
        worker_thread_id: event.worker_thread_id.as_deref(),
        native_session_id: event.native_session_id.as_deref(),
        kind: event.kind.as_deref(),
    }
}

struct NativeHistoryTurnView<'a> {
    turn_number: usize,
    user_prompt: &'a str,
    result: &'a str,
    events: Vec<&'a TiffanyNativeChatEvent>,
}

fn native_history_export_lines(
    codex_home: &Path,
    cwd: &Path,
    conversation: &TiffanyNativeChatConversation,
    out: Option<PathBuf>,
    filter: Option<NativeHistoryFilter>,
) -> Vec<Line<'static>> {
    let path = out.unwrap_or_else(|| default_native_history_export_path(codex_home, cwd));
    let turns = filtered_native_history_turns(conversation, filter.as_ref());
    if turns.is_empty() {
        let filter = filter
            .as_ref()
            .map(NativeHistoryFilter::display)
            .unwrap_or_else(|| "history".to_string());
        return vec![
            status_line(
                "⚠",
                Color::Yellow,
                "history",
                &format!("no events for {filter}"),
            ),
            next_line("/history full", "show all native process events"),
        ];
    }
    let filter_label = filter.as_ref().map(NativeHistoryFilter::display);
    let markdown_turns = turns
        .iter()
        .map(|turn| NativeHistoryMarkdownTurn {
            turn_number: turn.turn_number,
            user_prompt: turn.user_prompt,
            result: turn.result,
            events: turn.events.clone(),
        })
        .collect::<Vec<_>>();
    let body =
        render_native_history_markdown(conversation, &markdown_turns, filter_label.as_deref());
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return vec![
            status_line(
                "✗",
                Color::Red,
                "history",
                "could not create export directory",
            ),
            body_line(&format!("{err:#}"), true),
        ];
    }
    match std::fs::write(&path, body.as_bytes()) {
        Ok(()) => vec![
            status_line(
                "✓",
                TIFFANY_BLUE,
                "history",
                &format!("exported {} turn(s)", turns.len()),
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

fn native_history_search_lines(
    conversation: &TiffanyNativeChatConversation,
    pattern: &str,
    source: NativeHistorySource,
) -> Vec<Line<'static>> {
    let hits = native_history_search_hits(conversation, pattern, 20);
    if hits.is_empty() {
        return vec![
            status_line(
                "⚠",
                Color::Yellow,
                "history",
                &format!("no matches for '{}'", truncate_text(pattern, 80)),
            ),
            next_line("/history full", "inspect recent native events"),
        ];
    }

    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "history",
        &format!(
            "{} match(es) for '{}'",
            hits.len(),
            truncate_text(pattern, 80)
        ),
    )];
    lines.push(thread_meta_line("source", source.label()));
    for hit in hits {
        lines.push(body_line(
            &format!(
                "turn {} · {} · {}",
                hit.turn_number,
                hit.kind,
                truncate_text(&one_line(&hit.preview), 160)
            ),
            false,
        ));
    }
    lines.push(next_line("/history export", "write full Markdown handoff"));
    lines
}

#[derive(Debug, PartialEq, Eq)]
struct NativeHistorySearchHit {
    turn_number: usize,
    kind: &'static str,
    preview: String,
}

fn native_history_search_hits(
    conversation: &TiffanyNativeChatConversation,
    pattern: &str,
    limit: usize,
) -> Vec<NativeHistorySearchHit> {
    let needle = pattern.to_ascii_lowercase();
    let mut hits = Vec::new();
    for (turn_idx, turn) in conversation.turns.iter().enumerate() {
        let turn_number = turn_idx + 1;
        push_history_hit(
            &mut hits,
            limit,
            turn_number,
            "user",
            &turn.user_prompt,
            &needle,
        );
        push_history_hit(
            &mut hits,
            limit,
            turn_number,
            "answer",
            &turn.result,
            &needle,
        );
        for event in &turn.events {
            push_history_hit(
                &mut hits,
                limit,
                turn_number,
                "event",
                &event.title,
                &needle,
            );
            if let Some(content) = event.content.as_deref() {
                push_history_hit(&mut hits, limit, turn_number, "event", content, &needle);
            }
        }
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

fn push_history_hit(
    hits: &mut Vec<NativeHistorySearchHit>,
    limit: usize,
    turn_number: usize,
    kind: &'static str,
    haystack: &str,
    needle: &str,
) {
    if hits.len() >= limit || !haystack.to_ascii_lowercase().contains(needle) {
        return;
    }
    hits.push(NativeHistorySearchHit {
        turn_number,
        kind,
        preview: haystack.trim().to_string(),
    });
}

fn default_native_history_export_path(codex_home: &Path, cwd: &Path) -> PathBuf {
    codex_home
        .join("tiffany-orchestrator")
        .join("history-exports")
        .join(format!("{}.md", native_conversation_id(&cwd_key(cwd))))
}

fn default_native_history_graph_path(codex_home: &Path, cwd: &Path, mermaid: bool) -> PathBuf {
    codex_home
        .join("tiffany-orchestrator")
        .join("history-exports")
        .join(format!(
            "{}.{}",
            native_conversation_id(&cwd_key(cwd)),
            if mermaid { "mmd" } else { "txt" }
        ))
}

fn append_native_history_event_lines(
    lines: &mut Vec<Line<'static>>,
    turn: &NativeHistoryTurnView<'_>,
    limit: usize,
    full_event_content: bool,
) {
    if turn.events.is_empty() {
        lines.push(body_line("   native events none captured", true));
        return;
    }
    for event in turn.events.iter().copied().take(limit) {
        let mut label = format!("   {} {}", status_glyph(&event.status), event.title);
        if let Some(native) = event
            .native_session_id
            .as_deref()
            .and_then(nonempty_trimmed)
        {
            label.push_str(" · native ");
            label.push_str(short_task_id(Some(native)).unwrap_or(native));
        }
        if let Some(thread) = event.worker_thread_id.as_deref().and_then(nonempty_trimmed) {
            label.push_str(" · thread ");
            label.push_str(short_task_id(Some(thread)).unwrap_or(thread));
        }
        lines.push(body_line(&truncate_text(&label, 180), true));
        if let Some(kind) = event.kind.as_deref().and_then(nonempty_trimmed) {
            lines.push(detail_continuation_line(&format!("kind: {kind}")));
        }
        if let Some(content) = event.content.as_deref().and_then(nonempty_trimmed) {
            if full_event_content {
                for line in content.lines() {
                    lines.push(detail_continuation_line(line));
                }
            } else {
                for line in content.lines().take(4) {
                    lines.push(detail_continuation_line(&truncate_text(line, 180)));
                }
                let hidden = content.lines().count().saturating_sub(4);
                if hidden > 0 {
                    lines.push(detail_continuation_line(&format!(
                        "… {hidden} more line(s); /history full shows complete content"
                    )));
                }
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
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => TiffanyNativeChatStore::default(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    Ok(normalize_native_chat_store(
        store,
        CONTROL_SUMMARY_MAX_CHARS,
    ))
}

fn cwd_key(cwd: &Path) -> String {
    cwd.canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn normalize_native_event(event: TiffanyNativeChatEvent) -> Option<TiffanyNativeChatEvent> {
    bridge_normalize_native_event(event, CONTROL_SUMMARY_MAX_CHARS)
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
    let mut command = scoped_orchestrator_command(&launch.config());
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
    state.flush_pending_tool_call(&app_event_tx);
    state.flush_timeline(&app_event_tx);
    state.finish_answer_stream(&app_event_tx);
    if status.success() {
        if let Some(result) = state.final_output.take() {
            app_event_tx.send(AppEvent::TiffanyOrchestratorTurnCaptured {
                user_prompt: launch.user_prompt,
                result,
                result_already_visible: state.final_output_visible,
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
    let mut command = scoped_orchestrator_command(config);
    Ok(command.args(command_args).output().await?)
}

async fn import_native_history(
    config: &TiffanyOrchestratorConfig,
    native_sessions_path: &Path,
) -> anyhow::Result<()> {
    let output =
        run_orchestrator_command(config, &native_history_import_args(native_sessions_path)).await?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!(
        "sessions import-native exited with {}; stdout: {}; stderr: {}",
        output.status,
        stdout.trim(),
        stderr.trim()
    )
}

async fn native_history_sync_lines(
    config: &TiffanyOrchestratorConfig,
    codex_home: &Path,
) -> Vec<Line<'static>> {
    let path = native_sessions_path(codex_home);
    if !path.is_file() {
        return vec![
            status_line(
                "⚠",
                Color::Yellow,
                "history",
                "no local native history file to sync",
            ),
            thread_meta_line("source", &path.display().to_string()),
            next_line("run a prompt", "create Tiffany native history"),
        ];
    }

    let output = match run_orchestrator_command(config, &native_history_import_args(&path)).await {
        Ok(output) => output,
        Err(err) => {
            return vec![
                status_line("✗", Color::Red, "history", "sync failed"),
                thread_meta_line("source", &path.display().to_string()),
                body_line(&format!("{err:#}"), true),
            ];
        }
    };
    native_history_sync_output_lines(&path, output)
}

async fn native_history_status_lines(
    config: &TiffanyOrchestratorConfig,
    codex_home: &Path,
    cwd: &Path,
) -> Vec<Line<'static>> {
    let db = match load_native_history_from_store(config, cwd).await {
        Ok(conversation) => Ok(conversation),
        Err(err) => Err(format!("{err:#}")),
    };
    let local_path = native_sessions_path(codex_home);
    let local = match load_native_chat_conversation(codex_home, cwd) {
        Ok(conversation) => Ok(conversation),
        Err(err) => Err(format!("{err:#}")),
    };
    native_history_status_lines_from_sources(cwd, &local_path, db, local)
}

fn native_history_status_lines_from_sources(
    cwd: &Path,
    local_path: &Path,
    db: Result<Option<TiffanyNativeChatConversation>, String>,
    local: Result<Option<TiffanyNativeChatConversation>, String>,
) -> Vec<Line<'static>> {
    let db_summary = db.as_ref().ok().and_then(|conversation| {
        conversation.as_ref().map(|conversation| {
            NativeHistorySourceSummary::from_conversation("session-db", conversation)
        })
    });
    let local_summary = local.as_ref().ok().and_then(|conversation| {
        conversation.as_ref().map(|conversation| {
            NativeHistorySourceSummary::from_conversation("local-json", conversation)
        })
    });
    let (symbol, color, message) = native_history_status_header(&db_summary, &local_summary);

    let mut lines = vec![status_line(symbol, color, "history", message)];
    lines.push(thread_meta_line("cwd", &cwd_key(cwd)));
    lines.push(thread_meta_line("local", &local_path.display().to_string()));
    match db {
        Ok(Some(_)) => {
            if let Some(summary) = db_summary.as_ref() {
                lines.extend(native_history_source_summary_lines(summary));
            }
        }
        Ok(None) => lines.push(thread_meta_line("session-db", "empty")),
        Err(err) => {
            lines.push(thread_meta_line("session-db", "error"));
            lines.push(body_line(&truncate_text(&err, 220), true));
        }
    }
    match local {
        Ok(Some(_)) => {
            if let Some(summary) = local_summary.as_ref() {
                lines.extend(native_history_source_summary_lines(summary));
            }
        }
        Ok(None) => lines.push(thread_meta_line("local-json", "empty")),
        Err(err) => {
            lines.push(thread_meta_line("local-json", "error"));
            lines.push(body_line(&truncate_text(&err, 220), true));
        }
    }

    match (db_summary.as_ref(), local_summary.as_ref()) {
        (Some(db), Some(local)) if local.updated_at_unix > db.updated_at_unix => {
            lines.push(next_line("/history sync", "local native history is newer"));
        }
        (None, Some(_)) => {
            lines.push(next_line(
                "/history sync",
                "mirror local native history into session DB",
            ));
        }
        (Some(_), _) => {
            lines.push(next_line("/history", "read saved history"));
        }
        (None, None) => {
            lines.push(next_line("run a prompt", "create native history"));
        }
    }
    if let Some(summary) =
        native_history_status_primary_summary(db_summary.as_ref(), local_summary.as_ref())
    {
        lines.extend(native_history_status_action_lines(summary));
    }
    lines.push(next_line("/history compact", "review a handoff storyline"));
    lines
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeHistorySourceSummary {
    label: &'static str,
    turns: usize,
    events: usize,
    updated_at_unix: u64,
    worker_roles: Vec<String>,
    worker_threads: usize,
    native_sessions: usize,
    event_kinds: Vec<String>,
}

impl NativeHistorySourceSummary {
    fn from_conversation(
        label: &'static str,
        conversation: &TiffanyNativeChatConversation,
    ) -> Self {
        let mut worker_roles = BTreeSet::new();
        let mut worker_threads = BTreeSet::new();
        let mut native_sessions = BTreeSet::new();
        let mut event_kinds = BTreeSet::new();
        for event in conversation.turns.iter().flat_map(|turn| &turn.events) {
            if let Some(role) = event.worker_role.as_deref().and_then(nonempty_trimmed) {
                worker_roles.insert(role.to_string());
            }
            if let Some(thread) = event.worker_thread_id.as_deref().and_then(nonempty_trimmed) {
                worker_threads.insert(thread.to_string());
            }
            if let Some(native) = event
                .native_session_id
                .as_deref()
                .and_then(nonempty_trimmed)
            {
                native_sessions.insert(native.to_string());
            }
            if let Some(kind) = event.kind.as_deref().and_then(nonempty_trimmed) {
                event_kinds.insert(kind.to_string());
            }
        }
        Self {
            label,
            turns: conversation.turns.len(),
            events: conversation
                .turns
                .iter()
                .map(|turn| turn.events.len())
                .sum(),
            updated_at_unix: conversation.updated_at_unix,
            worker_roles: worker_roles.into_iter().collect(),
            worker_threads: worker_threads.len(),
            native_sessions: native_sessions.len(),
            event_kinds: event_kinds.into_iter().collect(),
        }
    }

    fn primary_worker_role(&self) -> Option<&str> {
        ["worker-cc", "worker-codex", "worker-gemini"]
            .into_iter()
            .find(|role| self.worker_roles.iter().any(|item| item.as_str() == *role))
            .or_else(|| self.worker_roles.first().map(String::as_str))
    }

    fn primary_event_kind(&self) -> Option<&str> {
        [
            "approval",
            "question",
            "tool_result",
            "tool_use",
            "diff",
            "patch",
            "file_update",
            "stderr",
            "answer",
        ]
        .into_iter()
        .find(|kind| {
            self.event_kinds
                .iter()
                .any(|item| native_history_kind_matches(item, kind))
        })
        .or_else(|| self.event_kinds.first().map(String::as_str))
    }
}

fn native_history_status_primary_summary<'a>(
    db: Option<&'a NativeHistorySourceSummary>,
    local: Option<&'a NativeHistorySourceSummary>,
) -> Option<&'a NativeHistorySourceSummary> {
    match (db, local) {
        (Some(db), Some(local)) if local.updated_at_unix > db.updated_at_unix => Some(local),
        (Some(db), _) => Some(db),
        (None, Some(local)) => Some(local),
        (None, None) => None,
    }
}

fn native_history_status_action_lines(summary: &NativeHistorySourceSummary) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(role) = summary.primary_worker_role() {
        lines.push(next_line(
            &format!("/history role {role}"),
            "show saved native transcript for this role",
        ));
        if summary.native_sessions > 0 {
            lines.push(next_line(
                &format!("/continue open {role}"),
                "resume the role's native CLI conversation",
            ));
        }
    }
    if let Some(kind) = summary.primary_event_kind() {
        lines.push(next_line(
            &format!("/history kind {kind}"),
            native_history_kind_action_detail(kind),
        ));
    }
    if summary.worker_threads > 0 {
        lines.push(next_line(
            "/thread",
            "inspect role sessions, native ids, and handoff state",
        ));
    }
    lines
}

fn native_history_kind_action_detail(kind: &str) -> &'static str {
    let normalized = event_format::normalize_event_kind(kind).unwrap_or_else(|| kind.to_string());
    match normalized.as_str() {
        "approval" => "show saved approval requests",
        "question" => "show saved worker questions",
        "tool_result" => "show saved tool results",
        "tool_use" => "show saved tool calls",
        "diff" | "patch" => "show saved patches and diffs",
        "file_update" => "show saved file updates",
        "stderr" => "show saved worker errors",
        "answer" | "assistant" | "turn_complete" => "show saved worker answers",
        _ => "show saved process events of this kind",
    }
}

fn native_history_status_header(
    db: &Option<NativeHistorySourceSummary>,
    local: &Option<NativeHistorySourceSummary>,
) -> (&'static str, Color, &'static str) {
    match (db, local) {
        (Some(db), Some(local)) if local.updated_at_unix > db.updated_at_unix => {
            ("⚠", Color::Yellow, "local history is newer than session DB")
        }
        (Some(_), Some(_)) => ("✓", TIFFANY_BLUE, "session DB and local history available"),
        (Some(_), None) => ("✓", TIFFANY_BLUE, "session DB history available"),
        (None, Some(_)) => ("⚠", Color::Yellow, "local history not synced to session DB"),
        (None, None) => ("⚠", Color::Yellow, "no native history saved"),
    }
}

fn native_history_source_summary_lines(summary: &NativeHistorySourceSummary) -> Vec<Line<'static>> {
    let mut lines = vec![
        thread_meta_line(summary.label, "available"),
        detail_continuation_line(&format!(
            "{} turn(s) · {} event(s) · updated {}",
            summary.turns, summary.events, summary.updated_at_unix
        )),
    ];
    if !summary.worker_roles.is_empty() {
        lines.push(detail_continuation_line(&format!(
            "roles {}",
            compact_string_list(&summary.worker_roles, 4)
        )));
    }
    if summary.worker_threads > 0 || summary.native_sessions > 0 {
        lines.push(detail_continuation_line(&format!(
            "handoff {} worker thread(s) · {} native session(s)",
            summary.worker_threads, summary.native_sessions
        )));
    }
    if !summary.event_kinds.is_empty() {
        lines.push(detail_continuation_line(&format!(
            "events {}",
            compact_string_list(&summary.event_kinds, 6)
        )));
    }
    lines
}

fn compact_string_list(values: &[String], max: usize) -> String {
    let shown = values.iter().take(max).cloned().collect::<Vec<_>>();
    let hidden = values.len().saturating_sub(shown.len());
    if hidden == 0 {
        shown.join(", ")
    } else {
        format!("{} +{hidden} more", shown.join(", "))
    }
}

fn native_history_sync_output_lines(
    path: &Path,
    output: std::process::Output,
) -> Vec<Line<'static>> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let mut lines = vec![
            status_line("✗", Color::Red, "history", "sync failed"),
            thread_meta_line("source", &path.display().to_string()),
        ];
        for line in stdout.lines().chain(stderr.lines()) {
            if !line.trim().is_empty() {
                lines.push(body_line(line.trim_end(), true));
            }
        }
        return lines;
    }

    let text = format!("{stdout}\n{stderr}");
    let conversations = parse_prefixed_field(&text, "conversations:")
        .or_else(|| parse_inline_count(&text, "conversations"))
        .unwrap_or("unknown");
    let turns = parse_prefixed_field(&text, "turns:")
        .or_else(|| parse_inline_count(&text, "turns"))
        .unwrap_or("unknown");
    let events = parse_prefixed_field(&text, "events:")
        .or_else(|| parse_inline_count(&text, "events"))
        .unwrap_or("unknown");

    vec![
        status_line(
            "✓",
            TIFFANY_BLUE,
            "history",
            "synced native history to session DB",
        ),
        thread_meta_line("source", &path.display().to_string()),
        thread_meta_line("target", "session-db"),
        thread_meta_line(
            "imported",
            &format!("{conversations} conversation(s) · {turns} turn(s) · {events} event(s)"),
        ),
        next_line("/history", "read back from session-db"),
        next_line("/history compact", "review a handoff storyline"),
    ]
}

fn parse_inline_count<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let (_, rest) = line.split_once(label)?;
        let rest = rest
            .trim_start_matches(|ch: char| ch == ':' || ch == '=' || ch.is_whitespace())
            .trim();
        let value = rest.split_whitespace().next()?;
        (!value.is_empty()).then_some(value)
    })
}

async fn load_native_history_from_store(
    config: &TiffanyOrchestratorConfig,
    cwd: &Path,
) -> anyhow::Result<Option<TiffanyNativeChatConversation>> {
    let output = run_orchestrator_command(config, &native_history_store_args(cwd)).await?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "sessions native-history exited with {}; stdout: {}; stderr: {}",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(None);
    }
    serde_json::from_str::<TiffanyNativeChatConversation>(trimmed)
        .map(Some)
        .context("parsing sessions native-history JSON")
}

fn native_history_import_args(native_sessions_path: &Path) -> Vec<String> {
    vec![
        "sessions".to_string(),
        "import-native".to_string(),
        "--path".to_string(),
        native_sessions_path.display().to_string(),
    ]
}

fn native_history_store_args(cwd: &Path) -> Vec<String> {
    vec![
        "sessions".to_string(),
        "native-history".to_string(),
        "--cwd".to_string(),
        cwd_key(cwd),
    ]
}

fn orchestrator_command(bin: &str, config_path: Option<&str>) -> Command {
    let mut command = Command::new(normalized_orchestrator_bin(bin));
    if let Some(config_path) = config_path.filter(|path| !path.trim().is_empty()) {
        command.arg("--config").arg(config_path);
    }
    command
}

fn scoped_orchestrator_command(config: &TiffanyOrchestratorConfig) -> Command {
    let mut command = orchestrator_command(&config.bin, config.config_path.as_deref());
    if !config.worker_scope.trim().is_empty() {
        command.env("TIFFANY_WORKER_SCOPE", config.worker_scope.trim());
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

fn emit_continue_open_output(app_event_tx: &AppEventSender, output: std::process::Output) {
    if !output.status.success() {
        emit_command_output(
            app_event_tx,
            "continue",
            &["thread".to_string(), "show".to_string()],
            output,
        );
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match native_cli_command_from_thread_output(&stdout) {
        Some(command) => {
            let fields = parse_thread_fields(&stdout);
            let mut lines = vec![status_line(
                "●",
                TIFFANY_BLUE,
                "continue",
                &format!("opening {} native CLI", command.role),
            )];
            if let Some(runtime) = command.runtime.as_deref().or_else(|| {
                fields
                    .get("runtime")
                    .and_then(|value| nonempty_trimmed(value))
            }) {
                lines.push(thread_meta_line("runtime", runtime));
            }
            if let Some(provider) = fields
                .get("provider")
                .and_then(|value| nonempty_trimmed(value))
            {
                lines.push(thread_meta_line("provider", provider));
            }
            lines.push(thread_meta_line("native", &command.native_session));
            if let Some(thread) = fields
                .get("tiffany thread")
                .and_then(|value| nonempty_trimmed(value))
            {
                lines.push(thread_meta_line("thread", thread));
            }
            if let Some(last) = fields
                .get("last tiffany session")
                .and_then(|value| nonempty_trimmed(value))
            {
                lines.push(thread_meta_line("last", last));
            }
            lines.push(thread_meta_line("command", &command.command));
            if let Some(worktree) = &command.worktree {
                lines.push(thread_meta_line("work", worktree));
            }
            lines.push(thread_meta_line(
                "capture",
                "native transcript, git status, and diff return to Tiffany",
            ));
            lines.push(body_line(
                "Tiffany will pause and return after the native CLI exits.",
                true,
            ));
            lines.push(next_line(
                "/history status",
                "confirm captured native events after return",
            ));
            emit_lines(app_event_tx, lines);
            app_event_tx.send(AppEvent::TiffanyOrchestratorOpenNativeCli { command });
        }
        None => {
            let lines = continue_summary_lines(&stdout).unwrap_or_else(|| {
                vec![
                    status_line("⚠", Color::Yellow, "continue", "no native handoff found"),
                    body_line(
                        "Run a worker task first, then retry /continue open <role>.",
                        true,
                    ),
                ]
            });
            emit_lines(app_event_tx, lines);
        }
    }
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
    lines.extend(doctor_issue_group_lines(&text));
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
    command: String,
    detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DoctorIssueGroup {
    label: &'static str,
    detail: String,
}

fn doctor_issue_group_lines(text: &str) -> Vec<Line<'static>> {
    doctor_issue_groups(text)
        .into_iter()
        .map(|group| {
            Line::from(vec![
                Span::styled(
                    "  issue",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    group.label,
                    Style::default()
                        .fg(TIFFANY_SOFT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(group.detail, Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect()
}

fn doctor_issue_groups(text: &str) -> Vec<DoctorIssueGroup> {
    let mut provider_auth = Vec::new();
    let mut runtime = Vec::new();
    let mut role_wiring = Vec::new();
    let mut config = Vec::new();
    let mut persistence = Vec::new();
    let mut other = Vec::new();

    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !doctor_line_is_issue(line) {
            continue;
        }
        let normalized = doctor_issue_detail(line);
        let lower = normalized.to_ascii_lowercase();
        if doctor_issue_is_provider_auth(&lower) {
            provider_auth.push(normalized);
        } else if doctor_issue_is_runtime(&lower) {
            runtime.push(normalized);
        } else if doctor_issue_is_role_wiring(&lower, &normalized) {
            role_wiring.push(normalized);
        } else if doctor_issue_is_persistence(&lower) {
            persistence.push(normalized);
        } else if doctor_issue_is_config(&lower) {
            config.push(normalized);
        } else {
            other.push(normalized);
        }
    }

    let mut groups = Vec::new();
    push_doctor_issue_group(&mut groups, "provider auth", provider_auth);
    push_doctor_issue_group(&mut groups, "runtime", runtime);
    push_doctor_issue_group(&mut groups, "role wiring", role_wiring);
    push_doctor_issue_group(&mut groups, "persistence", persistence);
    push_doctor_issue_group(&mut groups, "config", config);
    push_doctor_issue_group(&mut groups, "other", other);
    groups
}

fn doctor_line_is_issue(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('✗') || trimmed.starts_with("- ")
}

fn doctor_issue_detail(line: &str) -> String {
    line.trim_start()
        .trim_start_matches('✗')
        .trim_start_matches("- ")
        .trim()
        .to_string()
}

fn doctor_issue_is_provider_auth(lower: &str) -> bool {
    lower.contains("api key missing")
        || lower.contains("api key env")
        || lower.contains("provider auth")
        || lower.contains("provider auth missing")
        || lower.contains("missing provider")
        || lower.contains("401")
        || lower.contains("403")
}

fn doctor_issue_is_runtime(lower: &str) -> bool {
    lower.contains("not found on path")
        || lower.contains("runtime binaries")
        || lower.contains("codex cli exec")
        || lower.contains("codex exec")
        || lower.contains("runtime type")
}

fn doctor_issue_is_role_wiring(lower: &str, original: &str) -> bool {
    lower.contains("missing role config")
        || lower.contains("no worker role")
        || lower.contains("role/model")
        || lower.contains("role/model wiring")
        || lower.contains("model `")
        || lower.contains("runtime `")
        || lower.contains("provider missing")
        || lower.contains("model not found")
        || lower.contains("unknown model")
        || lower.contains("invalid model")
        || lower.contains("1211")
        || original.contains("模型不存在")
}

fn doctor_issue_is_config(lower: &str) -> bool {
    lower.contains("config missing")
        || lower.contains("config parse/load failed")
        || lower.contains("provider `")
        || lower.contains("missing provider")
        || lower.contains("provider missing")
        || lower.contains("no models configured")
        || lower.contains("no runtimes configured")
}

fn doctor_issue_is_persistence(lower: &str) -> bool {
    lower.contains("session db")
        || lower.contains("db parent")
        || lower.contains("session_log_dir")
        || lower.contains("native history")
        || lower.contains("worker threads")
}

fn push_doctor_issue_group(
    groups: &mut Vec<DoctorIssueGroup>,
    label: &'static str,
    mut items: Vec<String>,
) {
    if items.is_empty() {
        return;
    }
    items.sort_by(|left, right| {
        doctor_issue_display_priority(label, left)
            .cmp(&doctor_issue_display_priority(label, right))
            .then_with(|| left.cmp(right))
    });
    items.dedup();
    groups.push(DoctorIssueGroup {
        label,
        detail: join_doctor_issue_details(&items, 3),
    });
}

fn doctor_issue_display_priority(label: &str, item: &str) -> u8 {
    if label != "role wiring" {
        return 2;
    }
    let lower = item.to_ascii_lowercase();
    if lower.contains("1211")
        || lower.contains("model not found")
        || lower.contains("unknown model")
        || lower.contains("invalid model")
        || item.contains("模型不存在")
    {
        return 0;
    }
    if lower.contains("provider missing") {
        return 1;
    }
    2
}

fn join_doctor_issue_details(items: &[String], limit: usize) -> String {
    let mut detail = items
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if items.len() > limit {
        detail.push_str(&format!("; +{} more", items.len() - limit));
    }
    truncate_text(&detail, 240)
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
    let mut actions = doctor_specific_repair_actions(text);

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
        if !actions
            .iter()
            .any(|action| action.command.starts_with("/provider edit "))
        {
            push_doctor_action(
                &mut actions,
                "/provider",
                "configure provider auth or endpoint",
            );
        }
    }
    if lower.contains("missing role config")
        || lower.contains("no worker role")
        || lower.contains("role/model")
        || lower.contains("model `")
        || lower.contains("runtime `")
        || lower.contains("model not found")
        || lower.contains("unknown model")
        || lower.contains("invalid model")
        || lower.contains("1211")
        || text.contains("模型不存在")
    {
        if !actions
            .iter()
            .any(|action| action.command.starts_with("/role "))
        {
            push_doctor_action(
                &mut actions,
                "/role",
                "fix role provider/model/runtime binding and API model name",
            );
        }
    }
    if lower.contains("not found on path")
        || lower.contains("claude: not found")
        || lower.contains("codex: not found")
        || lower.contains("runtime binaries")
    {
        if !actions
            .iter()
            .any(|action| action.command.starts_with("install runtime "))
        {
            push_doctor_action(
                &mut actions,
                "install runtime",
                "install claude/codex/gemini or change the role runtime",
            );
        }
    }
    if lower.contains("worker threads: none persisted yet")
        || lower.contains("native handoff starts after first worker run")
    {
        push_doctor_action(
            &mut actions,
            "/thread",
            "inspect per-role Tiffany/native session state after a worker run",
        );
        push_doctor_action(
            &mut actions,
            "/continue open <role>",
            "open the role's native CLI session when captured",
        );
    }
    if lower.contains("native history: not created yet")
        || lower.contains("session-db/local")
        || lower.contains("native history sync")
    {
        push_doctor_action(
            &mut actions,
            "/history status",
            "verify session DB and local native history sync",
        );
    }
    if lower.contains("session db path is not a file")
        || lower.contains("db parent missing")
        || lower.contains("session_log_dir")
    {
        push_doctor_action(
            &mut actions,
            "orchestrator setup",
            "repair writable session DB and log paths",
        );
    }
    if actions.is_empty() && lower.contains("all required checks passed") {
        push_doctor_action(&mut actions, "tiffany-loop", "start an orchestration run");
    }

    actions
}

fn doctor_specific_repair_actions(text: &str) -> Vec<DoctorRepairAction> {
    let mut actions = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !doctor_line_is_issue(line) {
            continue;
        }
        let issue = doctor_issue_detail(line);
        let lower = issue.to_ascii_lowercase();
        if doctor_issue_is_provider_auth(&lower) {
            if let Some(provider) =
                doctor_issue_missing_provider_target(&issue).or_else(|| doctor_issue_target(&issue))
            {
                let detail =
                    if lower.contains("missing provider") || lower.contains("provider missing") {
                        "create provider credentials before role binding"
                    } else {
                        "set API key/env or endpoint"
                    };
                push_doctor_action(&mut actions, format!("/provider edit {provider}"), detail);
            }
        }
        if lower.contains("provider missing")
            && let Some(provider) = doctor_issue_missing_provider_target(&issue)
        {
            push_doctor_action(
                &mut actions,
                format!("/provider edit {provider}"),
                "create provider credentials before role binding",
            );
        }
        if doctor_issue_is_role_wiring(&lower, &issue) {
            if let Some(role) = doctor_issue_role_target(&issue) {
                let detail = if lower.contains("1211") || issue.contains("模型不存在") {
                    "fix API model name for selected provider"
                } else {
                    "bind provider, API model, and runtime"
                };
                push_doctor_action(&mut actions, format!("/role {role}"), detail);
            }
        }
        if doctor_issue_is_runtime(&lower) {
            if let Some(runtime) = doctor_issue_runtime_target(&issue) {
                push_doctor_action(
                    &mut actions,
                    format!("install runtime {runtime}"),
                    format!("install {runtime} or change the role runtime"),
                );
            }
        }
    }
    actions
}

fn doctor_issue_target(issue: &str) -> Option<String> {
    let (target, _) = issue.split_once(':')?;
    normalize_doctor_target(target)
}

fn doctor_issue_role_target(issue: &str) -> Option<String> {
    let (target, _) = issue.split_once(':')?;
    let role = target
        .trim()
        .strip_suffix(" stderr")
        .or_else(|| target.trim().strip_suffix(" stdout"))
        .or_else(|| target.trim().strip_suffix(" output"))
        .unwrap_or_else(|| target.trim());
    normalize_doctor_target(role)
}

fn doctor_issue_missing_provider_target(issue: &str) -> Option<String> {
    let lower = issue.to_ascii_lowercase();
    if let Some(idx) = lower.find("missing provider `") {
        let start = idx + "missing provider `".len();
        let rest = &issue[start..];
        if let Some((provider, _)) = rest.split_once('`') {
            return normalize_doctor_target(provider);
        }
    }
    if lower.contains("provider missing")
        && let Some((_, rest)) = issue.split_once("->")
    {
        let provider = rest.trim().split('/').next()?.trim();
        return normalize_doctor_target(provider);
    }
    None
}

fn doctor_issue_runtime_target(issue: &str) -> Option<String> {
    let lower = issue.to_ascii_lowercase();
    for runtime in ["claude", "claude-code", "codex", "gemini"] {
        if lower.contains(&format!("{runtime}:"))
            || lower.contains(&format!("`{runtime}` not found"))
            || lower.contains(&format!("{runtime} cli"))
        {
            return Some(runtime.to_string());
        }
    }
    doctor_issue_target(issue)
        .filter(|target| matches!(target.as_str(), "claude" | "codex" | "gemini"))
}

fn normalize_doctor_target(target: &str) -> Option<String> {
    let target = target
        .trim()
        .trim_matches('`')
        .trim_start_matches("role ")
        .trim_start_matches("provider ");
    if target.is_empty() || target.contains(' ') || target.contains('/') || target.contains('\\') {
        return None;
    }
    if target
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        Some(target.to_string())
    } else {
        None
    }
}

fn push_doctor_action(
    actions: &mut Vec<DoctorRepairAction>,
    command: impl Into<String>,
    detail: impl Into<String>,
) {
    let command = command.into();
    if actions.iter().any(|action| action.command == command) {
        return;
    }
    let detail = detail.into();
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
        "continue" => concise_continue_success(command_args, output),
        "thread" => concise_thread_success(command_args, output),
        "jobs" => concise_jobs_success(command_args, output),
        _ => None,
    }
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
            let model_id = flag_value(command_args, "--model").unwrap_or("<model-id>");
            let api_model = flag_value(command_args, "--model-name").unwrap_or(model_id);
            let provider = flag_value(command_args, "--provider").unwrap_or("existing");
            let runtime = flag_value(command_args, "--runtime").unwrap_or("runtime");
            let target = if provider == "existing" {
                api_model.to_string()
            } else {
                format!("{provider}/{api_model}")
            };
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
                    &format!("saved {role} -> {target}"),
                ),
                body_line(&format!("provider: {provider}"), true),
                body_line(&format!("api model: {api_model}"), true),
                body_line(&format!("model id: {model_id}"), true),
                body_line(&format!("runtime: {runtime}"), true),
                body_line(&format!("agent teams: {teams}"), true),
                next_line(&format!("/roles show {role}"), "review saved binding"),
                next_line("/doctor", "verify provider/model/runtime wiring"),
            ])
        }
        Some("delete") => {
            let role = command_args.get(2).map(String::as_str).unwrap_or("<role>");
            Some(vec![
                status_line("✓", TIFFANY_BLUE, "role", &format!("deleted {role}")),
                body_line(
                    "kept models, providers, runtimes, worker threads, and history",
                    true,
                ),
                next_line("/roles", "review active role bindings"),
                next_line(
                    &format!("/thread clear {role}"),
                    "clear the saved native session separately",
                ),
                next_line("/doctor", "check for broken orchestration wiring"),
            ])
        }
        Some("profile") => role_profile_summary_lines(command_args, output),
        Some("options") => role_options_lines(&String::from_utf8_lossy(&output.stdout)),
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
            lines.push(next_line(
                "/role worker-cc",
                "bind a worker role to this provider API model",
            ));
            lines.push(next_line(
                "/roles profile",
                "bind planner, critic, worker, and reviewer together",
            ));
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

fn concise_jobs_success(
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    if command_args.first().map(String::as_str) != Some("jobs") {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if is_jobs_retry_command(command_args)
        && let Some(mut lines) = jobs_retry_handoff_lines(&stdout)
    {
        if let Some(summary) = jobs_summary_lines(&stdout) {
            lines.extend(summary);
        }
        return Some(lines);
    }
    if is_jobs_recover_command(command_args)
        && let Some(mut lines) = jobs_recover_result_lines(&stdout)
    {
        if let Some(summary) = jobs_summary_lines(&stdout) {
            lines.extend(summary);
        }
        return Some(lines);
    }
    jobs_summary_lines(&stdout)
}

fn is_jobs_retry_command(command_args: &[String]) -> bool {
    command_args.first().map(String::as_str) == Some("jobs")
        && command_args.get(1).map(String::as_str) == Some("retry")
}

fn is_jobs_recover_command(command_args: &[String]) -> bool {
    command_args.first().map(String::as_str) == Some("jobs")
        && command_args.get(1).map(String::as_str) == Some("recover")
}

fn jobs_recover_result_lines(text: &str) -> Option<Vec<Line<'static>>> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let header = lines.find(|line| *line == "Recovered stale jobs")?;
    let detail = lines
        .find(|line| !line.starts_with("Jobs") && !line.starts_with("active:"))
        .unwrap_or_default();
    let mut out = Vec::new();
    if detail.starts_with("none older than") {
        out.push(status_line(
            "✓",
            TIFFANY_BLUE,
            "jobs",
            "no stale running jobs found",
        ));
        out.push(body_line(detail, true));
        out.push(next_line(
            "/jobs",
            "refresh queue status or keep current running jobs active",
        ));
        return Some(out);
    }

    let failed_count = parse_recovered_jobs_failed_count(detail).unwrap_or(0);
    let ids = parse_recovered_jobs_ids(detail);
    let message = if failed_count > 0 {
        format!("recovered stale running job(s) · {failed_count} marked failed")
    } else {
        header.to_ascii_lowercase()
    };
    out.push(status_line("⚠", Color::Yellow, "jobs", &message));
    if !detail.is_empty() {
        out.push(body_line(detail, true));
    }
    if let Some(first_id) = ids.first() {
        out.push(next_line(
            &format!("/jobs retry {first_id}"),
            "retry a recovered stale job",
        ));
    }
    out.push(next_line("/jobs", "refresh persisted jobs"));
    Some(out)
}

fn jobs_retry_handoff_lines(text: &str) -> Option<Vec<Line<'static>>> {
    let handoff = parse_jobs_retry_handoff(text)?;
    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "jobs",
        &format!("retry queued · {}", handoff.job_id),
    )];
    lines.push(session_card_header_line(
        "↳",
        TIFFANY_BLUE,
        "current TUI queue",
        "retry",
        &handoff.job_id,
        "ready",
    ));
    lines.push(session_card_detail_line("task", &handoff.prompt));
    lines.push(session_card_detail_line(
        "state",
        "restored from persisted job; runs as the next Tiffany prompt",
    ));
    lines.push(session_card_detail_line(
        "prompt",
        &format!(
            "{} line(s) restored into the current input queue",
            handoff.restored_prompt_lines
        ),
    ));
    lines.push(session_card_actions_line(&[
        "/queue run".to_string(),
        "/queue show".to_string(),
        format!("/jobs show {}", handoff.job_id),
        "/jobs".to_string(),
    ]));
    lines.push(next_line(
        "/queue run",
        "start the restored retry prompt now",
    ));
    lines.push(next_line(
        "/queue show",
        "review queued prompts before running",
    ));
    Some(lines)
}

fn concise_continue_success(
    command_args: &[String],
    output: &std::process::Output,
) -> Option<Vec<Line<'static>>> {
    if command_args.first().map(String::as_str) != Some("thread")
        || command_args.get(1).map(String::as_str) != Some("show")
    {
        return None;
    }
    continue_summary_lines(&String::from_utf8_lossy(&output.stdout))
}

fn continue_summary_lines(text: &str) -> Option<Vec<Line<'static>>> {
    if !text.contains("Worker thread") {
        return None;
    }
    let fields = parse_thread_fields(text);
    let role = fields
        .get("role")
        .cloned()
        .or_else(|| parse_worker_thread_title(text))
        .unwrap_or_else(|| "worker".to_string());
    let native = fields
        .get("native session")
        .and_then(|value| nonempty_trimmed(value))
        .unwrap_or("none");
    let handoff = fields
        .get("native handoff")
        .and_then(|value| nonempty_trimmed(value));
    let resume = fields
        .get("native resume")
        .and_then(|value| nonempty_trimmed(value));

    let Some(command) = handoff.or(resume).filter(|command| *command != "none") else {
        return Some(vec![
            status_line(
                "⚠",
                Color::Yellow,
                "continue",
                &format!("{role} has no native session yet"),
            ),
            thread_meta_line("native", native),
            next_line(
                "run a task",
                "first successful worker run creates a resumable native session",
            ),
            next_line(
                &format!("/role {role}"),
                "verify provider/model/runtime binding",
            ),
            next_line(&format!("/thread {role}"), "inspect thread state"),
        ]);
    };

    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "continue",
        &format!("{role} native handoff ready"),
    )];
    if let Some(runtime) = fields
        .get("runtime")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(thread_meta_line("runtime", runtime));
    }
    if let Some(provider) = fields
        .get("provider")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(thread_meta_line("provider", provider));
    }
    lines.push(thread_meta_line("native", native));
    if let Some(thread) = fields
        .get("tiffany thread")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(thread_meta_line("thread", thread));
    }
    if let Some(last) = fields
        .get("last tiffany session")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(thread_meta_line("last", last));
    }
    lines.push(thread_meta_line("command", command));
    if let Some(legacy) = fields
        .get("legacy handoff")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(thread_meta_line("legacy", legacy));
    }
    if let Some(worktree) = fields
        .get("worktree")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(thread_meta_line("work", worktree));
    }
    lines.push(thread_meta_line(
        "capture",
        "native transcript, git status, and diff return to Tiffany",
    ));
    lines.push(next_line(
        "copy command",
        "paste it in a shell to continue in the native CLI",
    ));
    lines.push(next_line(
        &format!("/continue open {role}"),
        "open the native CLI from Tiffany",
    ));
    lines.push(next_line(
        &format!("/thread export {role}"),
        "write full selectable handoff history",
    ));
    Some(lines)
}

fn native_cli_command_from_thread_output(text: &str) -> Option<TiffanyNativeCliCommand> {
    let handoff = parse_native_cli_handoff(text)?;
    Some(TiffanyNativeCliCommand {
        role: handoff.role,
        runtime: handoff.runtime,
        worker_thread_id: handoff.worker_thread_id,
        native_session: handoff.native_session,
        command: handoff.command,
        worktree: handoff.worktree,
    })
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
    for row in rows.iter().take(8) {
        lines.extend(thread_summary_card_lines(row));
    }
    if rows.len() > 8 {
        lines.push(body_line(&format!("… {} more", rows.len() - 8), true));
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
        "/history role <role>",
        "show native events for one worker role",
    ));
    lines.push(next_line(
        "/history kind answer|tool_result|diff|approval|session_recovery",
        "filter saved native events by stream kind",
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
    let has_native = native != "none";
    let symbol = if has_native { "✓" } else { "⚠" };
    let color = if has_native {
        TIFFANY_BLUE
    } else {
        Color::Yellow
    };
    let runtime = fields
        .get("runtime")
        .map(String::as_str)
        .and_then(nonempty_trimmed)
        .unwrap_or("runtime");
    let model = fields
        .get("model")
        .map(String::as_str)
        .and_then(nonempty_trimmed)
        .unwrap_or("model");
    let mut lines = vec![
        status_line(symbol, color, "thread", &format!("{role} session card")),
        session_card_header_line(symbol, color, &role, runtime, model, &status),
    ];
    if let Some(agent) = fields
        .get("agent")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_detail_line("agent", agent));
    }
    if let Some(provider) = fields
        .get("provider")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_detail_line("provider", provider));
    }
    if let Some(scope) = fields
        .get("scope")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_detail_line("scope", scope));
    }
    lines.push(session_card_detail_line("native", &native));
    if let Some(thread) = fields
        .get("tiffany thread")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_detail_line("tiffany", thread));
    }
    if let Some(last) = fields
        .get("last tiffany session")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_detail_line("last", last));
    }
    if let Some(worktree) = fields
        .get("worktree")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_detail_line("work", worktree));
    }
    if let Some(resume) = fields
        .get("native resume")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_command_line("resume", resume));
    }
    if let Some(handoff) = fields
        .get("native handoff")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_command_line("handoff", handoff));
    }
    if let Some(tui_resume) = fields
        .get("tui resume")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_command_line("tui", tui_resume));
    }
    if let Some(legacy) = fields
        .get("legacy handoff")
        .and_then(|value| nonempty_trimmed(value))
    {
        lines.push(session_card_command_line("legacy", legacy));
    }

    next_line_into(
        &mut lines,
        &format!("/history role {role}"),
        "show this role's native event stream",
    );
    next_line_into(
        &mut lines,
        "/history kind answer|tool_result|diff|approval|session_recovery",
        "filter answer, tool, diff, approval, or recovery events",
    );
    if let Some(thread_id) = fields
        .get("tiffany thread")
        .and_then(|value| nonempty_trimmed(value))
    {
        next_line_into(
            &mut lines,
            &format!(
                "/history thread {}",
                short_task_id(Some(thread_id)).unwrap_or(thread_id)
            ),
            "show events for this Tiffany worker thread",
        );
    }

    if !has_native {
        next_line_into(
            &mut lines,
            &format!("/role {role}"),
            "verify provider/model/runtime binding",
        );
        next_line_into(
            &mut lines,
            "/thread clear <role>",
            "already fresh; run a task to capture native session",
        );
    } else {
        let tui_resume = fields
            .get("tui resume")
            .and_then(|value| nonempty_trimmed(value))
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("/continue open {role}"));
        next_line_into(&mut lines, &tui_resume, "open native CLI from Tiffany");
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

fn jobs_summary_lines(text: &str) -> Option<Vec<Line<'static>>> {
    if !text.lines().any(|line| line.trim() == "Jobs") {
        return None;
    }

    let jobs = parse_job_summaries(text);
    if jobs.is_empty() {
        if text.contains("no persisted jobs yet") {
            return Some(vec![
                status_line("✓", TIFFANY_BLUE, "jobs", "queue is empty"),
                body_line("queued and background runs will appear here", true),
                next_line("send a prompt", "create a new Tiffany orchestration job"),
            ]);
        }
        return None;
    }

    let (active, shown) = parse_jobs_header_counts(text).unwrap_or_else(|| {
        (
            jobs.iter().filter(|job| job.status == "running").count(),
            jobs.len(),
        )
    });
    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "jobs",
        &format!("{active} active · {shown} shown"),
    )];
    for job in jobs.iter().take(10) {
        lines.extend(job_summary_card_lines(job));
    }
    if jobs.len() > 10 {
        lines.push(body_line(&format!("… {} more", jobs.len() - 10), true));
    }
    lines.push(next_line("/jobs 50", "show more persisted jobs"));
    lines.push(next_line(
        "/thread <role>",
        "open the worker's resumable native session",
    ));
    lines.push(next_line(
        "/history role <role>",
        "inspect full worker output, tools, diff, and approvals",
    ));
    Some(lines)
}

fn job_summary_card_lines(job: &JobSummary) -> Vec<Line<'static>> {
    let (symbol, color, status) = job_status_style(&job.status);
    let role = job.role.as_deref().unwrap_or("worker");
    let flow = job.flow.as_deref().unwrap_or("flow");
    let mut lines = vec![session_card_header_line(
        symbol, color, role, flow, &job.id, status,
    )];
    if let Some(prompt) = nonempty_trimmed(&job.prompt) {
        lines.push(session_card_detail_line("task", prompt));
    }
    lines.push(session_card_detail_line("state", &job_state_summary(job)));
    if let Some(task) = job.task.as_deref().and_then(nonempty_trimmed) {
        lines.push(session_card_detail_line("task id", task));
    }
    if let Some(timing) = job.timing.as_deref().and_then(nonempty_trimmed) {
        lines.push(session_card_detail_line("timing", timing));
    }
    if let Some(session) = job.session.as_deref().and_then(nonempty_trimmed) {
        lines.push(session_card_detail_line("session", session));
    }
    if let Some(thread) = job.thread.as_deref().and_then(nonempty_trimmed) {
        lines.push(session_card_detail_line("thread", thread));
    }
    if let Some(native) = job.native.as_deref().and_then(nonempty_trimmed) {
        lines.push(session_card_detail_line("native", native));
        if let Some(role) = job.role.as_deref().and_then(nonempty_trimmed) {
            lines.push(session_card_detail_line(
                "handoff",
                &format!("ready; use /continue open {role} to enter the native CLI"),
            ));
        }
    }
    if let Some(result) = job.result.as_deref().and_then(nonempty_trimmed) {
        lines.push(session_card_detail_line("result", result));
    }
    if let Some(error) = job.error.as_deref().and_then(nonempty_trimmed) {
        lines.push(session_card_detail_line("error", error));
    }
    lines.push(session_card_actions_line(&job_actions(job)));
    lines
}

fn job_status_style(status: &str) -> (&'static str, Color, &'static str) {
    match status {
        "done" => ("✓", TIFFANY_BLUE, "done"),
        "running" => ("●", TIFFANY_BLUE, "running"),
        "queued" => ("↳", TIFFANY_SOFT, "queued"),
        "failed" => ("✗", Color::Red, "failed"),
        "cancelled" => ("○", Color::DarkGray, "cancelled"),
        "removed" => ("○", Color::DarkGray, "removed"),
        "skipped" => ("○", Color::DarkGray, "skipped"),
        _ => ("·", Color::Gray, "saved"),
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
        lines.extend(role_summary_card_lines(role));
    }
    if roles.len() > 10 {
        lines.push(body_line(&format!("… {} more", roles.len() - 10), true));
    }
    lines.push(next_line("/role <name>", "edit one role binding"));
    lines.push(next_line("/doctor", "verify the orchestration chain"));
    Some(lines)
}

fn role_options_lines(text: &str) -> Option<Vec<Line<'static>>> {
    if !text.lines().any(|line| line.trim() == "Role options") {
        return None;
    }
    let options = parse_role_option_summaries(text);
    let mut lines = vec![status_line(
        "✓",
        TIFFANY_BLUE,
        "role",
        &format!("{} model option(s)", options.len()),
    )];
    let providers = parse_prefixed_field(text, "providers:");
    let runtimes = parse_prefixed_field(text, "runtimes:");
    if let Some(providers) = providers {
        lines.push(session_card_detail_line("providers", providers));
    }
    if let Some(runtimes) = runtimes {
        lines.push(session_card_detail_line("runtimes", runtimes));
    }
    for option in options.iter().take(10) {
        lines.extend(role_option_card_lines(option));
    }
    if options.len() > 10 {
        lines.push(body_line(&format!("… {} more", options.len() - 10), true));
    }
    lines.push(next_line(
        "/role <name>",
        "open role editor with provider/model/runtime dropdowns",
    ));
    lines.push(next_line(
        "/roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime>",
        "save one binding directly",
    ));
    lines.push(next_line("/doctor", "verify provider/model/runtime wiring"));
    Some(lines)
}

fn role_option_card_lines(option: &RoleOptionSummary) -> Vec<Line<'static>> {
    let ready = option.health == "ready";
    let symbol = if ready { "✓" } else { "⚠" };
    let color = if ready { TIFFANY_BLUE } else { Color::Yellow };
    let mut lines = vec![session_card_header_line(
        symbol,
        color,
        &option.model,
        &option.provider,
        &option.api_model,
        &option.health,
    )];
    lines.push(session_card_detail_line("runtimes", &option.runtimes));
    lines.push(session_card_detail_line("roles", &option.roles));
    lines.push(session_card_detail_line("teams", &option.teams));
    let first_role = option.roles.split(',').next().unwrap_or("<role>").trim();
    let first_runtime = option
        .runtimes
        .split(',')
        .next()
        .unwrap_or("<runtime>")
        .trim();
    lines.push(session_card_actions_line(&[
        format!(
            "/roles register {first_role} --provider {} --model-name {} --runtime {first_runtime}",
            option.provider, option.api_model
        ),
        format!("/provider edit {}", option.provider),
        "/doctor".to_string(),
    ]));
    lines
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

fn role_summary_card_lines(role: &RoleSummary) -> Vec<Line<'static>> {
    let target = role_target_label(role);
    let teams = if role.teams { "teams on" } else { "teams off" };
    let (symbol, color, health) = role_health_status(role);
    let model = role
        .api_model
        .as_deref()
        .filter(|value| !value.trim().is_empty() && *value != "-")
        .or(role.display_model.as_deref())
        .unwrap_or(&role.model);
    let mut lines = vec![session_card_header_line(
        symbol,
        color,
        &role.name,
        &role.runtime,
        model,
        &health,
    )];
    lines.push(session_card_detail_line("model id", &role.model));
    lines.push(session_card_detail_line("api model", &target));
    lines.push(session_card_detail_line("mode", teams));
    if is_worker_role_name(&role.name) {
        lines.push(session_card_detail_line(
            "session",
            &role_session_label(role),
        ));
    }
    let actions = if is_worker_role_name(&role.name) {
        vec![
            format!("/role {}", role.name),
            format!("/thread {}", role.name),
            format!("/continue open {}", role.name),
            format!("/history role {}", role.name),
        ]
    } else {
        vec![
            format!("/role {}", role.name),
            "/roles profile".to_string(),
            "/doctor".to_string(),
        ]
    };
    lines.push(session_card_actions_line(&actions));
    lines
}

fn role_session_label(role: &RoleSummary) -> String {
    let native = role
        .native
        .as_deref()
        .and_then(nonempty_trimmed)
        .filter(|value| *value != "none" && *value != "-");
    let thread = role
        .thread
        .as_deref()
        .and_then(nonempty_trimmed)
        .filter(|value| *value != "none" && *value != "-");
    let last = role
        .last
        .as_deref()
        .and_then(nonempty_trimmed)
        .filter(|value| *value != "none" && *value != "-");
    match (native, thread, last) {
        (Some(native), Some(thread), Some(last)) => {
            format!("native {native} · thread {thread} · last {last}")
        }
        (Some(native), Some(thread), None) => format!("native {native} · thread {thread}"),
        (Some(native), None, _) => format!("native {native} · /thread {}", role.name),
        (None, Some(thread), _) => format!("thread {thread} · native pending"),
        (None, None, _) => "pending; first worker run creates native session".to_string(),
    }
}

fn thread_summary_card_lines(thread: &ThreadSummary) -> Vec<Line<'static>> {
    let (symbol, color) = if thread.active {
        ("✓", TIFFANY_BLUE)
    } else {
        ("○", Color::DarkGray)
    };
    let mut lines = vec![session_card_header_line(
        symbol,
        color,
        &thread.role,
        &thread.runtime,
        if thread.model.is_empty() {
            "unbound"
        } else {
            &thread.model
        },
        if thread.active {
            "native session ready"
        } else {
            "no worker thread yet"
        },
    )];
    if thread.active {
        if let Some(thread_id) = thread.thread.as_deref().and_then(nonempty_trimmed) {
            lines.push(session_card_detail_line("tiffany", thread_id));
        }
        lines.push(session_card_detail_line(
            "native",
            thread.native.as_deref().unwrap_or("none"),
        ));
        if let Some(last) = thread.last.as_deref().and_then(nonempty_trimmed) {
            lines.push(session_card_detail_line("last", last));
        }
        lines.push(session_card_actions_line(&[
            format!("/thread {}", thread.role),
            format!("/continue open {}", thread.role),
            format!("/history role {}", thread.role),
            format!("/thread export {}", thread.role),
        ]));
    } else {
        lines.push(session_card_detail_line(
            "state",
            "run a task to create a resumable native session",
        ));
        lines.push(session_card_actions_line(&[
            format!("/thread {}", thread.role),
            format!("/role {}", thread.role),
        ]));
    }
    lines
}

fn session_card_header_line(
    symbol: &'static str,
    color: Color,
    role: &str,
    runtime: &str,
    model: &str,
    status: &str,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {symbol} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            truncate_text(role, 24),
            Style::default()
                .fg(TIFFANY_SOFT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(truncate_text(runtime, 18), Style::default().fg(Color::Gray)),
        Span::styled("  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(truncate_text(model, 34), Style::default()),
        Span::styled("  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(truncate_text(status, 72), Style::default().fg(color)),
    ])
}

fn session_card_detail_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("    ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            truncate_text(value, 180),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn session_card_command_line(label: &str, command: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("    ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            truncate_text(command, 220),
            Style::default().fg(TIFFANY_SOFT),
        ),
    ])
}

fn session_card_actions_line(actions: &[String]) -> Line<'static> {
    Line::from(vec![
        Span::styled("    ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            "actions ",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            truncate_text(&actions.join("  ·  "), 220),
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
    if auth.eq_ignore_ascii_case("not-required")
        || auth.eq_ignore_ascii_case("not_required")
        || auth.eq_ignore_ascii_case("not required")
    {
        return ("●", TIFFANY_BLUE, "not needed", "ready");
    }
    if auth.is_empty()
        || auth == "-"
        || auth == "—"
        || auth.eq_ignore_ascii_case("none")
        || auth.eq_ignore_ascii_case("missing")
        || auth.eq_ignore_ascii_case("(empty)")
    {
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

    if let Some(hint) = event_format::agent_failure_hint(&detail_text, CONTROL_SUMMARY_MAX_CHARS) {
        lines.push(failure_hint_line(hint.title(), hint.action()));
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
                self.final_output_visible = true;
            }
            let key = output_seen_key(&event, content, self.pending_tool_call_output.as_ref());
            if !self.seen_outputs.insert(key) {
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
            let Some(content) = visible else {
                return;
            };

            if is_worker_answer_stream_output(&event, &content) {
                self.flush_timeline(app_event_tx);
                self.flush_pending_tool_call(app_event_tx);
                if let Some(delta) = self.answer_stream_delta(&event, &content) {
                    app_event_tx.send(AppEvent::TiffanyOrchestratorAnswerDelta { delta });
                }
                self.final_output_visible = true;
                return;
            }

            if is_pending_tool_call_output(&event) {
                self.flush_timeline(app_event_tx);
                self.flush_pending_tool_call(app_event_tx);
                self.pending_tool_call_output = Some((event, content));
                return;
            }

            self.flush_timeline(app_event_tx);
            if is_tool_result_output(&event)
                && let Some((tool_event, tool_content)) = self.pending_tool_call_output.take()
            {
                if same_worker_tool_sequence(&tool_event, &event) {
                    emit_lines(
                        app_event_tx,
                        worker_tool_pair_event_lines(&tool_event, &tool_content, &event, &content),
                    );
                } else {
                    emit_event_lines(app_event_tx, &tool_event, Some(tool_content));
                    emit_event_lines(app_event_tx, &event, Some(content));
                }
                return;
            }

            self.flush_pending_tool_call(app_event_tx);
            emit_event_lines(app_event_tx, &event, Some(content));
        } else {
            if self.pending_tool_call_output.is_some() {
                self.flush_timeline(app_event_tx);
                self.flush_pending_tool_call(app_event_tx);
            }
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

    fn flush_pending_tool_call(&mut self, app_event_tx: &AppEventSender) {
        let Some((event, content)) = self.pending_tool_call_output.take() else {
            return;
        };
        emit_event_lines(app_event_tx, &event, Some(content));
    }

    fn answer_stream_delta(
        &mut self,
        event: &TiffanyProgressEvent,
        visible: &str,
    ) -> Option<String> {
        let candidate = worker_answer_stream_candidate(event, visible)?;
        let (merged, delta) =
            merge_streaming_answer_delta(self.streamed_answer_text.as_deref(), &candidate);
        if merged.trim().is_empty() {
            return None;
        }
        self.streamed_answer_text = Some(merged);
        if delta.is_empty() {
            return None;
        }
        self.answer_stream_active = true;
        Some(delta)
    }

    fn finish_answer_stream(&mut self, app_event_tx: &AppEventSender) {
        if self.answer_stream_active {
            app_event_tx.send(AppEvent::TiffanyOrchestratorAnswerFinished);
            self.answer_stream_active = false;
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
    let kind = native_event_kind_from_progress(event);
    normalize_native_event(TiffanyNativeChatEvent {
        role: event.role.clone(),
        status: event.status.clone(),
        title,
        kind,
        content: visible
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        agent: event.agent.clone(),
        worker_role: event.worker_role.clone(),
        model: event.model.clone(),
        provider: event.provider.clone(),
        task_id: event.task_id.clone(),
        worker_thread_id: event.worker_thread_id.clone(),
        native_session_id: event.native_session_id.clone(),
    })
}

fn native_event_kind_from_progress(event: &TiffanyProgressEvent) -> Option<String> {
    if event.status != "output" {
        return Some(event.status.clone());
    }
    if event.role == "worker" {
        if event
            .recovery
            .as_deref()
            .and_then(nonempty_trimmed)
            .is_some()
        {
            return Some("session_recovery".to_string());
        }
        return worker_visible_output_kind(event)
            .map(native_event_kind_label)
            .map(str::to_string)
            .or_else(|| Some("output".to_string()));
    }
    Some(
        match event.role.as_str() {
            "planner" => "plan",
            "critic" => "critique",
            "reviewer" => "review",
            _ => "output",
        }
        .to_string(),
    )
}

fn native_event_kind_label(kind: event_format::VisibleAgentOutputKind) -> &'static str {
    match kind {
        event_format::VisibleAgentOutputKind::Final => "final",
        event_format::VisibleAgentOutputKind::Answer => "answer",
        event_format::VisibleAgentOutputKind::Question => "question",
        event_format::VisibleAgentOutputKind::Approval => "approval",
        event_format::VisibleAgentOutputKind::ToolCall => "tool_call",
        event_format::VisibleAgentOutputKind::ToolResult => "tool_result",
        event_format::VisibleAgentOutputKind::Diff => "diff",
        event_format::VisibleAgentOutputKind::Patch => "patch",
        event_format::VisibleAgentOutputKind::FileUpdate => "file_update",
        event_format::VisibleAgentOutputKind::Stderr => "stderr",
        event_format::VisibleAgentOutputKind::Actionable => "alert",
        event_format::VisibleAgentOutputKind::Normal => "output",
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

    let events = compact_timeline_events(events);
    let mut lines = vec![timeline_header_line(&events)];
    for event in events {
        for line in event_timeline_detail_lines(event) {
            lines.push(timeline_body_line(line));
        }
    }
    lines
}

fn compact_timeline_events(events: &[TiffanyProgressEvent]) -> Vec<&TiffanyProgressEvent> {
    let has_route_update = events.iter().any(is_route_update_event);
    let has_worker_failure = events
        .iter()
        .any(|event| event.role == "worker" && event.status == "failed");
    events
        .iter()
        .filter(|event| !(has_route_update && is_route_selected_event(event)))
        .filter(|event| !(has_worker_failure && is_zero_worker_done_event(event)))
        .collect()
}

fn is_zero_worker_done_event(event: &TiffanyProgressEvent) -> bool {
    event.role == "orchestrator" && event.status == "done" && event.count == Some(0)
}

fn event_timeline_detail_lines(event: &TiffanyProgressEvent) -> Vec<Line<'static>> {
    let mut lines = vec![waterfall_status_line(event)];
    lines.extend(event_detail_lines(event));
    lines
}

fn timeline_header_line(events: &[&TiffanyProgressEvent]) -> Line<'static> {
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

fn timeline_header_symbol(events: &[&TiffanyProgressEvent]) -> (&'static str, Color) {
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

fn timeline_flow_label(events: &[&TiffanyProgressEvent]) -> Option<String> {
    for event in events.iter().rev() {
        if let Some(flow) = event.flow_steps.as_deref().and_then(nonempty_trimmed) {
            return Some(flow.to_string());
        }
    }
    None
}

fn is_route_selected_event(event: &TiffanyProgressEvent) -> bool {
    event.role == "orchestrator"
        && event.route.is_some()
        && event.message.contains("route selected")
}

fn is_route_update_event(event: &TiffanyProgressEvent) -> bool {
    event.role == "orchestrator" && event.route.is_some() && event.message.contains("route updated")
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
    let kind = event
        .content
        .as_deref()
        .and_then(|content| worker_visible_output_kind_for_event(event, content));
    let (symbol, color) = worker_output_kind_marker(kind);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            symbol,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            output_title(event),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])];
    lines.extend(worker_output_body_lines(content, kind));
    if matches!(
        kind,
        Some(event_format::VisibleAgentOutputKind::Question)
            | Some(event_format::VisibleAgentOutputKind::Approval)
    ) {
        lines.extend(worker_question_hint_lines(event, kind));
    }
    if let Some(raw) = event.content.as_deref()
        && let Some(hint) = event_format::agent_failure_hint(raw, CONTROL_SUMMARY_MAX_CHARS)
    {
        lines.push(failure_hint_line(hint.title(), hint.action()));
    }
    lines
}

fn is_pending_tool_call_output(event: &TiffanyProgressEvent) -> bool {
    event.role == "worker"
        && event.status == "output"
        && worker_visible_output_kind(event) == Some(event_format::VisibleAgentOutputKind::ToolCall)
}

fn is_tool_result_output(event: &TiffanyProgressEvent) -> bool {
    event.role == "worker"
        && event.status == "output"
        && worker_visible_output_kind(event)
            == Some(event_format::VisibleAgentOutputKind::ToolResult)
}

fn same_worker_tool_sequence(left: &TiffanyProgressEvent, right: &TiffanyProgressEvent) -> bool {
    left.role == right.role
        && left.agent == right.agent
        && left.worker_role == right.worker_role
        && left.task_id == right.task_id
}

fn worker_tool_pair_event_lines(
    _call_event: &TiffanyProgressEvent,
    call_content: &str,
    result_event: &TiffanyProgressEvent,
    result_content: &str,
) -> Vec<Line<'static>> {
    let label = output_label(result_event);
    let title = worker_tool_pair_title(&label, call_content);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "↳",
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            title,
            Style::default()
                .fg(TIFFANY_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let call_key = normalized_output_key(call_content);
    let result_key = normalized_output_key(result_content);
    if call_key == result_key {
        lines.extend(worker_output_body_lines(
            result_content,
            Some(event_format::VisibleAgentOutputKind::ToolResult),
        ));
    } else {
        lines.extend(worker_output_body_lines(
            call_content,
            Some(event_format::VisibleAgentOutputKind::ToolCall),
        ));
        lines.extend(worker_output_body_lines(
            result_content,
            Some(event_format::VisibleAgentOutputKind::ToolResult),
        ));
    }
    if let Some(raw) = result_event.content.as_deref()
        && let Some(hint) = event_format::agent_failure_hint(raw, CONTROL_SUMMARY_MAX_CHARS)
    {
        lines.push(failure_hint_line(hint.title(), hint.action()));
    }
    lines
}

fn worker_tool_pair_title(label: &str, call_content: &str) -> String {
    match worker_tool_display_name(call_content) {
        Some(tool) => format!("worker {tool} · {label}"),
        None => format!("worker tool · {label}"),
    }
}

fn worker_tool_display_name(content: &str) -> Option<String> {
    let first = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let lower = first.to_ascii_lowercase();
    let rest = lower
        .starts_with("tool ")
        .then(|| &first["tool ".len()..])
        .or_else(|| {
            lower
                .starts_with("tool: ")
                .then(|| &first["tool: ".len()..])
        })?;
    let name = rest
        .split_once(':')
        .map(|(name, _)| name)
        .unwrap_or_else(|| rest.split_whitespace().next().unwrap_or(rest))
        .trim()
        .trim_matches('`');
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return None;
    }
    Some(name.to_string())
}

fn worker_output_body_lines(
    content: &str,
    kind: Option<event_format::VisibleAgentOutputKind>,
) -> Vec<Line<'static>> {
    let lines = worker_visible_process_lines(content, kind);
    lines
        .iter()
        .enumerate()
        .map(|(idx, line)| output_body_line_for_kind(line, kind, idx == 0))
        .collect()
}

fn worker_visible_process_lines(
    content: &str,
    kind: Option<event_format::VisibleAgentOutputKind>,
) -> Vec<String> {
    let lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    match kind {
        Some(event_format::VisibleAgentOutputKind::Answer)
        | Some(event_format::VisibleAgentOutputKind::Final)
        | Some(event_format::VisibleAgentOutputKind::Question)
        | Some(event_format::VisibleAgentOutputKind::Approval)
        | Some(event_format::VisibleAgentOutputKind::Diff)
        | Some(event_format::VisibleAgentOutputKind::Patch)
        | Some(event_format::VisibleAgentOutputKind::Stderr)
        | Some(event_format::VisibleAgentOutputKind::Actionable)
        | Some(event_format::VisibleAgentOutputKind::ToolResult)
        | Some(event_format::VisibleAgentOutputKind::Normal)
        | None => lines,
        Some(event_format::VisibleAgentOutputKind::ToolCall) => {
            compact_process_lines(lines, 12, "tool_use")
        }
        Some(event_format::VisibleAgentOutputKind::FileUpdate) => {
            compact_process_lines(lines, 12, "file_update")
        }
    }
}

fn compact_process_lines(lines: Vec<String>, max_lines: usize, history_kind: &str) -> Vec<String> {
    if lines.len() <= max_lines {
        return lines;
    }
    if max_lines < 4 {
        return lines.into_iter().take(max_lines).collect();
    }

    let head = (max_lines / 2).max(2);
    let tail = max_lines.saturating_sub(head + 1).max(1);
    let hidden = lines.len().saturating_sub(head + tail);
    let mut out = Vec::with_capacity(max_lines);
    out.extend(lines.iter().take(head).cloned());
    let process = process_alias_for_history_kind(history_kind);
    out.push(format!(
        "… {hidden} line(s) hidden; /process {process} or /history kind {history_kind} shows complete native output"
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

fn process_alias_for_history_kind(history_kind: &str) -> &'static str {
    match history_kind {
        "approval" => "approvals",
        "question" => "questions",
        "tool_use" => "tool-call",
        "tool_result" => "tool",
        "file_update" => "file",
        "diff" | "patch" => "diff",
        "stderr" => "stderr",
        "answer" => "answer",
        _ => "full",
    }
}

fn worker_output_kind_marker(
    kind: Option<event_format::VisibleAgentOutputKind>,
) -> (&'static str, Color) {
    match kind {
        Some(event_format::VisibleAgentOutputKind::Answer) => ("◆", TIFFANY_BLUE),
        Some(event_format::VisibleAgentOutputKind::Question) => ("?", TIFFANY_BLUE),
        Some(event_format::VisibleAgentOutputKind::Approval) => ("⚠", Color::Yellow),
        Some(event_format::VisibleAgentOutputKind::ToolCall) => ("↳", TIFFANY_BLUE),
        Some(event_format::VisibleAgentOutputKind::ToolResult) => ("✓", Color::Gray),
        Some(event_format::VisibleAgentOutputKind::Diff)
        | Some(event_format::VisibleAgentOutputKind::Patch) => ("±", TIFFANY_BLUE),
        Some(event_format::VisibleAgentOutputKind::FileUpdate) => ("✓", TIFFANY_BLUE),
        Some(event_format::VisibleAgentOutputKind::Stderr) => ("✗", Color::Red),
        Some(event_format::VisibleAgentOutputKind::Actionable) => ("⚠", Color::Yellow),
        Some(event_format::VisibleAgentOutputKind::Final) => ("✓", TIFFANY_BLUE),
        Some(event_format::VisibleAgentOutputKind::Normal) | None => ("│", TIFFANY_DARK),
    }
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
        if suffix == "session recovery"
            && let Some(recovery) = event.recovery.as_deref().and_then(nonempty_trimmed)
        {
            return format!("worker {suffix} · {recovery} · {label}");
        }
        format!("worker {suffix} · {label}")
    } else {
        format!("{} · {suffix}", stage_label(event))
    }
}

fn worker_output_suffix(event: &TiffanyProgressEvent) -> &'static str {
    tiffany_bridge::visible_output_suffix(
        &bridge_visible_output_event(event),
        CONTROL_SUMMARY_MAX_CHARS,
    )
}

fn worker_visible_output_kind(
    event: &TiffanyProgressEvent,
) -> Option<event_format::VisibleAgentOutputKind> {
    let raw = event.content.as_deref()?;
    worker_visible_output_kind_for_event(event, raw)
}

fn worker_visible_output_kind_for_event(
    event: &TiffanyProgressEvent,
    raw: &str,
) -> Option<event_format::VisibleAgentOutputKind> {
    tiffany_bridge::visible_output_kind_for_event(
        &bridge_visible_output_event(event),
        raw,
        CONTROL_SUMMARY_MAX_CHARS,
    )
}

fn looks_like_native_session_recovery(content: &str) -> bool {
    tiffany_bridge::looks_like_native_session_recovery(content)
}

fn bridge_visible_output_event(
    event: &TiffanyProgressEvent,
) -> TiffanyBridgeVisibleOutputEvent<'_> {
    TiffanyBridgeVisibleOutputEvent {
        role: &event.role,
        status: &event.status,
        agent: event.agent.as_deref(),
        worker_role: event.worker_role.as_deref(),
        task_id: event.task_id.as_deref(),
        event_kind: event.event_kind.as_deref(),
        recovery: event.recovery.as_deref(),
        content: event.content.as_deref(),
    }
}

fn bridge_pending_visible_output<'a>(
    event: &'a TiffanyProgressEvent,
    content: &'a str,
) -> TiffanyBridgePendingVisibleOutput<'a> {
    TiffanyBridgePendingVisibleOutput {
        role: &event.role,
        agent: event.agent.as_deref(),
        worker_role: event.worker_role.as_deref(),
        task_id: event.task_id.as_deref(),
        content,
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
    let mut lines = Vec::new();
    if event.role == "worker" && event.status == "running" && event.task_id.is_some() {
        if let Some(prompt) = event
            .task_prompt
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let detail = visible_task_detail(prompt);
            lines.extend(labeled_detail_block(detail.label, &detail.text));
        }
    }
    if let Some(line) = event_failure_hint_line(event) {
        lines.push(line);
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleTaskDetail {
    label: &'static str,
    text: String,
}

fn visible_task_detail(prompt: &str) -> VisibleTaskDetail {
    let trimmed = prompt.trim();
    let request = event_format::current_user_request(trimmed);
    if request != trimmed {
        return VisibleTaskDetail {
            label: "user",
            text: request.trim().to_string(),
        };
    }

    for marker in ["User message:", "Current request:", "Task:"] {
        if let Some(idx) = trimmed.rfind(marker) {
            let visible = trimmed[idx + marker.len()..].trim();
            if !visible.is_empty() {
                let lower = trimmed[..idx].to_ascii_lowercase();
                let label = if lower.contains("answer the user's message directly")
                    || lower.contains("conversational or explanatory request")
                    || marker != "Task:"
                {
                    "user"
                } else {
                    "task"
                };
                return VisibleTaskDetail {
                    label,
                    text: visible.to_string(),
                };
            }
        }
    }

    VisibleTaskDetail {
        label: "task",
        text: trimmed.to_string(),
    }
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
    tiffany_bridge::visible_output_content(
        &bridge_visible_output_event(event),
        FULL_MESSAGE_STREAM_MAX_CHARS,
        CONTROL_SUMMARY_MAX_CHARS,
    )
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
        .recovery
        .as_deref()
        .and_then(nonempty_trimmed)
        .is_some()
    {
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

fn is_worker_answer_stream_output(event: &TiffanyProgressEvent, visible: &str) -> bool {
    worker_answer_stream_candidate(event, visible).is_some()
}

fn worker_answer_stream_candidate(event: &TiffanyProgressEvent, visible: &str) -> Option<String> {
    if event.role != "worker" || event.status != "output" {
        return None;
    }
    if event
        .recovery
        .as_deref()
        .and_then(nonempty_trimmed)
        .is_some()
    {
        return None;
    }
    let raw = event.content.as_deref()?;
    if looks_like_native_session_recovery(raw) {
        return None;
    }
    let output_kind = worker_visible_output_kind(event);
    let runtime_kind = event_format::runtime_output_kind(raw);
    let is_answer = matches!(
        output_kind,
        Some(event_format::VisibleAgentOutputKind::Answer)
            | Some(event_format::VisibleAgentOutputKind::Final)
    ) || runtime_kind
        .as_deref()
        .is_some_and(|kind| matches!(kind, "assistant" | "result" | "final" | "turn_complete"));
    if !is_answer {
        return None;
    }
    let candidate = final_output_from_event(event, visible)
        .unwrap_or_else(|| event_format::strip_known_final_heading(visible));
    let candidate = candidate.trim();
    (!candidate.is_empty() && !is_low_value_output(candidate)).then_some(candidate.to_string())
}

fn merge_streaming_answer_delta(current: Option<&str>, candidate: &str) -> (String, String) {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        let current = current.unwrap_or_default().trim().to_string();
        return (current, String::new());
    }
    let Some(current) = current.map(str::trim).filter(|current| !current.is_empty()) else {
        return (candidate.to_string(), candidate.to_string());
    };
    if candidate == current || current.contains(candidate) {
        return (current.to_string(), String::new());
    }
    if candidate.starts_with(current) {
        let delta = candidate[current.len()..].to_string();
        return (candidate.to_string(), delta);
    }
    if candidate.contains(current) {
        return (candidate.to_string(), candidate.to_string());
    }
    if candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_punctuation())
    {
        return (format!("{current}{candidate}"), candidate.to_string());
    }
    let delta = format!("\n{candidate}");
    (format!("{current}{delta}"), delta)
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
    tiffany_bridge::normalized_visible_output_key(content, FULL_MESSAGE_STREAM_MAX_CHARS)
}

fn output_seen_key(
    event: &TiffanyProgressEvent,
    content: &str,
    pending_tool_call: Option<&(TiffanyProgressEvent, String)>,
) -> String {
    let pending = pending_tool_call
        .map(|(tool_event, tool_content)| bridge_pending_visible_output(tool_event, tool_content));
    tiffany_bridge::visible_output_seen_key(
        &bridge_visible_output_event(event),
        content,
        pending.as_ref(),
        FULL_MESSAGE_STREAM_MAX_CHARS,
        CONTROL_SUMMARY_MAX_CHARS,
    )
}

#[cfg(test)]
fn output_scope(event: &TiffanyProgressEvent) -> String {
    tiffany_bridge::visible_output_scope(
        &bridge_visible_output_event(event),
        CONTROL_SUMMARY_MAX_CHARS,
    )
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
        "planner" => {
            if !lower.starts_with("plan ready") {
                return false;
            }
            match plan_ready_worker_run_count(&lower) {
                Some(count) => count <= 1,
                None => lines.len() <= 1,
            }
        }
        "critic" | "reviewer" => {
            lower.starts_with("approved")
                || (lower.starts_with("needs changes")
                    && !role_output_has_actionable_detail(&lines))
        }
        _ => false,
    }
}

fn plan_ready_worker_run_count(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with("plan ready") {
        return None;
    }
    line.split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
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

fn native_cli_transcript_label(command: &TiffanyNativeCliCommand) -> &'static str {
    if native_cli_command_is_codex(command) {
        "Codex transcript"
    } else if native_cli_command_is_gemini(command) {
        "Gemini transcript"
    } else {
        "Claude transcript"
    }
}

#[cfg(test)]
pub(crate) fn native_cli_return_lines(
    command: &TiffanyNativeCliCommand,
    outcome: &TiffanyNativeCliReturn,
) -> Vec<Line<'static>> {
    native_cli_return_lines_with_exit_status(command, outcome, None)
}

#[cfg(test)]
pub(crate) fn native_cli_return_lines_with_exit_status(
    command: &TiffanyNativeCliCommand,
    outcome: &TiffanyNativeCliReturn,
    exit_status: Option<&std::process::ExitStatus>,
) -> Vec<Line<'static>> {
    native_cli_return_lines_with_history_status(command, outcome, exit_status, None, false, None)
}

pub(crate) fn native_cli_return_lines_with_history_status(
    command: &TiffanyNativeCliCommand,
    outcome: &TiffanyNativeCliReturn,
    exit_status: Option<&std::process::ExitStatus>,
    history_path: Option<&Path>,
    session_db_import_queued: bool,
    history_error: Option<&str>,
) -> Vec<Line<'static>> {
    let has_changes = native_cli_return_has_changes(outcome);
    let transcript_label = native_cli_transcript_label(command);
    let exited_successfully = exit_status.is_none_or(std::process::ExitStatus::success);
    let symbol = if exited_successfully { "✓" } else { "⚠" };
    let color = if exited_successfully {
        TIFFANY_BLUE
    } else {
        Color::Yellow
    };
    let message = if !exited_successfully {
        "native CLI exited with non-zero status"
    } else if has_changes {
        "native CLI exited with workspace changes"
    } else {
        "native CLI exited"
    };
    let mut lines = vec![status_line(symbol, color, &command.role, message)];
    if let Some(status) = exit_status
        && !status.success()
    {
        lines.push(thread_meta_line("exit", &status.to_string()));
    }
    lines.push(thread_meta_line("native", &command.native_session));
    if let Some(runtime) = command.runtime.as_deref().and_then(nonempty_trimmed) {
        lines.push(thread_meta_line("runtime", runtime));
    }
    lines.push(thread_meta_line("command", &command.command));
    if let Some(thread) = command
        .worker_thread_id
        .as_deref()
        .and_then(nonempty_trimmed)
    {
        lines.push(thread_meta_line("thread", thread));
    }
    if let Some(worktree) = command.worktree.as_deref().and_then(nonempty_trimmed) {
        lines.push(thread_meta_line("work", worktree));
    }
    if outcome.transcript_event_count > 0 {
        lines.push(thread_meta_line(
            "native events",
            &format!(
                "{} captured from {transcript_label}",
                outcome.transcript_event_count,
            ),
        ));
        lines.push(thread_meta_line(
            "captured",
            "Tiffany native history updated with returned transcript",
        ));
    } else {
        lines.push(thread_meta_line(
            "native events",
            &format!("none captured from {transcript_label}"),
        ));
        lines.push(next_line(
            "/history status",
            "check session-db and local native transcript availability",
        ));
    }
    if let Some(preview) = outcome
        .transcript_preview
        .as_deref()
        .and_then(nonempty_trimmed)
    {
        lines.push(thread_meta_line("native answer", transcript_label));
        for line in preview.lines().take(12) {
            lines.push(output_body_line_for_kind(
                line,
                Some(event_format::VisibleAgentOutputKind::Answer),
                false,
            ));
        }
        let hidden = preview.lines().count().saturating_sub(12);
        if hidden > 0 {
            lines.push(detail_continuation_line(&format!(
                "… {hidden} more line(s); /history kind answer shows complete native answer"
            )));
        }
    }
    if let Some(status) = outcome.status.as_deref().and_then(nonempty_trimmed) {
        lines.push(thread_meta_line("status", "git status --short"));
        for line in status.lines().take(20) {
            lines.push(detail_continuation_line(line));
        }
        let hidden = status.lines().count().saturating_sub(20);
        if hidden > 0 {
            lines.push(detail_continuation_line(&format!(
                "… {hidden} more file(s); run git status for all"
            )));
        }
    }
    if let Some(diff_stat) = outcome.diff_stat.as_deref().and_then(nonempty_trimmed) {
        lines.push(thread_meta_line("diff", "git diff --stat"));
        for line in diff_stat.lines().take(20) {
            lines.push(detail_continuation_line(line));
        }
        let hidden = diff_stat.lines().count().saturating_sub(20);
        if hidden > 0 {
            lines.push(detail_continuation_line(&format!(
                "… {hidden} more line(s); run git diff --stat for all"
            )));
        }
    }
    if let Some(diff_patch) = outcome.diff_patch.as_deref().and_then(nonempty_trimmed) {
        lines.push(thread_meta_line("patch", "git diff"));
        append_native_cli_patch_preview(&mut lines, diff_patch, "git diff");
    }
    if let Some(diff_stat) = outcome
        .staged_diff_stat
        .as_deref()
        .and_then(nonempty_trimmed)
    {
        lines.push(thread_meta_line("staged", "git diff --cached --stat"));
        for line in diff_stat.lines().take(20) {
            lines.push(detail_continuation_line(line));
        }
        let hidden = diff_stat.lines().count().saturating_sub(20);
        if hidden > 0 {
            lines.push(detail_continuation_line(&format!(
                "… {hidden} more line(s); run git diff --cached --stat for all"
            )));
        }
    }
    if let Some(diff_patch) = outcome
        .staged_diff_patch
        .as_deref()
        .and_then(nonempty_trimmed)
    {
        lines.push(thread_meta_line("patch", "git diff --cached"));
        append_native_cli_patch_preview(&mut lines, diff_patch, "git diff --cached");
    }
    if let Some(path) = history_path {
        lines.push(thread_meta_line("history", &path.display().to_string()));
        if session_db_import_queued {
            lines.push(thread_meta_line(
                "session-db",
                "import queued from local native history",
            ));
        } else {
            lines.push(thread_meta_line(
                "session-db",
                "not imported yet; run /history sync",
            ));
        }
    } else if let Some(error) = history_error.and_then(nonempty_trimmed) {
        lines.push(thread_meta_line("history", "local save failed"));
        lines.push(body_line(error, true));
    }
    if has_changes {
        lines.push(thread_meta_line(
            "workspace",
            "workspace status and diff captured into Tiffany history",
        ));
        lines.push(next_line(
            "/history kind diff",
            "inspect full saved native patch",
        ));
    }
    lines.push(next_line(
        &format!("/continue open {}", command.role),
        "resume the same native CLI conversation",
    ));
    lines.push(next_line(
        "/history status",
        "verify session-db/local-json sync after native return",
    ));
    lines.push(next_line(
        &format!("/history role {}", command.role),
        "show returned native transcript, tools, answers, and diff",
    ));
    lines.push(next_line(
        &format!("/thread {}", command.role),
        "confirm native session and last Tiffany session pointer",
    ));
    lines.push(next_line(
        &format!("/thread export {}", command.role),
        "write full selectable handoff history",
    ));
    lines
}

fn append_native_cli_patch_preview(lines: &mut Vec<Line<'static>>, patch: &str, command: &str) {
    const PATCH_PREVIEW_LINES: usize = 32;
    for line in patch.lines().take(PATCH_PREVIEW_LINES) {
        lines.push(diff_body_line(line));
    }
    let hidden = patch.lines().count().saturating_sub(PATCH_PREVIEW_LINES);
    if hidden > 0 {
        lines.push(detail_continuation_line(&format!(
            "… {hidden} more patch line(s); /history kind diff shows complete {command}"
        )));
    }
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

fn output_body_line_for_kind(
    line: &str,
    kind: Option<event_format::VisibleAgentOutputKind>,
    first_line: bool,
) -> Line<'static> {
    match kind {
        Some(event_format::VisibleAgentOutputKind::Diff) => diff_body_line(line),
        Some(event_format::VisibleAgentOutputKind::Answer) => {
            process_body_line(line, "◆", TIFFANY_BLUE, Style::default(), first_line)
        }
        Some(event_format::VisibleAgentOutputKind::Question) => process_body_line(
            line,
            "?",
            TIFFANY_BLUE,
            Style::default().fg(TIFFANY_BLUE),
            first_line,
        ),
        Some(event_format::VisibleAgentOutputKind::Approval) => process_body_line(
            line,
            "⚠",
            Color::Yellow,
            Style::default().fg(Color::Yellow),
            first_line,
        ),
        Some(event_format::VisibleAgentOutputKind::ToolCall) => {
            process_body_line(line, "↳", TIFFANY_BLUE, Style::default(), first_line)
        }
        Some(event_format::VisibleAgentOutputKind::ToolResult) => process_body_line(
            line,
            "✓",
            Color::Gray,
            Style::default().fg(Color::Gray),
            first_line,
        ),
        Some(event_format::VisibleAgentOutputKind::Patch)
        | Some(event_format::VisibleAgentOutputKind::FileUpdate) => process_body_line(
            line,
            if kind == Some(event_format::VisibleAgentOutputKind::FileUpdate) {
                "✓"
            } else {
                "±"
            },
            TIFFANY_BLUE,
            Style::default().fg(Color::Gray),
            first_line,
        ),
        Some(event_format::VisibleAgentOutputKind::Stderr) => process_body_line(
            line,
            "✗",
            Color::Red,
            Style::default().fg(Color::Red),
            first_line,
        ),
        Some(event_format::VisibleAgentOutputKind::Actionable) => process_body_line(
            line,
            "⚠",
            Color::Yellow,
            Style::default().fg(Color::Yellow),
            first_line,
        ),
        Some(event_format::VisibleAgentOutputKind::Final) => {
            process_body_line(line, "✓", TIFFANY_BLUE, Style::default(), first_line)
        }
        _ => output_body_line(line),
    }
}

fn process_body_line(
    line: &str,
    first_marker: &'static str,
    marker_color: Color,
    text_style: Style,
    first_line: bool,
) -> Line<'static> {
    let marker = if first_line { first_marker } else { "│" };
    Line::from(vec![
        Span::styled("  ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(
            marker,
            Style::default()
                .fg(marker_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(line.to_string(), text_style),
    ])
}

fn diff_body_line(line: &str) -> Line<'static> {
    let color = if line.starts_with('+') && !line.starts_with("+++") {
        TIFFANY_BLUE
    } else if line.starts_with('-') && !line.starts_with("---") {
        Color::Red
    } else if line.starts_with("@@") {
        TIFFANY_SOFT
    } else if line.starts_with("diff --git") || line.starts_with("files changed:") {
        Color::Gray
    } else {
        Color::DarkGray
    };
    Line::from(vec![
        Span::styled("  │ ", Style::default().fg(TIFFANY_DARK)),
        Span::styled(line.to_string(), Style::default().fg(color)),
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

fn event_failure_hint_line(event: &TiffanyProgressEvent) -> Option<Line<'static>> {
    let raw = event.content.as_deref().unwrap_or(&event.message);
    let hint = event_format::agent_failure_hint(raw, CONTROL_SUMMARY_MAX_CHARS)?;
    Some(failure_hint_line(hint.title(), hint.action()))
}

fn worker_question_hint_lines(
    event: &TiffanyProgressEvent,
    kind: Option<event_format::VisibleAgentOutputKind>,
) -> Vec<Line<'static>> {
    let target = event
        .worker_role
        .as_deref()
        .and_then(nonempty_trimmed)
        .or_else(|| event.agent.as_deref().and_then(nonempty_trimmed))
        .unwrap_or("worker");
    let has_native_session = event
        .native_session_id
        .as_deref()
        .and_then(nonempty_trimmed)
        .is_some();
    let (handoff_detail, history_kind, history_detail) = match kind {
        Some(event_format::VisibleAgentOutputKind::Approval) => (
            if has_native_session {
                "reopen same native session to approve or deny"
            } else {
                "open worker handoff to approve or deny"
            },
            "approval",
            "show saved approval requests across native worker events",
        ),
        _ => (
            if has_native_session {
                "reopen same native session to answer the worker question"
            } else {
                "open worker handoff to answer the question"
            },
            "question",
            "show saved worker questions across native worker events",
        ),
    };
    let process = process_alias_for_history_kind(history_kind);
    let mut lines = vec![
        next_line(&format!("/continue open {target}"), handoff_detail),
        next_line(
            &format!("/thread {target}"),
            "inspect session, native id, and handoff state",
        ),
        next_line(
            &format!("/process {process}"),
            "show matching process events in this TUI",
        ),
        next_line(&format!("/history kind {history_kind}"), history_detail),
    ];
    if let Some(native) = event
        .native_session_id
        .as_deref()
        .and_then(nonempty_trimmed)
    {
        let short = short_task_id(Some(native)).unwrap_or(native);
        lines.push(next_line(
            &format!("native {short}"),
            "same underlying CLI conversation will be resumed",
        ));
    }
    lines
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

    #[cfg(unix)]
    fn test_exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn test_exit_status(code: u32) -> std::process::ExitStatus {
        use std::os::windows::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(code)
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn bridge_history_cells(
        events: impl IntoIterator<Item = TiffanyProgressEvent>,
    ) -> Vec<Vec<String>> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let mut state = BridgeState::default();
        for event in events {
            state.emit_event(&app_event_tx, event);
        }
        state.flush_timeline(&app_event_tx);
        state.flush_pending_tool_call(&app_event_tx);

        let mut cells = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::InsertHistoryCell(cell) = event {
                cells.push(
                    cell.transcript_lines(u16::MAX)
                        .iter()
                        .map(line_text)
                        .collect::<Vec<_>>(),
                );
            }
        }
        cells
    }

    fn continue_open_output_text_and_command(
        thread_show_stdout: &str,
    ) -> (String, Option<TiffanyNativeCliCommand>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        emit_continue_open_output(
            &app_event_tx,
            std::process::Output {
                status: test_exit_status(0),
                stdout: thread_show_stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            },
        );

        let mut text = Vec::new();
        let mut command = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::InsertHistoryCell(cell) => {
                    text.push(
                        cell.transcript_lines(u16::MAX)
                            .iter()
                            .map(line_text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                AppEvent::TiffanyOrchestratorOpenNativeCli {
                    command: native_command,
                } => {
                    command = Some(native_command);
                }
                _ => {}
            }
        }

        (text.join("\n"), command)
    }

    fn test_config(
        bin: impl Into<String>,
        config_path: Option<String>,
    ) -> TiffanyOrchestratorConfig {
        TiffanyOrchestratorConfig {
            bin: bin.into(),
            extra_args: Vec::new(),
            config_path,
            worker_scope: "tui:test:/tmp/tiffany".to_string(),
        }
    }

    fn test_launch(
        user_prompt: impl Into<String>,
        prompt: impl Into<String>,
        context_turn_count: usize,
    ) -> TiffanyOrchestratorLaunch {
        TiffanyOrchestratorLaunch {
            bin: "orchestrator".into(),
            user_prompt: user_prompt.into(),
            prompt: prompt.into(),
            extra_args: Vec::new(),
            config_path: None,
            context_turn_count,
            worker_scope: "tui:test:/tmp/tiffany".to_string(),
        }
    }

    fn sample_native_conversation(
        id: &str,
        cwd: &Path,
        updated_at_unix: u64,
        turn_count: usize,
        event_count: usize,
    ) -> TiffanyNativeChatConversation {
        let events = (0..event_count)
            .map(|idx| TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: format!("worker answer {idx}"),
                kind: Some("answer".into()),
                content: Some(format!("answer {idx}")),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: None,
                provider: None,
                task_id: Some(format!("task-{idx}")),
                worker_thread_id: Some("thread-1".into()),
                native_session_id: Some("native-1".into()),
            })
            .collect::<Vec<_>>();
        TiffanyNativeChatConversation {
            id: id.to_string(),
            cwd: cwd_key(cwd),
            created_at_unix: updated_at_unix.saturating_sub(1),
            updated_at_unix,
            turns: (0..turn_count)
                .map(|idx| TiffanyNativeChatTurn {
                    user_prompt: format!("prompt {idx}"),
                    result: format!("result {idx}"),
                    captured_at_unix: updated_at_unix,
                    events: if idx == 0 { events.clone() } else { Vec::new() },
                })
                .collect(),
        }
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
    fn failed_event_detail_lines_show_failure_hint() {
        let event = test_event(
            "orchestrator",
            "failed",
            "worker-codex exited: unexpected status 401 Unauthorized: invalid_api_key",
        );

        let lines = event_detail_lines(&event);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("provider authentication failed"));
        assert!(text.contains("Set the provider key in /provider"));
    }

    fn test_native_worker_event(
        worker_role: &str,
        agent: &str,
        kind: &str,
        content: &str,
    ) -> TiffanyNativeChatEvent {
        let provider = if worker_role == "worker-codex" {
            "minimax"
        } else {
            "anthropic"
        };
        let model = if worker_role == "worker-codex" {
            "MiniMax-M3"
        } else {
            "claude-sonnet-4-6"
        };
        TiffanyNativeChatEvent {
            role: "worker".into(),
            status: "output".into(),
            title: format!(
                "worker {} · {worker_role} · {agent}",
                kind.replace('_', " ")
            ),
            kind: Some(kind.into()),
            content: Some(content.into()),
            agent: Some(agent.into()),
            worker_role: Some(worker_role.into()),
            model: Some(model.into()),
            provider: Some(provider.into()),
            task_id: Some(format!("task-{kind}")),
            worker_thread_id: Some(if worker_role == "worker-codex" {
                "thread-codex".into()
            } else {
                "thread-worker".into()
            }),
            native_session_id: Some(if worker_role == "worker-codex" {
                "codex-native".into()
            } else {
                "claude-native".into()
            }),
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
        assert!(
            text.contains("setup  /provider  /role  /roles  /thread  /continue  /history  /doctor")
        );
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
        let config = test_config(
            "definitely-missing-orchestrator",
            Some(missing.display().to_string()),
        );
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
        let config = test_config("orchestrator", Some(config_path.display().to_string()));
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
        let config = test_config("orchestrator", Some(config_path.display().to_string()));
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
        let config = test_config("orchestrator", Some(config_path.display().to_string()));
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
        let config = test_config("orchestrator", Some(config_path.display().to_string()));
        let readiness = startup_readiness(Some(&config));

        let lines = idle_intro_lines_with_readiness(0, Some(&readiness));
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker  provider-missing  worker-codex · codex · codex"));
        Ok(())
    }

    #[test]
    fn startup_health_actions_group_missing_runtime_and_config() {
        let missing = std::env::temp_dir().join(format!(
            "definitely-missing-tiffany-config-health-{}",
            std::process::id()
        ));
        let config = test_config(
            "definitely-missing-orchestrator",
            Some(missing.display().to_string()),
        );
        let readiness = startup_readiness(Some(&config));

        let lines = startup_health_action_lines(&readiness);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("health  runtime"));
        assert!(text.contains("TIFFANY_ORCHESTRATOR_BIN"));
        assert!(text.contains("health  config"));
        assert!(text.contains("fix /provider then /role"));
        assert!(text.contains("health  provider auth"));
        assert!(text.contains("health  role wiring"));
        assert!(text.contains("health  doctor"));
    }

    #[test]
    fn startup_health_actions_group_default_worker_provider_missing() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers: {}\nruntimes:\n  codex:\n    type: subprocess\nmodels:\n  - id: minimax-m3-codex\n    provider: minimax\n    name: MiniMax-M3\nroles:\n  worker-codex:\n    model: minimax-m3-codex\n    runtime: codex\nbehavior: {}\n",
        )?;
        let config = test_config("orchestrator", Some(config_path.display().to_string()));
        let readiness = startup_readiness(Some(&config));

        let lines = startup_health_action_lines(&readiness);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("health  provider auth"));
        assert!(text.contains("worker-codex model provider is missing"));
        assert!(text.contains("fix /provider then /role worker-cc"));
        assert!(text.contains("health  doctor"));
        Ok(())
    }

    #[test]
    fn startup_health_actions_report_ok_when_ready() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join("config.yaml");
        let runtime = temp.path().join(executable_name("claude"));
        std::fs::write(&runtime, "")?;
        make_launchable(&runtime)?;
        std::fs::write(
            &config_path,
            format!(
                "providers:\n  anthropic:\n    type: anthropic\nruntimes:\n  claude-code:\n    type: subprocess\n    binary: {}\nmodels:\n  - id: sonnet\n    provider: anthropic\n    name: claude-sonnet-4-6\nroles:\n  worker-cc:\n    model: sonnet\n    runtime: claude-code\nbehavior: {{}}\n",
                runtime.display()
            ),
        )?;
        let config = test_config("orchestrator", Some(config_path.display().to_string()));
        let readiness = startup_readiness(Some(&config));

        let lines = startup_health_action_lines(&readiness);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(lines.len(), 1);
        assert!(text.contains("health  ok"));
        assert!(text.contains("/doctor for full diagnostics"));
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
    fn native_chat_append_returns_sessions_file_for_db_mirror() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        let path = append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "hello".into(),
                result: "hi".into(),
            },
            Vec::new(),
        )
        .expect("append native")
        .expect("native sessions path");

        assert_eq!(path, home.path().join(NATIVE_SESSIONS_FILE));
        assert!(path.is_file());
    }

    #[test]
    fn native_chat_sessions_ignore_empty_turns() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        let path = append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: " ".into(),
                result: "ignored".into(),
            },
            Vec::new(),
        )
        .expect("append empty");

        assert!(path.is_none());
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
                kind: Some("diff".into()),
                content: Some(
                    "files changed:\n  - src/lib.rs\n\ndiff --git a/src/lib.rs b/src/lib.rs".into(),
                ),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: Some("claude-sonnet-4-6".into()),
                provider: Some("anthropic".into()),
                task_id: Some("abc12345".into()),
                worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                native_session_id: Some("native-1".into()),
            }],
        )
        .expect("append native event");

        let loaded = load_native_chat_conversation(home.path(), cwd.path())
            .expect("load")
            .expect("conversation");
        let events = &loaded.turns[0].events;
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].title,
            "worker diff · worker-cc · claude-code · abc12345"
        );
        assert_eq!(events[0].kind.as_deref(), Some("diff"));
        assert!(events[0].content.as_deref().unwrap().contains("diff --git"));
        assert_eq!(events[0].agent.as_deref(), Some("claude-code"));
        assert_eq!(
            events[0].worker_thread_id.as_deref(),
            Some("abcdef12-0000-0000-0000-000000000000")
        );
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
                    kind: Some("diff".into()),
                    content: Some(
                        "files changed:\n  - README.md\n\ndiff --git a/README.md b/README.md"
                            .into(),
                    ),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("abc12345".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-1".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "done".into(),
                    title: "worker-cc done · claude-code · 1s".into(),
                    kind: Some("done".into()),
                    content: None,
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: None,
                    provider: None,
                    task_id: Some("abc12345".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-1".into()),
                },
            ],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  1 turn(s) saved"));
        assert!(text.contains("source  local-json"));
        assert!(text.contains("user  改一下 README"));
        assert!(text.contains("answer 已更新 README"));
        assert!(text.contains("worker diff"));
        assert!(text.contains("kind: diff"));
        assert!(text.contains("diff --git a/README.md b/README.md"));
        assert!(text.contains("thread abcdef12"));
        assert!(text.contains("/history full"));
        assert!(text.contains("/history kind answer|tool_result|diff|approval|session_recovery"));
        assert!(text.contains("/history export kind <event-kind>"));
        assert!(text.contains("/thread <role>"));
    }

    #[test]
    fn native_history_full_expands_complete_event_content() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let long_content = (1..=8)
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "跑测试".into(),
                result: "测试完成".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker tool result · worker-cc · claude-code".into(),
                kind: Some("tool_result".into()),
                content: Some(long_content),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: Some("claude-sonnet-4-6".into()),
                provider: Some("anthropic".into()),
                task_id: Some("abc12345".into()),
                worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                native_session_id: Some("native-1".into()),
            }],
        )
        .expect("append");

        let summary = native_history_lines(home.path(), cwd.path(), "")
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(summary.contains("line 1"));
        assert!(!summary.contains("line 8"));
        assert!(summary.contains("/history full shows complete content"));

        let full = native_history_lines(home.path(), cwd.path(), "full")
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(full.contains("line 1"));
        assert!(full.contains("line 8"));
        assert!(!full.contains("more line(s)"));
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
                kind: Some("tool_result".into()),
                content: Some("cargo test -q\n\nok".into()),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: Some("claude-sonnet-4-6".into()),
                provider: Some("anthropic".into()),
                task_id: Some("abc12345".into()),
                worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
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
        assert!(markdown.contains("kind `tool_result`"));
        assert!(markdown.contains("thread `abcdef12-0000-0000-0000-000000000000`"));
        assert!(markdown.contains("```text\ncargo test -q"));
    }

    #[test]
    fn native_history_graph_summarizes_saved_turn_flow() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "实现登录".into(),
                result: "登录完成".into(),
            },
            vec![
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker tool call · worker-cc · claude-code".into(),
                    kind: Some("tool_call".into()),
                    content: Some("Bash: cargo test -q".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("abc12345".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-1".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker tool result · worker-cc · claude-code".into(),
                    kind: Some("tool_result".into()),
                    content: Some("ok".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("abc12345".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-1".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker diff · worker-cc · claude-code".into(),
                    kind: Some("diff".into()),
                    content: Some("diff --git a/src/login.rs b/src/login.rs".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("abc12345".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-1".into()),
                },
            ],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "graph");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  conversation flow graph"));
        assert!(text.contains("Conversation flow"));
        assert!(text.contains("1. user -> answer"));
        assert!(text.contains("user: 实现登录"));
        assert!(text.contains("answer: 登录完成"));
        assert!(text.contains("events: tool: Bash: cargo test -q -> tool result -> diff"));
        assert!(text.contains("/history export-graph"));
    }

    #[test]
    fn native_history_mermaid_renders_escaped_flowchart() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "生成 {json}".into(),
                result: "完成 [ok]".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker tool call · worker-codex · codex".into(),
                kind: Some("tool_call".into()),
                content: Some("Shell {\"cmd\":\"cargo test\"}".into()),
                agent: Some("codex".into()),
                worker_role: Some("worker-codex".into()),
                model: Some("gpt-5".into()),
                provider: Some("openai".into()),
                task_id: Some("codex-1".into()),
                worker_thread_id: Some("thread-codex".into()),
                native_session_id: Some("native-codex".into()),
            }],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "mermaid");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  conversation Mermaid graph"));
        assert!(text.contains("```mermaid"));
        assert!(text.contains("flowchart TD"));
        assert!(text.contains("user: 生成 (json)"));
        assert!(text.contains("answer: 完成 (ok)"));
        assert!(text.contains("tool: Shell cargo test"));
        assert!(!text.contains("\\\"cmd\\\""));
        assert!(!text.contains('{'));
        assert!(!text.contains('}'));
    }

    #[test]
    fn native_history_compact_renders_storyline_without_raw_json() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "实现登录".into(),
                result: "登录完成".into(),
            },
            vec![
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker tool call · worker-cc · claude-code".into(),
                    kind: Some("tool_call".into()),
                    content: Some("Bash {\"cmd\":\"cargo test -q\"}".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("abc12345".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-1".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker diff · worker-cc · claude-code".into(),
                    kind: Some("diff".into()),
                    content: Some("diff --git a/src/login.rs b/src/login.rs".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("abc12345".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-1".into()),
                },
            ],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "compact role worker-cc");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  compact storyline"));
        assert!(text.contains("filter  role worker-cc"));
        assert!(text.contains("roles  worker-cc"));
        assert!(text.contains("events  tool call, diff"));
        assert!(text.contains("Task storyline"));
        assert!(text.contains("1. user  实现登录"));
        assert!(text.contains("answer: 登录完成"));
        assert!(text.contains("flow: tool: Bash"));
        assert!(text.contains("diff captured"));
        assert!(text.contains("artifacts: role worker-cc, thread abcdef12, native native-1"));
        assert!(text.contains("resume  /continue worker-cc"));
        assert!(!text.contains("\"cmd\""));
    }

    #[test]
    fn native_history_export_graph_writes_graph_file() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let out = home.path().join("flow.txt");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "整理历史".into(),
                result: "已整理".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker final · worker-cc · claude-code".into(),
                kind: Some("final".into()),
                content: Some("已整理".into()),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: Some("claude-sonnet-4-6".into()),
                provider: Some("anthropic".into()),
                task_id: Some("abc12345".into()),
                worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                native_session_id: Some("native-1".into()),
            }],
        )
        .expect("append");

        let lines = native_history_lines(
            home.path(),
            cwd.path(),
            &format!("export-graph --text --out {}", out.display()),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("history  conversation graph exported"));
        assert!(text.contains(out.to_string_lossy().as_ref()));

        let graph = std::fs::read_to_string(&out).expect("graph export");
        assert!(graph.contains("Conversation flow"));
        assert!(graph.contains("1. user -> answer"));
        assert!(graph.contains("events: final"));
    }

    #[test]
    fn native_chat_events_infer_kind_for_old_saved_events() {
        let old_event = TiffanyNativeChatEvent {
            role: "worker".into(),
            status: "output".into(),
            title: "worker patch · worker-cc · claude-code".into(),
            kind: None,
            content: Some("*** Begin Patch\n*** Update File: README.md".into()),
            agent: Some("claude-code".into()),
            worker_role: Some("worker-cc".into()),
            model: None,
            provider: None,
            task_id: Some("abc12345".into()),
            worker_thread_id: None,
            native_session_id: None,
        };

        let normalized = normalize_native_event(old_event).expect("normalized old event");

        assert_eq!(normalized.kind.as_deref(), Some("patch"));
    }

    #[test]
    fn progress_worker_diff_event_captures_native_kind() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            content: Some(
                "claude-code diff: files changed:\n  - README.md\n\ndiff --git a/README.md b/README.md"
                    .to_string(),
            ),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let visible = visible_content(&event).expect("diff visible");

        let native = native_chat_event_from_progress(&event, Some(&visible)).expect("native event");

        assert_eq!(native.kind.as_deref(), Some("diff"));
        assert_eq!(
            native.content.as_deref(),
            Some("files changed:\n  - README.md\n\ndiff --git a/README.md b/README.md")
        );
        assert!(native.title.contains("worker diff"));
    }

    #[test]
    fn progress_worker_event_kind_drives_native_kind() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code event: Bash({\"command\":\"cargo test\"})".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let visible = visible_content(&event).expect("tool visible");

        let native = native_chat_event_from_progress(&event, Some(&visible)).expect("native event");

        assert_eq!(native.kind.as_deref(), Some("tool_call"));
        assert!(native.title.contains("worker tool call"));
        assert!(native.content.as_deref().unwrap().contains("Bash"));
    }

    #[test]
    fn progress_worker_answer_event_captures_native_answer_kind() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("assistant".to_string()),
            content: Some("claude-code assistant: 你好！".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let visible = visible_content(&event).expect("answer visible");

        let native = native_chat_event_from_progress(&event, Some(&visible)).expect("native event");

        assert_eq!(visible, "你好！");
        assert_eq!(native.kind.as_deref(), Some("answer"));
        assert_eq!(native.content.as_deref(), Some("你好！"));
        assert!(native.title.contains("worker answer"));
    }

    #[test]
    fn native_history_export_can_filter_worker_role() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let out = home.path().join("worker-cc.md");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "分别处理两个角色".into(),
                result: "处理完成".into(),
            },
            vec![
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker output · worker-cc · claude-code".into(),
                    kind: Some("output".into()),
                    content: Some("claude handoff content".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("task-cc".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-cc".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker output · worker-codex · codex".into(),
                    kind: Some("output".into()),
                    content: Some("codex handoff content".into()),
                    agent: Some("codex".into()),
                    worker_role: Some("worker-codex".into()),
                    model: Some("gpt-4o".into()),
                    provider: Some("openai".into()),
                    task_id: Some("task-codex".into()),
                    worker_thread_id: Some("99999999-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-codex".into()),
                },
            ],
        )
        .expect("append");

        let lines = native_history_lines(
            home.path(),
            cwd.path(),
            &format!("export role worker-cc --out {}", out.display()),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("history  exported 1 turn(s)"));

        let markdown = std::fs::read_to_string(&out).expect("export body");
        assert!(markdown.contains("- Filter: `role worker-cc`"));
        assert!(markdown.contains("claude handoff content"));
        assert!(!markdown.contains("codex handoff content"));
    }

    #[test]
    fn native_history_export_can_filter_event_kind() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let out = home.path().join("approval.md");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "需要权限".into(),
                result: "等待用户批准".into(),
            },
            vec![
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker approval · worker-cc · claude-code".into(),
                    kind: Some("approval".into()),
                    content: Some("waiting for permission approval: write access".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("task-approval".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-cc".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker answer · worker-cc · claude-code".into(),
                    kind: Some("answer".into()),
                    content: Some("普通回答".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("task-answer".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-cc".into()),
                },
            ],
        )
        .expect("append");

        let lines = native_history_lines(
            home.path(),
            cwd.path(),
            &format!("export kind approval --out {}", out.display()),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("history  exported 1 turn(s)"));

        let markdown = std::fs::read_to_string(&out).expect("export body");
        assert!(markdown.contains("- Filter: `kind approval`"));
        assert!(markdown.contains("worker approval"));
        assert!(markdown.contains("waiting for permission approval"));
        assert!(!markdown.contains("普通回答"));
    }

    #[test]
    fn native_history_search_finds_turns_and_events() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "修复登录按钮".into(),
                result: "登录按钮已修复".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker diff · worker-cc · claude-code".into(),
                kind: Some("diff".into()),
                content: Some("diff --git a/src/login.rs b/src/login.rs".into()),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: None,
                provider: None,
                task_id: Some("abc12345".into()),
                worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                native_session_id: Some("native-1".into()),
            }],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "search login.rs");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("history  1 match(es) for 'login.rs'"));
        assert!(text.contains("turn 1 · event"));
        assert!(text.contains("diff --git a/src/login.rs b/src/login.rs"));

        let misses = native_history_lines(home.path(), cwd.path(), "search missing");
        let miss_text = misses.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(miss_text.contains("no matches for 'missing'"));
    }

    #[test]
    fn native_history_can_filter_by_role_thread_native_session_or_kind() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "并行处理两个角色".into(),
                result: "处理完成".into(),
            },
            vec![
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker answer · worker-cc · claude-code".into(),
                    kind: Some("answer".into()),
                    content: Some("claude role output".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: None,
                    provider: None,
                    task_id: Some("task-cc".into()),
                    worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-cc".into()),
                },
                TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker diff · worker-codex · codex".into(),
                    kind: Some("diff".into()),
                    content: Some("codex role output".into()),
                    agent: Some("codex".into()),
                    worker_role: Some("worker-codex".into()),
                    model: None,
                    provider: None,
                    task_id: Some("task-codex".into()),
                    worker_thread_id: Some("99999999-0000-0000-0000-000000000000".into()),
                    native_session_id: Some("native-codex".into()),
                },
            ],
        )
        .expect("append");

        let by_role = native_history_lines(home.path(), cwd.path(), "role worker-cc");
        let role_text = by_role.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(role_text.contains("filter  role worker-cc"));
        assert!(role_text.contains("claude role output"));
        assert!(!role_text.contains("codex role output"));

        let by_thread = native_history_lines(home.path(), cwd.path(), "thread abcdef12");
        let thread_text = by_thread
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(thread_text.contains("filter  thread abcdef12"));
        assert!(thread_text.contains("thread abcdef12"));
        assert!(thread_text.contains("claude role output"));
        assert!(!thread_text.contains("codex role output"));

        let by_native = native_history_lines(home.path(), cwd.path(), "native native-codex");
        let native_text = by_native
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(native_text.contains("filter  native native-codex"));
        assert!(native_text.contains("native native-c"));
        assert!(native_text.contains("codex role output"));
        assert!(!native_text.contains("claude role output"));

        let by_kind = native_history_lines(home.path(), cwd.path(), "kind diff");
        let kind_text = by_kind.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(kind_text.contains("filter  kind diff"));
        assert!(kind_text.contains("kind: diff"));
        assert!(kind_text.contains("codex role output"));
        assert!(!kind_text.contains("claude role output"));

        let by_kind_alias = native_history_lines(home.path(), cwd.path(), "kind assistant-message");
        let alias_text = by_kind_alias
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(alias_text.contains("filter  kind assistant-message"));
        assert!(alias_text.contains("kind: answer"));
        assert!(alias_text.contains("claude role output"));
        assert!(!alias_text.contains("codex role output"));

        let by_permission_alias = native_history_lines(home.path(), cwd.path(), "kind permission");
        let permission_text = by_permission_alias
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            permission_text.contains("no events for kind permission"),
            "no approval events were present in this fixture; got {permission_text}"
        );
    }

    #[test]
    fn native_history_kind_permission_alias_matches_approval_events() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "需要权限".into(),
                result: "等待批准".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker approval · worker-cc · claude-code".into(),
                kind: Some("approval".into()),
                content: Some("waiting for permission approval: write access".into()),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: None,
                provider: None,
                task_id: Some("task-approval".into()),
                worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                native_session_id: Some("native-cc".into()),
            }],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "kind permission");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("filter  kind permission"));
        assert!(text.contains("kind: approval"));
        assert!(text.contains("waiting for permission approval: write access"));

        let compact = native_history_lines(home.path(), cwd.path(), "compact kind permission");
        let compact_text = compact.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(compact_text.contains("filter  kind permission"));
        assert!(compact_text.contains("events  approval"));
        assert!(compact_text.contains("flow: approval needed"));
        assert!(compact_text.contains("resume  /continue worker-cc"));
    }

    #[test]
    fn native_history_approvals_render_resume_hints() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        append_native_chat_turn(
            home.path(),
            cwd.path(),
            &TiffanyOrchestratorTurn {
                user_prompt: "需要权限".into(),
                result: "等待批准".into(),
            },
            vec![TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: "worker approval · worker-cc · claude-code".into(),
                kind: Some("approval".into()),
                content: Some("waiting for command approval: rm -rf target".into()),
                agent: Some("claude-code".into()),
                worker_role: Some("worker-cc".into()),
                model: Some("claude-sonnet-4-6".into()),
                provider: Some("anthropic".into()),
                task_id: Some("task-approval".into()),
                worker_thread_id: Some("abcdef12-0000-0000-0000-000000000000".into()),
                native_session_id: Some("native-cc-session".into()),
            }],
        )
        .expect("append");

        let lines = native_history_lines(home.path(), cwd.path(), "approvals");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("approvals  1 saved approval request(s)"));
        assert!(text.contains("turn 1 · worker-cc · 需要权限 · claude-code"));
        assert!(text.contains("claude-sonnet-4-6"));
        assert!(text.contains("thread: abcdef12"));
        assert!(text.contains("native: native-cc-session"));
        assert!(text.contains("waiting for command approval: rm -rf target"));
        assert!(text.contains("/continue open worker-cc"));
        assert!(text.contains("resume same native CLI conversation"));
        assert!(text.contains("next  /history kind approval"));
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
        assert!(bad_text.contains(
            "Usage: /history [last|full|<count>|role <role>|thread <id>|native <session-id>|kind <event-kind>|search <text>|status|sync|compact|graph|mermaid|export-graph [--out path]|export [role <role>|thread <id>|native <session-id>|kind <event-kind>] [--out path]]"
        ));
    }

    #[test]
    fn native_history_status_suggests_sync_for_local_only_history() {
        let cwd = tempfile::tempdir().expect("cwd");
        let local = sample_native_conversation("local", cwd.path(), 10, 1, 2);

        let lines = native_history_status_lines_from_sources(
            cwd.path(),
            Path::new("/tmp/native-sessions.json"),
            Ok(None),
            Ok(Some(local)),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  local history not synced to session DB"));
        assert!(text.contains("session-db  empty"));
        assert!(text.contains("local-json  available"));
        assert!(text.contains("1 turn(s) · 2 event(s) · updated 10"));
        assert!(text.contains("roles worker-cc"));
        assert!(text.contains("handoff 1 worker thread(s) · 1 native session(s)"));
        assert!(text.contains("events answer"));
        assert!(text.contains("next  /history sync  mirror local native history into session DB"));
        assert!(
            text.contains(
                "next  /history role worker-cc  show saved native transcript for this role"
            )
        );
        assert!(
            text.contains(
                "next  /continue open worker-cc  resume the role's native CLI conversation"
            )
        );
        assert!(text.contains("next  /history kind answer  show saved worker answers"));
        assert!(
            text.contains("next  /thread  inspect role sessions, native ids, and handoff state")
        );
    }

    #[test]
    fn native_history_status_marks_local_newer_than_db() {
        let cwd = tempfile::tempdir().expect("cwd");
        let db = sample_native_conversation("db", cwd.path(), 10, 1, 1);
        let local = sample_native_conversation("local", cwd.path(), 20, 2, 3);

        let lines = native_history_status_lines_from_sources(
            cwd.path(),
            Path::new("/tmp/native-sessions.json"),
            Ok(Some(db)),
            Ok(Some(local)),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  local history is newer than session DB"));
        assert!(text.contains("session-db  available"));
        assert!(text.contains("local-json  available"));
        assert!(text.contains("roles worker-cc"));
        assert!(text.contains("handoff 1 worker thread(s) · 1 native session(s)"));
        assert!(text.contains("events answer"));
        assert!(text.contains("next  /history sync  local native history is newer"));
        assert!(
            text.contains(
                "next  /history role worker-cc  show saved native transcript for this role"
            )
        );
        assert!(
            text.contains(
                "next  /continue open worker-cc  resume the role's native CLI conversation"
            )
        );
        assert!(text.contains("next  /history kind answer  show saved worker answers"));
    }

    #[test]
    fn native_history_status_marks_db_available() {
        let cwd = tempfile::tempdir().expect("cwd");
        let db = sample_native_conversation("db", cwd.path(), 20, 2, 3);
        let local = sample_native_conversation("local", cwd.path(), 20, 2, 3);

        let lines = native_history_status_lines_from_sources(
            cwd.path(),
            Path::new("/tmp/native-sessions.json"),
            Ok(Some(db)),
            Ok(Some(local)),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ history  session DB and local history available"));
        assert!(text.contains("session-db  available"));
        assert!(text.contains("local-json  available"));
        assert!(text.contains("roles worker-cc"));
        assert!(text.contains("handoff 1 worker thread(s) · 1 native session(s)"));
        assert!(text.contains("events answer"));
        assert!(text.contains("next  /history  read saved history"));
        assert!(
            text.contains(
                "next  /history role worker-cc  show saved native transcript for this role"
            )
        );
        assert!(text.contains("next  /history kind answer  show saved worker answers"));
    }

    #[test]
    fn native_history_status_compacts_many_roles_and_event_kinds() {
        let cwd = tempfile::tempdir().expect("cwd");
        let roles = [
            ("critic-codex", "approval"),
            ("planner-codex", "answer"),
            ("reviewer-gemini", "diff"),
            ("worker-cc", "file_update"),
            ("worker-codex", "patch"),
            ("worker-gemini", "tool_result"),
            ("worker-local", "stderr"),
        ];
        let events = roles
            .iter()
            .enumerate()
            .map(|(idx, (role, kind))| TiffanyNativeChatEvent {
                role: "worker".into(),
                status: "output".into(),
                title: format!("{role} {kind}"),
                kind: Some((*kind).into()),
                content: Some(format!("{kind} content")),
                agent: Some("claude-code".into()),
                worker_role: Some((*role).into()),
                model: None,
                provider: None,
                task_id: Some(format!("task-{idx}")),
                worker_thread_id: Some(format!("thread-{idx}")),
                native_session_id: Some(format!("native-{idx}")),
            })
            .collect::<Vec<_>>();
        let conversation = TiffanyNativeChatConversation {
            id: "many".into(),
            cwd: cwd_key(cwd.path()),
            created_at_unix: 1,
            updated_at_unix: 2,
            turns: vec![TiffanyNativeChatTurn {
                user_prompt: "多角色任务".into(),
                result: "完成".into(),
                captured_at_unix: 2,
                events,
            }],
        };

        let lines = native_history_status_lines_from_sources(
            cwd.path(),
            Path::new("/tmp/native-sessions.json"),
            Ok(Some(conversation)),
            Ok(None),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(
            text.contains("roles critic-codex, planner-codex, reviewer-gemini, worker-cc +3 more")
        );
        assert!(text.contains("handoff 7 worker thread(s) · 7 native session(s)"));
        assert!(text.contains("events answer, approval, diff, file_update, patch, stderr +1 more"));
        assert!(
            text.contains(
                "next  /history role worker-cc  show saved native transcript for this role"
            )
        );
        assert!(text.contains("next  /history kind approval  show saved approval requests"));
    }

    #[test]
    fn native_history_sync_output_summarizes_import_report() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"imported Tiffany native history from /tmp/native-sessions.json\n  conversations: 2\n  turns: 5\n  events: 13\n".to_vec(),
            stderr: Vec::new(),
        };

        let lines =
            native_history_sync_output_lines(Path::new("/tmp/native-sessions.json"), output);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("history  synced native history to session DB"));
        assert!(text.contains("source  /tmp/native-sessions.json"));
        assert!(text.contains("target  session-db"));
        assert!(text.contains("imported  2 conversation(s) · 5 turn(s) · 13 event(s)"));
        assert!(text.contains("next  /history  read back from session-db"));
    }

    #[test]
    fn native_history_sync_output_renders_failure_detail() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"database locked\n".to_vec(),
        };

        let lines =
            native_history_sync_output_lines(Path::new("/tmp/native-sessions.json"), output);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✗ history  sync failed"));
        assert!(text.contains("database locked"));
    }

    #[test]
    fn native_history_store_args_reads_current_cwd_from_session_store() {
        let cwd = Path::new("/tmp/tiffany project");

        assert_eq!(
            native_history_store_args(cwd),
            strings(&[
                "sessions",
                "native-history",
                "--cwd",
                "/tmp/tiffany project",
            ])
        );
    }

    #[test]
    fn native_history_lines_can_render_loaded_store_conversation() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let conversation = TiffanyNativeChatConversation {
            id: "db-conversation".into(),
            cwd: cwd_key(cwd.path()),
            created_at_unix: 1,
            updated_at_unix: 2,
            turns: vec![TiffanyNativeChatTurn {
                user_prompt: "修改登录逻辑".into(),
                result: "已同步 diff".into(),
                captured_at_unix: 2,
                events: vec![TiffanyNativeChatEvent {
                    role: "worker".into(),
                    status: "output".into(),
                    title: "worker diff · worker-cc · claude-code".into(),
                    kind: Some("diff".into()),
                    content: Some("diff --git a/src/login.rs b/src/login.rs\n+native sync".into()),
                    agent: Some("claude-code".into()),
                    worker_role: Some("worker-cc".into()),
                    model: Some("claude-sonnet-4-6".into()),
                    provider: Some("anthropic".into()),
                    task_id: Some("task-1".into()),
                    worker_thread_id: Some("thread-1".into()),
                    native_session_id: Some("claude-session-1".into()),
                }],
            }],
        };

        let lines = native_history_lines_from_conversation(
            Ok(Some(NativeHistoryLoadedConversation {
                source: NativeHistorySource::SessionDb,
                conversation,
            })),
            home.path(),
            cwd.path(),
            "full",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("session  db-conversation"));
        assert!(text.contains("source  session-db"));
        assert!(text.contains("user  修改登录逻辑"));
        assert!(text.contains("kind: diff"));
        assert!(text.contains("diff --git a/src/login.rs b/src/login.rs"));
        assert!(text.contains("native claude-s"));
        assert!(text.contains("thread thread-1"));
    }

    #[test]
    fn native_history_loaded_store_preserves_typed_worker_process_events() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let events = vec![
            test_native_worker_event(
                "worker-cc",
                "claude-code",
                "answer",
                "我会先读取规则，然后实现 agent。",
            ),
            test_native_worker_event(
                "worker-cc",
                "claude-code",
                "tool_call",
                "tool Bash: python -m pytest tests/test_agent.py",
            ),
            test_native_worker_event(
                "worker-cc",
                "claude-code",
                "tool_result",
                "tool shell result: exit 0\n2 passed",
            ),
            test_native_worker_event(
                "worker-cc",
                "claude-code",
                "approval",
                "waiting for command approval: rm -rf target",
            ),
            test_native_worker_event(
                "worker-cc",
                "claude-code",
                "diff",
                "diff --git a/agent.py b/agent.py\n+return action",
            ),
            test_native_worker_event(
                "worker-codex",
                "codex",
                "stderr",
                "API Error: 400 [1211][模型不存在]",
            ),
        ];
        let conversation = TiffanyNativeChatConversation {
            id: "db-conversation".into(),
            cwd: cwd_key(cwd.path()),
            created_at_unix: 1,
            updated_at_unix: 2,
            turns: vec![TiffanyNativeChatTurn {
                user_prompt: "写参赛 agent".into(),
                result: "已实现并验证 agent".into(),
                captured_at_unix: 2,
                events,
            }],
        };

        let loaded = Ok(Some(NativeHistoryLoadedConversation {
            source: NativeHistorySource::SessionDb,
            conversation: conversation.clone(),
        }));
        let lines = native_history_lines_from_conversation(loaded, home.path(), cwd.path(), "full");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        for expected in [
            "source  session-db",
            "user  写参赛 agent",
            "kind: answer",
            "我会先读取规则，然后实现 agent。",
            "kind: tool_call",
            "tool Bash: python -m pytest tests/test_agent.py",
            "kind: tool_result",
            "2 passed",
            "kind: approval",
            "waiting for command approval: rm -rf target",
            "kind: diff",
            "diff --git a/agent.py b/agent.py",
            "kind: stderr",
            "[1211][模型不存在]",
            "native claude-",
            "thread thread-w",
        ] {
            assert!(text.contains(expected), "missing {expected:?}\n{text}");
        }

        let compact = native_history_lines_from_conversation(
            Ok(Some(NativeHistoryLoadedConversation {
                source: NativeHistorySource::SessionDb,
                conversation: conversation.clone(),
            })),
            home.path(),
            cwd.path(),
            "compact",
        );
        let compact_text = compact.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(compact_text.contains("answer"));
        assert!(compact_text.contains("tool: tool Bash: python -m pytest tests/test_agent.py"));
        assert!(compact_text.contains("tool result"));
        assert!(compact_text.contains("approval"));
        assert!(compact_text.contains("diff"));
        assert!(compact_text.contains("stderr"));
        assert!(!compact_text.contains('{'));

        let native = native_history_lines_from_conversation(
            Ok(Some(NativeHistoryLoadedConversation {
                source: NativeHistorySource::SessionDb,
                conversation: conversation.clone(),
            })),
            home.path(),
            cwd.path(),
            "native claude-native",
        );
        let native_text = native.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(native_text.contains("source  session-db"));
        assert!(native_text.contains("filter  native claude-native"));
        assert!(native_text.contains("kind: answer"));
        assert!(native_text.contains("kind: diff"));
        assert!(native_text.contains("native claude-"));
        assert!(!native_text.contains("[1211][模型不存在]"));

        let compact_native = native_history_lines_from_conversation(
            Ok(Some(NativeHistoryLoadedConversation {
                source: NativeHistorySource::SessionDb,
                conversation,
            })),
            home.path(),
            cwd.path(),
            "compact native claude-native",
        );
        let compact_native_text = compact_native
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(compact_native_text.contains("filter  native claude-native"));
        assert!(compact_native_text.contains("roles  worker-cc"));
        assert!(compact_native_text.contains("diff"));
        assert!(!compact_native_text.contains("stderr"));
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
    fn jobs_command_args_maps_native_tui_shortcuts() {
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
            jobs_command_args("status").unwrap(),
            strings(&["jobs", "--limit", "20"])
        );
        assert_eq!(
            jobs_command_args("show abcd1234").unwrap(),
            strings(&["jobs", "show", "abcd1234"])
        );
        assert_eq!(
            jobs_command_args("open abcd1234").unwrap(),
            strings(&["jobs", "show", "abcd1234"])
        );
        assert_eq!(
            jobs_command_args("cancel abcd1234").unwrap(),
            strings(&["jobs", "cancel", "abcd1234"])
        );
        assert_eq!(
            jobs_command_args("retry abcd1234").unwrap(),
            strings(&["jobs", "retry", "abcd1234", "--tui-handoff"])
        );
        assert_eq!(
            jobs_command_args("recover").unwrap(),
            strings(&["jobs", "recover"])
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
    fn jobs_retry_output_restores_full_prompt_for_tui_queue() {
        assert_eq!(
            retry_prompt_from_jobs_retry_output(
                "Job old queued for retry as new\n  status: queued\n  retry prompt: first\\nsecond\\tline \\\\ slash\n\nJobs\n"
            )
            .as_deref(),
            Some("first\nsecond\tline \\ slash")
        );
        assert!(retry_prompt_from_jobs_retry_output("Jobs\n  no persisted jobs yet").is_none());
        assert!(retry_prompt_from_jobs_retry_output("retry prompt: trailing \\").is_none());
    }

    #[test]
    fn jobs_retry_handoff_renders_current_tui_queue_card() {
        let output = std::process::Output {
            status: test_exit_status(0),
            stdout: "Job abcd1234 prepared for TUI retry\n  status: queued in current TUI input queue\n  prompt: first line second line\n  retry prompt: first line\\nsecond line\n\nNext:\n  /queue run\n  /jobs\n\nJobs\n  active: 0  shown: 1\n\n✗ abcd1234 failed    first line second line\n  timing created 5m ago  flow direct-answer  role worker-cc  error model not found\n"
                .as_bytes()
                .to_vec(),
            stderr: Vec::new(),
        };

        let lines = concise_jobs_success(
            &strings(&["jobs", "retry", "abcd1234", "--tui-handoff"]),
            &output,
        )
        .expect("jobs retry handoff summary");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ jobs  retry queued · abcd1234"));
        assert!(text.contains("↳ current TUI queue"));
        assert!(text.contains("retry"));
        assert!(text.contains("task  first line second line"));
        assert!(text.contains("state  restored from persisted job"));
        assert!(text.contains("prompt  2 line(s) restored into the current input queue"));
        assert!(text.contains("actions /queue run"));
        assert!(text.contains("/queue show"));
        assert!(text.contains("/jobs show abcd1234"));
        assert!(text.contains("next  /queue run  start the restored retry prompt now"));
        assert!(text.contains("✗ worker-cc"));
        assert!(text.contains("error  model not found"));
        assert!(!text.contains("retry prompt:"));
    }

    #[test]
    fn jobs_recover_output_surfaces_recovery_result_before_cards() {
        let output = std::process::Output {
            status: test_exit_status(0),
            stdout: "Recovered stale jobs\n  failed: 1  ids: abcd1234\n\nJobs\n  active: 0  shown: 1\n\n✗ abcd1234 failed    stale worker task\n  timing created 45m ago · updated just now · duration 45m  flow single-worker  role worker-cc  next orchestrator jobs retry abcd1234 --emit-retry-prompt  error recovered stale running job after 30 minute(s); previous process is no longer tracked\n"
                .as_bytes()
                .to_vec(),
            stderr: Vec::new(),
        };

        let lines = concise_jobs_success(
            &strings(&["jobs", "recover", "--stale-minutes", "30"]),
            &output,
        )
        .expect("jobs recover summary");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("⚠ jobs  recovered stale running job(s) · 1 marked failed"));
        assert!(text.contains("failed: 1  ids: abcd1234"));
        assert!(text.contains("next  /jobs retry abcd1234  retry a recovered stale job"));
        assert!(text.contains("✗ worker-cc"));
        assert!(text.contains("state  needs attention; retry with /jobs retry abcd1234"));
        assert!(text.contains("actions /jobs retry abcd1234"));
        assert!(!text.contains("orchestrator jobs retry"));
    }

    #[test]
    fn jobs_recover_output_explains_when_no_stale_jobs_exist() {
        let output = std::process::Output {
            status: test_exit_status(0),
            stdout: "Recovered stale jobs\n  none older than 30 minute(s)\n\nJobs\n  active: 1  shown: 1\n\n● 22222222 running   still active\n  timing created 1m ago · running 30s  flow single-worker  role worker-codex\n"
                .as_bytes()
                .to_vec(),
            stderr: Vec::new(),
        };

        let lines = concise_jobs_success(
            &strings(&["jobs", "recover", "--stale-minutes", "30"]),
            &output,
        )
        .expect("jobs recover empty summary");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ jobs  no stale running jobs found"));
        assert!(text.contains("none older than 30 minute(s)"));
        assert!(text.contains("next  /jobs  refresh queue status"));
        assert!(text.contains("● worker-codex"));
        assert!(text.contains("/jobs recover"));
    }

    #[test]
    fn continue_target_role_maps_runtime_aliases() {
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
    fn run_intro_uses_direct_flow_for_chat_requests() {
        let launch = test_launch("你好", "你好", 0);
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
        let launch = test_launch(
            "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle",
            "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle",
            0,
        );
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
        let launch = test_launch("写参赛 agent", "写参赛 agent", 0);
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
        let launch = test_launch(prompt, prompt, 1);
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
        let launch = test_launch("优化 TUI 显示", "优化 TUI 显示", 2);
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
        let launch = test_launch("继续", prompt, 1);
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
            roles_command_args("profile dev --planner sonnet@claude-code --worker-cc minimax/MiniMax-M3@claude-code --worker-gemini google/gemini-2.5-pro@gemini")
                .unwrap(),
            strings(&[
                "roles",
                "profile",
                "dev",
                "--planner",
                "sonnet@claude-code",
                "--worker-cc",
                "minimax/MiniMax-M3@claude-code",
                "--worker-gemini",
                "google/gemini-2.5-pro@gemini",
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
             ✗ worker-codex stderr: [1211] 模型不存在\n\
             ✗ model `gpt4o` references missing provider `openai`\n\
             ✗ worker-cc: model=gpt4o -> openai/gpt-4o -> runtime=codex teams=false (provider missing)",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("repair  /provider edit google  set API key/env or endpoint"));
        assert!(text.contains(
            "repair  /provider edit openai  create provider credentials before role binding"
        ));
        assert!(text.contains("repair  /role planner  bind provider, API model, and runtime"));
        assert!(text.contains("repair  /role worker-cc  bind provider, API model, and runtime"));
        assert!(text.contains("repair  install runtime codex  install codex or change"));
        assert!(
            text.contains("repair  /role worker-codex  fix API model name for selected provider")
        );
        assert!(!text.contains("repair  /provider  configure provider auth or endpoint"));
        assert_eq!(text.matches("/role ").count(), 3);
    }

    #[test]
    fn doctor_repair_lines_keep_generic_fallbacks_for_unscoped_issues() {
        let lines = doctor_repair_lines(
            "✗ provider auth failed\n\
             ✗ role/model wiring needs attention\n\
             ✗ runtime binaries missing",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("repair  /provider  configure provider auth or endpoint"));
        assert!(
            text.contains(
                "repair  /role  fix role provider/model/runtime binding and API model name"
            )
        );
        assert!(text.contains("repair  install runtime  install claude/codex/gemini"));
    }

    #[test]
    fn doctor_issue_group_lines_cluster_common_setup_failures() {
        let lines = doctor_issue_group_lines(
            "Issue summary:\n\
             - Provider auth missing for 1 provider(s): google\n\
             - model `gpt4o` references missing provider `openai`\n\
             - Runtime binaries missing: codex: `codex` not found on PATH\n\
             - Role/model wiring needs attention: planner: missing role config\n\
             Details:\n\
             ✗ worker-cc: model=gpt4o -> openai/gpt-4o -> runtime=codex teams=false (provider missing)\n\
             ✗ worker-codex stderr: [1211] 模型不存在\n",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("issue  provider auth"));
        assert!(text.contains("Provider auth missing"));
        assert!(text.contains("missing provider"));
        assert!(text.contains("issue  runtime"));
        assert!(text.contains("codex"));
        assert!(text.contains("issue  role wiring"));
        assert!(text.contains("missing role config"));
        assert!(text.contains("provider missing"));
        assert!(text.contains("模型不存在"));
    }

    #[test]
    fn doctor_issue_group_lines_empty_when_ready() {
        assert!(doctor_issue_group_lines("✓ all required checks passed").is_empty());
    }

    #[test]
    fn doctor_repair_lines_suggest_launch_when_ready() {
        let lines = doctor_repair_lines("✓ all required checks passed");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("repair  tiffany-loop  start an orchestration run"));
    }

    #[test]
    fn doctor_repair_lines_surface_persistence_and_handoff_actions() {
        let lines = doctor_repair_lines(
            "⚠ worker threads: none persisted yet; native handoff starts after first worker run\n\
             ⚠ native history: not created yet (/Users/me/.tiffany/tiffany-orchestrator/native-sessions.json)\n",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("repair  /thread  inspect per-role Tiffany/native session state"));
        assert!(text.contains("repair  /continue open <role>  open the role's native CLI session"));
        assert!(
            text.contains(
                "repair  /history status  verify session DB and local native history sync"
            )
        );
    }

    #[test]
    fn doctor_issue_group_lines_cluster_persistence_failures() {
        let lines = doctor_issue_group_lines(
            "Issue summary:\n\
             - session db path is not a file: /tmp/state.db\n\
             Details:\n\
             ✗ db parent missing: /tmp/missing\n",
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("issue  persistence"));
        assert!(text.contains("session db path is not a file"));
        assert!(text.contains("db parent missing"));
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
    fn provider_setup_success_points_to_role_binding_flow() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"ok\n".to_vec(),
            stderr: Vec::new(),
        };
        let lines = concise_provider_success(
            &strings(&[
                "config",
                "provider",
                "setup",
                "minimax",
                "--type",
                "openai",
                "--env",
                "MINIMAX_API_KEY",
                "--endpoint",
                "https://api.minimaxi.com/v1",
            ]),
            &output,
        )
        .expect("provider setup summary");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ provider  saved minimax"));
        assert!(text.contains("type: openai"));
        assert!(text.contains("auth: $MINIMAX_API_KEY"));
        assert!(text.contains("endpoint: https://api.minimaxi.com/v1"));
        assert!(
            text.contains("next  /role worker-cc  bind a worker role to this provider API model")
        );
        assert!(
            text.contains(
                "next  /roles profile  bind planner, critic, worker, and reviewer together"
            )
        );
        assert!(text.contains("next  /doctor  verify the full chain"));
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
    fn provider_summary_lines_parse_terminal_registry_output() {
        let lines = provider_summary_lines(
            "Provider registry\n\
               configured: 3  models: 4  roles: 5\n\
             \n\
             Providers:\n\
               ✓ anthropic        anthropic  auth=set endpoint=default models=sonnet roles=planner,reviewer\n\
               ⚠ openai           openai     auth=missing endpoint=https://api.openai.com/v1 models=gpt4o roles=worker-codex\n\
               ● ollama           ollama     auth=not-required endpoint=http://localhost:11434 models=local roles=worker-local\n\
             \n\
             Missing providers referenced by models:\n\
               ⚠ minimax\n\
             \n\
             Actions: /provider <name>, tiffany-loop config provider\n\
             Health: provider auth + model binding + role runtime must all be ready; verify with /doctor.\n",
        )
        .expect("provider summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ provider  4 configured"));
        assert!(text.contains("anthropic"));
        assert!(text.contains("auth set"));
        assert!(text.contains("openai"));
        assert!(text.contains("no key"));
        assert!(text.contains("needs auth"));
        assert!(text.contains("● ollama"));
        assert!(text.contains("local"));
        assert!(text.contains("⚠ minimax"));
        assert!(text.contains("missing"));
        assert!(text.contains("models gpt4o"));
        assert!(text.contains("roles worker-codex"));
        assert!(!text.contains("Provider registry"));
        assert!(!text.contains("Actions:"));
        assert!(!text.contains("Health:"));
    }

    #[test]
    fn provider_summary_lines_parse_terminal_detail_output() {
        let lines = provider_summary_lines(
            "Provider openai\n\
               type: openai\n\
               auth: missing\n\
               endpoint: https://api.openai.com/v1\n\
               models: gpt4o\n\
               roles: worker-codex\n\
             \n\
             Model bindings:\n\
               gpt4o -> gpt-4o\n\
             \n\
             Actions: tiffany-loop config provider, /roles save <role> --provider openai --model-name <api-model> --runtime <runtime>, /doctor\n",
        )
        .expect("provider detail summary");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ provider  1 configured"));
        assert!(text.contains("⚠ openai"));
        assert!(text.contains("openai"));
        assert!(text.contains("no key"));
        assert!(text.contains("https://api.openai.com/v1"));
        assert!(text.contains("models gpt4o"));
        assert!(text.contains("roles worker-codex"));
        assert!(!text.contains("Model bindings:"));
        assert!(!text.contains("Actions:"));
    }

    #[test]
    fn role_summary_lines_render_registered_roles() {
        let lines = role_summary_lines(
            "Registered roles:\n\
               critic         model=minimax-m3-claude provider=minimax api_model=MiniMax-M3 runtime=claude-code teams=false health=ready\n\
               worker-cc      model=minimax-m3-claude provider=minimax api_model=MiniMax-M3 runtime=claude-code teams=true health=ready\n\
               worker-codex   model=minimax-m3-codex provider=minimax api_model=MiniMax-M3 runtime=codex teams=false health=runtime-missing:codex thread=00000000 native=codex-native last=11111111\n\
             \n\
             Register: orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime>\n",
            "roles",
        )
        .expect("role summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ roles  3 registered"));
        assert!(text.contains("critic"));
        assert!(text.contains("MiniMax-M3"));
        assert!(text.contains("model id  minimax-m3-codex"));
        assert!(text.contains("api model  minimax/MiniMax-M3"));
        assert!(text.contains("minimax/MiniMax-M3"));
        assert!(text.contains("worker-cc"));
        assert!(text.contains("teams on"));
        assert!(text.contains("session  pending; first worker run creates native session"));
        assert!(text.contains("worker-codex"));
        assert!(text.contains("codex"));
        assert!(text.contains("runtime-missing:codex"));
        assert!(text.contains("session  native codex-native · thread 00000000 · last 11111111"));
        assert!(text.contains("actions /role worker-codex"));
        assert!(text.contains("/continue open worker-codex"));
        assert!(text.contains("/history role worker-codex"));
        assert!(text.contains("actions /role critic"));
        assert!(text.contains("/roles profile"));
        assert!(text.contains("next  /doctor  verify the orchestration chain"));
        assert!(!text.contains("Registered roles:"));
        assert!(!text.contains("Register: orchestrator"));
    }

    #[test]
    fn role_options_lines_render_provider_model_runtime_cards() {
        let lines = role_options_lines(
            "Role options\n\
               providers: anthropic, google, openai\n\
               runtimes: claude-code, codex, gemini\n\
             \n\
             Models:\n\
               ✓ sonnet             provider=anthropic    api_model=claude-sonnet-4-6          runtimes=claude-code            teams=auto roles=worker-cc                         health=ready\n\
               ✓ gpt4o              provider=openai       api_model=gpt-4o                     runtimes=codex                  teams=off  roles=worker-codex,planner,critic,reviewer health=ready\n\
               ✓ gemini-pro         provider=google       api_model=gemini-2.5-pro             runtimes=gemini                 teams=off  roles=worker-gemini                     health=ready\n\
             \n\
             Register: orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime-id>\n",
        )
        .expect("role options lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ role  3 model option(s)"));
        assert!(text.contains("providers  anthropic, google, openai"));
        assert!(text.contains("runtimes  claude-code, codex, gemini"));
        assert!(text.contains("sonnet"));
        assert!(text.contains("anthropic"));
        assert!(text.contains("claude-sonnet-4-6"));
        assert!(text.contains("roles  worker-cc"));
        assert!(text.contains("gpt4o"));
        assert!(text.contains("worker-codex,planner,critic,reviewer"));
        assert!(text.contains("gemini-pro"));
        assert!(text.contains("worker-gemini"));
        assert!(text.contains(
            "/roles register worker-cc --provider anthropic --model-name claude-sonnet-4-6 --runtime claude-code"
        ));
        assert!(text.contains("next  /role <name>"));
        assert!(!text.contains("Role options"));
        assert!(!text.contains("Register: orchestrator"));
    }

    #[test]
    fn concise_roles_success_separates_model_id_from_api_model() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        let lines = concise_roles_success(
            &strings(&[
                "roles",
                "register",
                "worker-cc",
                "--model",
                "sonnet",
                "--runtime",
                "claude-code",
                "--provider",
                "anthropic",
                "--model-name",
                "claude-sonnet-4-6",
                "--agent-teams",
            ]),
            &output,
        )
        .expect("role register summary");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ role  saved worker-cc -> anthropic/claude-sonnet-4-6"));
        assert!(text.contains("provider: anthropic"));
        assert!(text.contains("api model: claude-sonnet-4-6"));
        assert!(text.contains("model id: sonnet"));
        assert!(text.contains("runtime: claude-code"));
        assert!(text.contains("agent teams: on"));
        assert!(text.contains("next  /roles show worker-cc  review saved binding"));
        assert!(text.contains("next  /doctor  verify provider/model/runtime wiring"));
    }

    #[test]
    fn concise_roles_success_renders_delete_as_safe_config_update() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"\xe2\x9c\x93 role worker-codex deleted\n  kept: models, providers, runtimes, worker threads, and session history\n".to_vec(),
            stderr: Vec::new(),
        };
        let lines = concise_roles_success(&strings(&["roles", "delete", "worker-codex"]), &output)
            .expect("role delete summary");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ role  deleted worker-codex"));
        assert!(text.contains("kept models, providers, runtimes, worker threads, and history"));
        assert!(text.contains("next  /roles  review active role bindings"));
        assert!(text.contains("next  /thread clear worker-codex"));
        assert!(text.contains("next  /doctor  check for broken orchestration wiring"));
    }

    #[test]
    fn jobs_summary_lines_render_queue_cards_without_raw_stdout() {
        let lines = jobs_summary_lines(
            "Jobs\n\
               active: 1  shown: 2\n\
             \n\
             ✓ f2dabd3b done      Improve the fake runtime smoke flow again\n\
               timing created 10m ago · updated 2m ago · duration 6m  flow single-worker  role worker-cc  task 99999999  session abcdef12  thread 12345678  native claude-native-session  history /history thread 12345678  next /continue open worker-cc  result Fake Claude worker completed the Tiffany e2e smoke run and wrote README.md\n\
             ✗ abcd1234 failed    queued follow-up\n\
               timing created 5m ago · updated 1m ago · duration 2m  flow direct-answer  role worker-codex  next orchestrator jobs retry abcd1234 --emit-retry-prompt  error {\"message\":\"[1211][模型不存在] invalid model\"}\n\
             \n\
             Use `tiffany-loop` and /jobs to inspect this from the TUI.\n",
        )
        .expect("jobs summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ jobs  1 active · 2 shown"));
        assert!(text.contains("worker-cc"));
        assert!(text.contains("single-worker"));
        assert!(text.contains("f2dabd3b"));
        assert!(text.contains("task  Improve the fake runtime smoke flow again"));
        assert!(text.contains("state  complete; native session is available for handoff"));
        assert!(text.contains("task id  99999999"));
        assert!(text.contains("timing  created 10m ago · updated 2m ago · duration 6m"));
        assert!(text.contains("session  abcdef12"));
        assert!(text.contains("thread  12345678"));
        assert!(text.contains("native  claude-native-session"));
        assert!(text.contains("handoff  ready; use /continue open worker-cc"));
        assert!(text.contains("result  Fake Claude worker completed"));
        assert!(text.contains("✗ worker-codex"));
        assert!(text.contains("direct-answer"));
        assert!(text.contains("state  needs attention; retry with /jobs retry abcd1234"));
        assert!(text.contains("timing  created 5m ago · updated 1m ago · duration 2m"));
        assert!(text.contains("error  [1211][模型不存在] invalid model"));
        assert!(text.contains("/jobs retry abcd1234"));
        assert!(text.contains("actions /continue open worker-cc"));
        assert!(text.contains("/thread export worker-cc"));
        assert!(text.contains("/history thread 12345678"));
        assert!(text.contains("/jobs"));
        assert!(text.contains("/process 200"));
        assert!(text.contains("/thread worker-cc"));
        assert!(text.contains("/history role worker-codex"));
        assert!(!text.contains("orchestrator jobs retry"));
        assert!(text.contains("next  /jobs 50"));
        assert!(!text.contains("Use `tiffany-loop`"));
        assert!(!text.contains("{\"message\""));
    }

    #[test]
    fn jobs_summary_lines_render_empty_queue_hint() {
        let lines = jobs_summary_lines("Jobs\n  no persisted jobs yet\n")
            .expect("empty jobs summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ jobs  queue is empty"));
        assert!(text.contains("queued and background runs will appear here"));
        assert!(text.contains("next  send a prompt"));
    }

    #[test]
    fn jobs_summary_lines_use_status_specific_actions() {
        let lines = jobs_summary_lines(
            "Jobs\n\
               active: 2  shown: 3\n\
             \n\
             ↳ 11111111 queued    queued follow-up\n\
               timing created 4m ago  flow direct-answer  role worker-cc\n\
             ● 22222222 running   active task\n\
               timing created 3m ago · running 42s  flow single-worker  role worker-codex  session abcdef12  thread 2222abcd  history /history thread 2222abcd\n\
             ✓ 33333333 done      completed without native session\n\
               timing created 2m ago · duration 12s  flow direct-answer  session 12345678  result done text\n",
        )
        .expect("jobs summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("↳ worker-cc"));
        assert!(text.contains("state  waiting in the Tiffany queue"));
        assert!(text.contains("timing  created 4m ago"));
        assert!(text.contains("actions /jobs show 11111111"));
        assert!(text.contains("/jobs show 11111111"));
        assert!(text.contains("/jobs cancel 11111111"));
        assert!(text.contains("/queue run"));
        assert!(text.contains("/queue show"));
        assert!(text.contains("● worker-codex"));
        assert!(text.contains("state  active worker run"));
        assert!(text.contains("timing  created 3m ago · running 42s"));
        assert!(text.contains("thread  2222abcd"));
        assert!(text.contains("actions /jobs show 22222222"));
        assert!(text.contains("/jobs show 22222222"));
        assert!(text.contains("/jobs cancel 22222222"));
        assert!(text.contains("/jobs"));
        assert!(text.contains("/process 200"));
        assert!(text.contains("/jobs recover"));
        assert!(text.contains("/history thread 2222abcd"));
        assert!(text.contains("✓ worker"));
        assert!(text.contains("state  complete; result captured in Tiffany history"));
        assert!(text.contains("timing  created 2m ago · duration 12s"));
        assert!(text.contains("/history session 12345678"));
        assert!(!text.contains("/continue open worker"));
    }

    #[test]
    fn jobs_summary_lines_translate_cli_next_actions_to_tui_commands() {
        let lines = jobs_summary_lines(
            "Jobs\n\
               active: 0  shown: 3\n\
             \n\
             ✗ abcd1234 failed    failed worker\n\
               timing created 1m ago  flow single-worker  role worker-codex  next orchestrator jobs retry abcd1234 --emit-retry-prompt  error model unavailable\n\
             ✓ 22222222 done      native done\n\
               timing created 2m ago  flow direct-answer  role worker-gemini  native gemini-native-session  next open tiffany-loop and run /continue open worker-gemini  result done\n\
             ✓ 33333333 done      session done\n\
               timing created 3m ago  flow direct-answer  session session-3333  next orchestrator sessions show session-3333  result done\n",
        )
        .expect("jobs summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("actions /jobs retry abcd1234"));
        assert!(text.contains("actions /continue open worker-gemini"));
        assert!(text.contains("actions /history session session-3333"));
        assert!(!text.contains("orchestrator jobs retry"));
        assert!(!text.contains("open tiffany-loop and run"));
        assert!(!text.contains("orchestrator sessions show"));
    }

    #[test]
    fn jobs_parser_keeps_result_and_error_fields_separate() {
        let jobs = parse_job_summaries(
            "Jobs\n\
               active: 0  shown: 2\n\
             \n\
             ✓ 12345678 done      answer user question\n\
               timing created 1m ago  flow direct-answer  role worker-cc  result final answer with several words\n\
             ✗ 87654321 failed    answer another question\n\
               timing created 2m ago  flow single-worker  role worker-codex  error model not found with several words\n",
        );

        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].flow.as_deref(), Some("direct-answer"));
        assert_eq!(jobs[0].role.as_deref(), Some("worker-cc"));
        assert_eq!(jobs[0].timing.as_deref(), Some("created 1m ago"));
        assert_eq!(
            jobs[0].result.as_deref(),
            Some("final answer with several words")
        );
        assert_eq!(jobs[1].flow.as_deref(), Some("single-worker"));
        assert_eq!(jobs[1].role.as_deref(), Some("worker-codex"));
        assert_eq!(jobs[1].timing.as_deref(), Some("created 2m ago"));
        assert_eq!(
            jobs[1].error.as_deref(),
            Some("model not found with several words")
        );
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
        assert!(text.contains("last  11111111"));
        assert!(text.contains("actions /thread worker-codex"));
        assert!(text.contains("/continue open worker-codex"));
        assert!(text.contains("/history role worker-codex"));
        assert!(text.contains("/thread export worker-codex"));
        assert!(text.contains("run a task to create a resumable native session"));
        assert!(text.contains("next  /thread <role>"));
        assert!(text.contains("next  /history role <role>"));
        assert!(
            text.contains("next  /history kind answer|tool_result|diff|approval|session_recovery")
        );
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
               scope: cwd:/tmp/tiffany-worker\n\
               Tiffany thread: 00000000-0000-0000-0000-000000000123\n\
               native session: codex-native-session\n\
               native resume: codex exec resume codex-native-session\n\
               native handoff: cd /tmp/tiffany-worker && codex resume codex-native-session\n\
               TUI resume: /continue open worker-codex\n\
               legacy handoff: /continue codex\n\
               last Tiffany session: 00000000-0000-0000-0000-000000000456\n\
               worktree: /tmp/tiffany-worker\n\
             \n\
             Status: ready for native resume; Tiffany will reuse this session for the same role\n\
             Action: orchestrator thread clear worker-codex resets only the native CLI session id for a fresh next run.\n",
        )
        .expect("thread detail summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ thread  worker-codex session card"));
        assert!(text.contains("worker-codex  codex  openai/gpt-4o"));
        assert!(text.contains("ready for native resume; Tiffany will reuse this session"));
        assert!(text.contains("agent  codex"));
        assert!(text.contains("provider  openai"));
        assert!(text.contains("scope  cwd:/tmp/tiffany-worker"));
        assert!(text.contains("native  codex-native-session"));
        assert!(text.contains("resume  codex exec resume codex-native-session"));
        assert!(
            text.contains("handoff  cd /tmp/tiffany-worker && codex resume codex-native-session")
        );
        assert!(text.contains("tui  /continue open worker-codex"));
        assert!(text.contains("legacy  /continue codex"));
        assert!(text.contains("last  00000000-0000-0000-0000-000000000456"));
        assert!(text.contains("next  /continue open worker-codex"));
        assert!(text.contains("next  /history role worker-codex"));
        assert!(
            text.contains("next  /history kind answer|tool_result|diff|approval|session_recovery")
        );
        assert!(text.contains("next  /history thread 00000000"));
        assert!(text.contains("next  /thread clear worker-codex"));
        assert!(!text.contains("Worker thread 00000000"));
        assert!(!text.contains("Action: orchestrator"));
    }

    #[test]
    fn continue_summary_lines_render_native_handoff_command() {
        let lines = continue_summary_lines(
            "Worker thread 00000000\n\
               role: worker-gemini\n\
               runtime: gemini\n\
               agent: gemini\n\
               model: google/gemini-2.5-pro\n\
               provider: google\n\
               Tiffany thread: 00000000-0000-0000-0000-000000000123\n\
               native session: latest\n\
               native resume: gemini --resume latest\n\
               native handoff: cd /tmp/tiffany-worker && gemini --resume latest\n\
               legacy handoff: /continue gemini\n\
               last Tiffany session: 00000000-0000-0000-0000-000000000456\n\
               worktree: /tmp/tiffany-worker\n\
             \n\
             Status: ready for Gemini native resume; Tiffany will reuse latest/index for the same role\n",
        )
        .expect("continue summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("✓ continue  worker-gemini native handoff ready"));
        assert!(text.contains("runtime  gemini"));
        assert!(text.contains("provider  google"));
        assert!(text.contains("native  latest"));
        assert!(text.contains("thread  00000000-0000-0000-0000-000000000123"));
        assert!(text.contains("last  00000000-0000-0000-0000-000000000456"));
        assert!(text.contains("command  cd /tmp/tiffany-worker && gemini --resume latest"));
        assert!(text.contains("legacy  /continue gemini"));
        assert!(text.contains("work  /tmp/tiffany-worker"));
        assert!(
            text.contains("capture  native transcript, git status, and diff return to Tiffany")
        );
        assert!(text.contains("next  copy command"));
        assert!(text.contains("next  /thread export worker-gemini"));
        assert!(!text.contains("Worker thread 00000000"));
        assert!(!text.contains("Status: ready for Gemini"));
    }

    #[test]
    fn continue_open_output_renders_context_and_emits_native_cli_event() {
        let (text, command) = continue_open_output_text_and_command(
            "Worker thread 00000000\n\
               role: worker-codex\n\
               runtime: codex\n\
               agent: codex\n\
               model: openai/gpt-4o\n\
               provider: openai\n\
               Tiffany thread: 00000000-0000-0000-0000-000000000123\n\
               native session: codex-native-session\n\
               native resume: codex exec resume codex-native-session\n\
               native handoff: cd /tmp/tiffany-worker && codex resume codex-native-session\n\
               last Tiffany session: 00000000-0000-0000-0000-000000000456\n\
               worktree: /tmp/tiffany-worker\n",
        );

        assert!(text.contains("● continue  opening worker-codex native CLI"));
        assert!(text.contains("runtime  codex"));
        assert!(text.contains("provider  openai"));
        assert!(text.contains("native  codex-native-session"));
        assert!(text.contains("thread  00000000-0000-0000-0000-000000000123"));
        assert!(text.contains("last  00000000-0000-0000-0000-000000000456"));
        assert!(
            text.contains("command  cd /tmp/tiffany-worker && codex resume codex-native-session")
        );
        assert!(text.contains("work  /tmp/tiffany-worker"));
        assert!(
            text.contains("capture  native transcript, git status, and diff return to Tiffany")
        );
        assert!(text.contains("Tiffany will pause and return after the native CLI exits."));
        assert!(text.contains("next  /history status"));

        let command = command.expect("open native CLI command");
        assert_eq!(command.role, "worker-codex");
        assert_eq!(command.runtime.as_deref(), Some("codex"));
        assert_eq!(
            command.worker_thread_id.as_deref(),
            Some("00000000-0000-0000-0000-000000000123")
        );
        assert_eq!(command.native_session, "codex-native-session");
        assert_eq!(
            command.command,
            "cd /tmp/tiffany-worker && codex resume codex-native-session"
        );
        assert_eq!(command.worktree.as_deref(), Some("/tmp/tiffany-worker"));
    }

    #[test]
    fn native_cli_command_from_thread_output_reads_handoff_command() {
        let command = native_cli_command_from_thread_output(
            "Worker thread 00000000\n\
               role: worker-cc\n\
               runtime: claude-code\n\
               native session: claude-native-session\n\
               native resume: claude --resume claude-native-session\n\
               native handoff: cd /tmp/tiffany-worker && claude --resume claude-native-session\n\
               worktree: /tmp/tiffany-worker\n",
        )
        .expect("native command");

        assert_eq!(command.role, "worker-cc");
        assert_eq!(command.runtime.as_deref(), Some("claude-code"));
        assert_eq!(command.native_session, "claude-native-session");
        assert_eq!(
            command.command,
            "cd /tmp/tiffany-worker && claude --resume claude-native-session"
        );
        assert_eq!(command.worktree.as_deref(), Some("/tmp/tiffany-worker"));
    }

    #[test]
    fn native_cli_command_from_thread_output_prefers_codex_interactive_handoff() {
        let command = native_cli_command_from_thread_output(
            "Worker thread 00000000\n\
               role: worker-codex\n\
               runtime: codex\n\
               native session: codex-native-session\n\
               native resume: codex exec resume codex-native-session\n\
               native handoff: cd /tmp/tiffany-worker && codex resume codex-native-session\n\
               worktree: /tmp/tiffany-worker\n",
        )
        .expect("native command");

        assert_eq!(command.role, "worker-codex");
        assert_eq!(command.runtime.as_deref(), Some("codex"));
        assert_eq!(command.native_session, "codex-native-session");
        assert_eq!(
            command.command,
            "cd /tmp/tiffany-worker && codex resume codex-native-session"
        );
        assert!(!command.command.contains("codex exec resume"));
    }

    #[test]
    fn native_cli_command_detects_runtime_with_custom_binary() {
        let command = native_cli_command_from_thread_output(
            "Worker thread 00000000\n\
               role: executor\n\
               runtime: codex\n\
               native session: codex-native-session\n\
               native resume: worker-codex exec resume codex-native-session\n\
               native handoff: cd /tmp/tiffany-worker && worker-codex resume codex-native-session\n\
               worktree: /tmp/tiffany-worker\n",
        )
        .expect("native command");

        assert_eq!(command.role, "executor");
        assert_eq!(command.runtime.as_deref(), Some("codex"));
        assert!(native_cli_command_is_codex(&command));
        assert!(!native_cli_command_is_claude(&command));
        assert!(!native_cli_command_is_gemini(&command));
    }

    #[test]
    fn native_cli_command_from_thread_output_reads_gemini_latest_handoff() {
        let command = native_cli_command_from_thread_output(
            "Worker thread 00000000\n\
               role: worker-gemini\n\
               runtime: gemini\n\
               native session: latest\n\
               native resume: gemini --resume latest\n\
               native handoff: cd /tmp/tiffany-worker && gemini --resume latest\n\
               worktree: /tmp/tiffany-worker\n",
        )
        .expect("native command");

        assert_eq!(command.role, "worker-gemini");
        assert_eq!(command.runtime.as_deref(), Some("gemini"));
        assert_eq!(command.native_session, "latest");
        assert_eq!(
            command.command,
            "cd /tmp/tiffany-worker && gemini --resume latest"
        );
        assert_eq!(command.worktree.as_deref(), Some("/tmp/tiffany-worker"));
    }

    #[test]
    fn native_cli_return_lines_show_workspace_snapshot() {
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: Some("thread-claude".into()),
            native_session: "claude-native-session".into(),
            command: "cd /tmp/repo && claude --resume claude-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let outcome = TiffanyNativeCliReturn {
            status: Some(" M src/lib.rs\n?? tests/new.rs".into()),
            diff_stat: Some(" src/lib.rs | 2 ++\n 1 file changed, 2 insertions(+)".into()),
            diff_patch: Some(
                "diff --git a/src/lib.rs b/src/lib.rs\n@@ -1 +1,2 @@\n-old\n+new\n+line".into(),
            ),
            staged_diff_stat: Some(" README.md | 1 +".into()),
            staged_diff_patch: Some(
                "diff --git a/README.md b/README.md\n@@ -1 +1,2 @@\n title\n+staged".into(),
            ),
            transcript_event_count: 4,
            transcript_preview: Some("原生 Claude 已完成修改。\n请看 diff。".into()),
        };

        let text = native_cli_return_lines(&command, &outcome)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("✓ worker-cc  native CLI exited with workspace changes"));
        assert!(text.contains("native  claude-native-session"));
        assert!(text.contains("runtime  claude-code"));
        assert!(text.contains("command  cd /tmp/repo && claude --resume claude-native-session"));
        assert!(text.contains("thread  thread-claude"));
        assert!(text.contains("work  /tmp/repo"));
        assert!(text.contains("native events  4 captured from Claude transcript"));
        assert!(text.contains("captured  Tiffany native history updated with returned transcript"));
        assert!(text.contains("native answer  Claude transcript"));
        assert!(text.contains("原生 Claude 已完成修改。"));
        assert!(text.contains("status  git status --short"));
        assert!(text.contains("M src/lib.rs"));
        assert!(text.contains("diff  git diff --stat"));
        assert!(text.contains("1 file changed"));
        assert!(text.contains("patch  git diff"));
        assert!(text.contains("diff --git a/src/lib.rs b/src/lib.rs"));
        assert!(text.contains("+new"));
        assert!(text.contains("staged  git diff --cached --stat"));
        assert!(text.contains("README.md"));
        assert!(text.contains("patch  git diff --cached"));
        assert!(text.contains("+staged"));
        assert!(
            text.contains("workspace  workspace status and diff captured into Tiffany history")
        );
        assert!(text.contains("next  /history kind diff  inspect full saved native patch"));
        assert!(
            text.contains(
                "next  /continue open worker-cc  resume the same native CLI conversation"
            )
        );
        assert!(text.contains(
            "next  /history role worker-cc  show returned native transcript, tools, answers, and diff"
        ));
        assert!(text.contains(
            "next  /thread worker-cc  confirm native session and last Tiffany session pointer"
        ));
        assert!(
            text.contains("next  /thread export worker-cc  write full selectable handoff history")
        );
    }

    #[test]
    fn native_cli_return_lines_show_history_persistence_status() {
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: None,
            native_session: "claude-native-session".into(),
            command: "cd /tmp/repo && claude --resume claude-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let outcome = TiffanyNativeCliReturn {
            transcript_event_count: 1,
            transcript_preview: Some("done".into()),
            ..TiffanyNativeCliReturn::default()
        };

        let text = native_cli_return_lines_with_history_status(
            &command,
            &outcome,
            None,
            Some(Path::new("/tmp/native-sessions.json")),
            true,
            None,
        )
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("history  /tmp/native-sessions.json"));
        assert!(text.contains("session-db  import queued from local native history"));
        assert!(text.contains(
            "next  /history status  verify session-db/local-json sync after native return"
        ));

        let text = native_cli_return_lines_with_history_status(
            &command,
            &outcome,
            None,
            Some(Path::new("/tmp/native-sessions.json")),
            false,
            None,
        )
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("session-db  not imported yet; run /history sync"));

        let text = native_cli_return_lines_with_history_status(
            &command,
            &outcome,
            None,
            None,
            false,
            Some("permission denied"),
        )
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("history  local save failed"));
        assert!(text.contains("permission denied"));
    }

    #[test]
    fn native_cli_return_lines_keep_transcript_for_nonzero_exit() {
        let command = TiffanyNativeCliCommand {
            role: "worker-codex".into(),
            runtime: Some("codex".into()),
            worker_thread_id: None,
            native_session: "codex-native-session".into(),
            command: "cd /tmp/repo && codex resume codex-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let outcome = TiffanyNativeCliReturn {
            transcript_event_count: 2,
            transcript_preview: Some("Codex 保存了部分输出。".into()),
            ..TiffanyNativeCliReturn::default()
        };
        let status = test_exit_status(1);

        let text = native_cli_return_lines_with_exit_status(&command, &outcome, Some(&status))
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("⚠ worker-codex  native CLI exited with non-zero status"));
        assert!(text.contains("exit  exit status: 1"));
        assert!(text.contains("native events  2 captured from Codex transcript"));
        assert!(text.contains("captured  Tiffany native history updated with returned transcript"));
        assert!(text.contains("native answer  Codex transcript"));
        assert!(text.contains("Codex 保存了部分输出。"));
        assert!(text.contains("next  /thread export worker-codex"));
    }

    #[test]
    fn native_cli_return_lines_explain_missing_transcript_capture() {
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: None,
            native_session: "claude-native-session".into(),
            command: "cd /tmp/repo && claude --resume claude-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let outcome = TiffanyNativeCliReturn::default();

        let text = native_cli_return_lines(&command, &outcome)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("✓ worker-cc  native CLI exited"));
        assert!(text.contains("native events  none captured from Claude transcript"));
        assert!(text.contains(
            "next  /history status  check session-db and local native transcript availability"
        ));
        assert!(text.contains("next  /continue open worker-cc"));
        assert!(text.contains("next  /history role worker-cc"));
        assert!(text.contains("next  /thread worker-cc"));
    }

    #[test]
    fn native_cli_return_appends_diff_history() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: Some("thread-claude".into()),
            native_session: "claude-native-session".into(),
            command: "claude --resume claude-native-session".into(),
            worktree: Some(cwd.path().display().to_string()),
        };
        let outcome = TiffanyNativeCliReturn {
            status: Some(" M README.md".into()),
            diff_stat: Some(" README.md | 1 +".into()),
            diff_patch: Some("diff --git a/README.md b/README.md\n+unstaged".into()),
            staged_diff_stat: Some(" src/lib.rs | 1 +".into()),
            staged_diff_patch: Some("diff --git a/src/lib.rs b/src/lib.rs\n+staged".into()),
            transcript_event_count: 0,
            transcript_preview: Some("native final answer".into()),
        };

        append_native_cli_return(home.path(), cwd.path(), &command, &outcome, Vec::new())
            .expect("append return")
            .expect("sessions path");
        let loaded = load_native_chat_conversation(home.path(), cwd.path())
            .expect("load native history")
            .expect("conversation");

        assert_eq!(loaded.turns.len(), 1);
        let turn = &loaded.turns[0];
        assert_eq!(turn.user_prompt, "/continue open worker-cc");
        assert_eq!(turn.result, "native final answer");
        assert_eq!(turn.events.len(), 3);
        assert_eq!(turn.events[0].kind.as_deref(), Some("file_update"));
        assert_eq!(turn.events[1].kind.as_deref(), Some("diff"));
        assert_eq!(turn.events[2].kind.as_deref(), Some("diff"));
        assert!(
            turn.events[1]
                .content
                .as_deref()
                .unwrap()
                .contains("README.md")
        );
        assert!(
            turn.events[1]
                .content
                .as_deref()
                .unwrap()
                .contains("+unstaged")
        );
        assert!(
            turn.events[2]
                .content
                .as_deref()
                .unwrap()
                .contains("src/lib.rs")
        );
        assert!(
            turn.events[2]
                .content
                .as_deref()
                .unwrap()
                .contains("+staged")
        );
        assert_eq!(
            turn.events[1].native_session_id.as_deref(),
            Some("claude-native-session")
        );
        assert_eq!(
            turn.events[1].worker_thread_id.as_deref(),
            Some("thread-claude")
        );
    }

    #[test]
    fn native_cli_return_appends_transcript_history_with_worker_thread() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: Some("thread-claude".into()),
            native_session: "claude-native-session".into(),
            command: "claude --resume claude-native-session".into(),
            worktree: Some(cwd.path().display().to_string()),
        };
        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"我会继续处理。"}]}}
{"type":"result","result":"已完成原生会话里的后续工作。"}
"#;
        let transcript_events = native_cli_transcript_events_from_claude_jsonl(&command, jsonl);
        let outcome = TiffanyNativeCliReturn {
            transcript_event_count: transcript_events.len(),
            transcript_preview: native_cli_transcript_preview(&transcript_events),
            ..TiffanyNativeCliReturn::default()
        };

        append_native_cli_return(
            home.path(),
            cwd.path(),
            &command,
            &outcome,
            transcript_events,
        )
        .expect("append return")
        .expect("sessions path");
        let loaded = load_native_chat_conversation(home.path(), cwd.path())
            .expect("load native history")
            .expect("conversation");

        assert_eq!(loaded.turns.len(), 1);
        let turn = &loaded.turns[0];
        assert_eq!(turn.result, "已完成原生会话里的后续工作。");
        assert_eq!(turn.events.len(), 2);
        assert_eq!(turn.events[0].kind.as_deref(), Some("answer"));
        assert_eq!(turn.events[1].kind.as_deref(), Some("final"));
        assert!(
            turn.events
                .iter()
                .all(|event| event.worker_thread_id.as_deref() == Some("thread-claude"))
        );
        assert!(
            turn.events
                .iter()
                .all(|event| event.native_session_id.as_deref() == Some("claude-native-session"))
        );
    }

    #[test]
    fn native_cli_return_appends_empty_return_history_when_transcript_missing() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: None,
            native_session: "claude-native-session".into(),
            command: "claude --resume claude-native-session".into(),
            worktree: Some(cwd.path().display().to_string()),
        };
        let outcome = TiffanyNativeCliReturn::default();

        append_native_cli_return(home.path(), cwd.path(), &command, &outcome, Vec::new())
            .expect("append return")
            .expect("sessions path");
        let loaded = load_native_chat_conversation(home.path(), cwd.path())
            .expect("load native history")
            .expect("conversation");

        assert_eq!(loaded.turns.len(), 1);
        let turn = &loaded.turns[0];
        assert_eq!(turn.user_prompt, "/continue open worker-cc");
        assert_eq!(turn.result, "native CLI returned to Tiffany");
        assert_eq!(turn.events.len(), 1);
        assert_eq!(turn.events[0].kind.as_deref(), Some("status"));
        assert_eq!(
            turn.events[0].native_session_id.as_deref(),
            Some("claude-native-session")
        );
        assert!(
            turn.events[0]
                .content
                .as_deref()
                .unwrap()
                .contains("without captured transcript or workspace changes")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_claude_jsonl_without_raw_json() {
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: None,
            native_session: "claude-native-session".into(),
            command: "claude --resume claude-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"我会先检查文件。"},{"type":"tool_use","name":"Read","input":{"file_path":"README.md"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"README body"}]}}
{"type":"result","result":"完成。"}
"#;

        let events = native_cli_transcript_events_from_claude_jsonl(&command, jsonl);

        assert_eq!(events.len(), 4);
        assert_eq!(events[0].kind.as_deref(), Some("answer"));
        assert_eq!(events[0].content.as_deref(), Some("我会先检查文件。"));
        assert_eq!(events[1].kind.as_deref(), Some("tool_call"));
        assert!(events[1].content.as_deref().unwrap().contains("tool Read"));
        assert!(
            events[1]
                .content
                .as_deref()
                .unwrap()
                .contains("file_path=README.md")
        );
        assert!(!events[1].content.as_deref().unwrap().contains("\"type\""));
        assert!(
            !events[1]
                .content
                .as_deref()
                .unwrap()
                .contains("\"file_path\"")
        );
        assert_eq!(events[2].kind.as_deref(), Some("tool_result"));
        assert_eq!(events[2].content.as_deref(), Some("README body"));
        assert_eq!(events[3].kind.as_deref(), Some("final"));
        assert_eq!(
            events[3].native_session_id.as_deref(),
            Some("claude-native-session")
        );

        assert_eq!(
            native_cli_transcript_preview(&events).as_deref(),
            Some("完成。")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_claude_permission_error_as_approval() {
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: None,
            native_session: "claude-native-session".into(),
            command: "claude --resume claude-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","is_error":true,"content":"User did not allow tool call: Edit README.md"}]}}
"#;

        let events = native_cli_transcript_events_from_claude_jsonl(&command, jsonl);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind.as_deref(), Some("approval"));
        assert!(events[0].title.contains("native approval"));
        assert!(
            events[0]
                .content
                .as_deref()
                .unwrap()
                .contains("User did not allow")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_claude_edit_as_file_update() {
        let command = TiffanyNativeCliCommand {
            role: "worker-cc".into(),
            runtime: Some("claude-code".into()),
            worker_thread_id: None,
            native_session: "claude-native-session".into(),
            command: "claude --resume claude-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"old","new_string":"new"}}]}}
"#;

        let events = native_cli_transcript_events_from_claude_jsonl(&command, jsonl);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind.as_deref(), Some("file_update"));
        assert!(events[0].title.contains("native file update"));
        assert!(
            events[0]
                .content
                .as_deref()
                .unwrap()
                .contains("file_path=src/lib.rs")
        );
        assert!(
            !events[0]
                .content
                .as_deref()
                .unwrap()
                .contains("\"file_path\"")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_codex_rollout_without_raw_json() {
        let command = TiffanyNativeCliCommand {
            role: "worker-codex".into(),
            runtime: Some("codex".into()),
            worker_thread_id: None,
            native_session: "123e4567-e89b-12d3-a456-426614174000".into(),
            command: "codex resume 123e4567-e89b-12d3-a456-426614174000".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let jsonl = r#"{"timestamp":"2026-06-26T00:00:00Z","item":{"event_msg":{"agent_message":{"message":"我会检查代码。"}}}}
{"timestamp":"2026-06-26T00:00:01Z","item":{"event_msg":{"exec_command_begin":{"command":"cargo test"}}}}
{"timestamp":"2026-06-26T00:00:02Z","item":{"event_msg":{"exec_command_end":{"exit_code":0,"stdout":"ok"}}}}
{"timestamp":"2026-06-26T00:00:03Z","item":{"event_msg":{"turn_diff":{"diff":"diff --git a/lib.rs b/lib.rs\n+done"}}}}
"#;

        let events = native_cli_transcript_events_from_codex_jsonl(&command, jsonl);

        assert_eq!(events.len(), 4);
        assert_eq!(events[0].kind.as_deref(), Some("answer"));
        assert_eq!(events[0].agent.as_deref(), Some("codex"));
        assert_eq!(events[0].content.as_deref(), Some("我会检查代码。"));
        assert_eq!(events[1].kind.as_deref(), Some("tool_call"));
        assert_eq!(events[1].content.as_deref(), Some("shell: cargo test"));
        assert_eq!(events[2].kind.as_deref(), Some("tool_result"));
        assert!(events[2].content.as_deref().unwrap().contains("exit 0"));
        assert!(events[2].content.as_deref().unwrap().contains("ok"));
        assert_eq!(events[3].kind.as_deref(), Some("diff"));
        assert!(events[3].content.as_deref().unwrap().contains("diff --git"));
        assert!(
            !events[3]
                .content
                .as_deref()
                .unwrap()
                .contains("\"turn_diff\"")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_codex_permission_end_as_approval() {
        let command = TiffanyNativeCliCommand {
            role: "worker-codex".into(),
            runtime: Some("codex".into()),
            worker_thread_id: None,
            native_session: "123e4567-e89b-12d3-a456-426614174000".into(),
            command: "codex resume 123e4567-e89b-12d3-a456-426614174000".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let jsonl = r#"{"item":{"event_msg":{"exec_command_end":{"exit_code":1,"stderr":"command requires approval: rm -rf target"}}}}
"#;

        let events = native_cli_transcript_events_from_codex_jsonl(&command, jsonl);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind.as_deref(), Some("approval"));
        assert!(events[0].title.contains("native approval"));
        assert!(
            events[0]
                .content
                .as_deref()
                .unwrap()
                .contains("command requires approval")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_gemini_chat_without_raw_json() {
        let command = TiffanyNativeCliCommand {
            role: "worker-gemini".into(),
            runtime: Some("gemini".into()),
            worker_thread_id: None,
            native_session: "gemini-native-session".into(),
            command: "gemini --resume gemini-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let json = r#"{
  "sessionId": "gemini-native-session",
  "projectHash": "hash",
  "messages": [
    {"type":"user","content":"你好"},
    {"type":"gemini","content":"我会检查文件。","thoughts":[{"subject":"Plan","description":"Inspect the project first."}],"toolCalls":[{"name":"read_file","displayName":"Read","args":{"file_path":"/tmp/repo/README.md"},"result":[{"functionResponse":{"response":{"output":"README body"}}}],"status":"success"}]},
    {"type":"error","content":"API Error: Premature close"}
  ]
}"#;

        let events = native_cli_transcript_events_from_gemini_json(&command, json, 0);

        assert_eq!(events.len(), 6);
        assert_eq!(events[0].kind.as_deref(), Some("question"));
        assert_eq!(events[0].agent.as_deref(), Some("gemini"));
        assert_eq!(events[0].content.as_deref(), Some("你好"));
        assert_eq!(events[1].kind.as_deref(), Some("answer"));
        assert_eq!(events[1].content.as_deref(), Some("我会检查文件。"));
        assert_eq!(events[2].kind.as_deref(), Some("output"));
        assert!(events[2].content.as_deref().unwrap().contains("Plan"));
        assert_eq!(events[3].kind.as_deref(), Some("tool_call"));
        assert!(events[3].content.as_deref().unwrap().contains("tool Read"));
        assert!(
            events[3]
                .content
                .as_deref()
                .unwrap()
                .contains("/tmp/repo/README.md")
        );
        assert!(
            !events[3]
                .content
                .as_deref()
                .unwrap()
                .contains("\"file_path\"")
        );
        assert_eq!(events[4].kind.as_deref(), Some("tool_result"));
        assert_eq!(events[4].content.as_deref(), Some("README body"));
        assert_eq!(events[5].kind.as_deref(), Some("stderr"));
        assert_eq!(
            events[5].content.as_deref(),
            Some("API Error: Premature close")
        );
        assert_eq!(
            native_cli_transcript_preview(&events).as_deref(),
            Some("我会检查文件。")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_gemini_edit_as_file_update() {
        let command = TiffanyNativeCliCommand {
            role: "worker-gemini".into(),
            runtime: Some("gemini".into()),
            worker_thread_id: None,
            native_session: "gemini-native-session".into(),
            command: "gemini --resume gemini-native-session".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let json = r#"{
  "messages": [
    {"type":"gemini","content":"我会修改文件。","toolCalls":[{"name":"replace","displayName":"Edit","args":{"file_path":"/tmp/repo/src/lib.rs","old_string":"old","new_string":"new"}}]}
  ]
}"#;

        let events = native_cli_transcript_events_from_gemini_json(&command, json, 0);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind.as_deref(), Some("answer"));
        assert_eq!(events[1].kind.as_deref(), Some("file_update"));
        assert!(events[1].title.contains("native file update"));
        assert!(
            events[1]
                .content
                .as_deref()
                .unwrap()
                .contains("/tmp/repo/src/lib.rs")
        );
        assert!(
            !events[1]
                .content
                .as_deref()
                .unwrap()
                .contains("\"file_path\"")
        );
    }

    #[test]
    fn native_cli_transcript_events_parse_gemini_new_messages_only() {
        let command = TiffanyNativeCliCommand {
            role: "worker-gemini".into(),
            runtime: Some("gemini".into()),
            worker_thread_id: None,
            native_session: "latest".into(),
            command: "gemini --resume latest".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let json = r#"{
  "messages": [
    {"type":"user","content":"old"},
    {"type":"gemini","content":"old answer"},
    {"type":"user","content":"new"},
    {"type":"gemini","content":"new answer"}
  ]
}"#;

        let events = native_cli_transcript_events_from_gemini_json(&command, json, 2);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].content.as_deref(), Some("new"));
        assert_eq!(events[1].content.as_deref(), Some("new answer"));
    }

    #[test]
    fn native_cli_transcript_events_parse_gemini_cancelled_tool_as_approval() {
        let command = TiffanyNativeCliCommand {
            role: "worker-gemini".into(),
            runtime: Some("gemini".into()),
            worker_thread_id: None,
            native_session: "latest".into(),
            command: "gemini --resume latest".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let json = r#"{
  "messages": [
    {"type":"gemini","content":"准备编辑。","toolCalls":[{"name":"replace","displayName":"Edit","status":"cancelled","result":[{"functionResponse":{"response":{"error":"[Operation Cancelled] Reason: User did not allow tool call"}}}]}]}
  ]
}"#;

        let events = native_cli_transcript_events_from_gemini_json(&command, json, 0);

        assert_eq!(events.len(), 3);
        assert_eq!(events[2].kind.as_deref(), Some("approval"));
        assert!(events[2].title.contains("native approval"));
        assert!(
            events[2]
                .content
                .as_deref()
                .unwrap()
                .contains("Operation Cancelled")
        );
    }

    #[test]
    fn gemini_project_hash_matches_cli_storage_layout() {
        assert_eq!(
            gemini_project_hash("/tiffany-loop/nonexistent-gemini-project").as_deref(),
            Some("beefa779e279137845bc9e354c1678846c66d0641b01693571f0bb86bae3fdb6")
        );
    }

    #[test]
    fn gemini_chat_path_uses_current_worktree_project_hash() -> std::io::Result<()> {
        let home = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        let project_hash =
            gemini_project_hash(&project.path().display().to_string()).expect("project hash");
        let chat_dir = home
            .path()
            .join(".gemini")
            .join("tmp")
            .join(project_hash)
            .join("chats");
        std::fs::create_dir_all(&chat_dir)?;
        let chat_path = chat_dir.join("session-2026-06-26T00-00-abcdef12.json");
        std::fs::write(
            &chat_path,
            r#"{"sessionId":"abcdef12-0000-0000-0000-000000000000","messages":[]}"#,
        )?;

        let command = TiffanyNativeCliCommand {
            role: "worker-gemini".into(),
            runtime: Some("gemini".into()),
            worker_thread_id: None,
            native_session: "latest".into(),
            command: "gemini --resume latest".into(),
            worktree: Some(project.path().display().to_string()),
        };

        assert_eq!(
            gemini_session_json_path_in_home(home.path(), &command),
            Some(chat_path)
        );
        Ok(())
    }

    #[test]
    fn native_cli_return_lines_show_gemini_transcript_label() {
        let command = TiffanyNativeCliCommand {
            role: "worker-gemini".into(),
            runtime: Some("gemini".into()),
            worker_thread_id: None,
            native_session: "latest".into(),
            command: "gemini --resume latest".into(),
            worktree: Some("/tmp/repo".into()),
        };
        let outcome = TiffanyNativeCliReturn {
            transcript_event_count: 2,
            transcript_preview: Some("Gemini done".into()),
            ..TiffanyNativeCliReturn::default()
        };

        let text = native_cli_return_lines(&command, &outcome)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("native events  2 captured from Gemini transcript"));
        assert!(text.contains("native answer  Gemini transcript"));
    }

    #[test]
    fn continue_summary_lines_warn_when_native_session_missing() {
        let lines = continue_summary_lines(
            "Worker thread 00000000\n\
               role: worker-cc\n\
               runtime: claude-code\n\
               native session: none\n\
             \n\
             Status: no native session yet\n",
        )
        .expect("continue summary lines");
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("⚠ continue  worker-cc has no native session yet"));
        assert!(text.contains("native  none"));
        assert!(text.contains("next  run a task"));
        assert!(text.contains("next  /role worker-cc"));
        assert!(text.contains("next  /thread worker-cc"));
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
        assert_eq!(lines[0].spans[2].style.fg, Some(TIFFANY_DARK));
        assert_eq!(lines[1].spans[0].style.fg, Some(TIFFANY_DARK));
        let tool = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test".to_string()),
            ..event
        };
        let tool_lines = output_event_lines(&tool, "tool Bash: cargo test");
        assert_eq!(tool_lines[0].spans[0].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(tool_lines[0].spans[2].style.fg, Some(TIFFANY_BLUE));
        assert_eq!(tool_lines[1].spans[0].style.fg, Some(TIFFANY_DARK));
        assert_eq!(tool_lines[1].spans[1].style.fg, Some(TIFFANY_BLUE));
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
        assert!(text.contains("  user  你好"));
    }

    #[test]
    fn timeline_compacts_stale_route_selected_when_route_updates() {
        let selected = TiffanyProgressEvent {
            role: "orchestrator".to_string(),
            status: "running".to_string(),
            message: "route selected - full-pipeline".to_string(),
            route: Some("full-pipeline".to_string()),
            route_label: Some("full".to_string()),
            route_reason_label: Some("implementation".to_string()),
            flow_steps: Some("planner -> critic -> worker -> reviewer -> answer".to_string()),
            ..TiffanyProgressEvent::default()
        };
        let updated = TiffanyProgressEvent {
            role: "orchestrator".to_string(),
            status: "warning".to_string(),
            message: "route updated - single-worker".to_string(),
            route: Some("single-worker".to_string()),
            route_label: Some("single".to_string()),
            route_reason_label: Some("planner fallback".to_string()),
            flow_steps: Some("worker -> answer".to_string()),
            ..TiffanyProgressEvent::default()
        };
        let worker = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "ready".to_string(),
            message: "worker ready - 1 run(s)".to_string(),
            count: Some(1),
            route: Some("single-worker".to_string()),
            route_label: Some("single".to_string()),
            flow_steps: Some("worker -> answer".to_string()),
            ..TiffanyProgressEvent::default()
        };

        let lines = timeline_event_lines(&[selected, updated, worker]);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(
            line_text(&lines[0]),
            "⚠ flow  run timeline · worker -> answer · 2 updates"
        );
        assert!(text.contains("route  updated · single · planner fallback · worker -> answer"));
        assert!(text.contains("worker  ready · 1 run(s) · single · worker -> answer"));
        assert!(!text.contains("selected · full"));
        assert!(!text.contains("planner -> critic"));
    }

    #[test]
    fn timeline_hides_zero_done_after_worker_failure() {
        let worker_failed = TiffanyProgressEvent {
            role: "worker".to_string(),
            status: "failed".to_string(),
            message: "worker-cc failed".to_string(),
            task_id: Some("12345678-0000-0000-0000-000000000000".to_string()),
            worker_role: Some("worker-cc".to_string()),
            agent: Some("claude-code".to_string()),
            duration_ms: Some(253),
            ..TiffanyProgressEvent::default()
        };
        let zero_done = TiffanyProgressEvent {
            role: "orchestrator".to_string(),
            status: "done".to_string(),
            message: "done - 0 worker run(s)".to_string(),
            count: Some(0),
            ..TiffanyProgressEvent::default()
        };

        let lines = timeline_event_lines(&[worker_failed, zero_done]);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker-cc failed"));
        assert!(!text.contains("done - 0 worker run"));
        assert!(!text.contains("0 worker run"));
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code assistant: running tests",
            )
        };

        let visible = visible_content(&event).expect("answer visible");
        let lines = output_event_lines(&event, &visible);

        assert_eq!(
            line_text(&lines[0]),
            "◆ worker answer · worker-cc · claude-code · 12345678"
        );
    }

    #[test]
    fn worker_output_title_uses_native_kind_markers() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "running tests")
        };

        let tool = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test".to_string()),
            ..base.clone()
        };
        let approval = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some(
                "claude-code tool_use: tool request_permissions: write access".to_string(),
            ),
            ..base.clone()
        };
        let stderr = TiffanyProgressEvent {
            event_kind: Some("stderr".to_string()),
            content: Some("claude-code stderr: model not found".to_string()),
            ..base.clone()
        };
        let diff = TiffanyProgressEvent {
            event_kind: Some("diff".to_string()),
            content: Some("claude-code diff: diff --git a/lib.rs b/lib.rs".to_string()),
            ..base
        };

        assert_eq!(
            line_text(&output_event_lines(&tool, "tool Bash: cargo test")[0]),
            "↳ worker tool call · worker-cc · claude-code · 12345678"
        );
        assert_eq!(
            line_text(
                &output_event_lines(&approval, "waiting for permission approval: write access")[0]
            ),
            "⚠ worker approval · worker-cc · claude-code · 12345678"
        );
        assert_eq!(
            native_event_kind_from_progress(&approval).as_deref(),
            Some("approval")
        );
        assert_eq!(
            line_text(&output_event_lines(&stderr, "model not found")[0]),
            "✗ worker stderr · worker-cc · claude-code · 12345678"
        );
        assert_eq!(
            line_text(&output_event_lines(&diff, "diff --git a/lib.rs b/lib.rs")[0]),
            "± worker diff · worker-cc · claude-code · 12345678"
        );
    }

    #[test]
    fn native_session_busy_retry_renders_as_session_recovery() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("status".to_string()),
            content: Some(
                "claude-code status: native session busy · waiting for saved session native-busy and retry 1/4 after 500ms with the same native session"
                    .to_string(),
            ),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };

        let visible = visible_content(&event).expect("recovery visible");
        let lines = output_event_lines(&event, &visible);

        assert_eq!(
            line_text(&lines[0]),
            "⚠ worker session recovery · worker-cc · claude-code · 12345678"
        );
        assert!(line_text(&lines[1]).contains("native session busy"));
        assert_eq!(
            native_event_kind_from_progress(&event).as_deref(),
            Some("alert")
        );
    }

    #[test]
    fn worker_output_body_uses_native_kind_markers() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let tool = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test".to_string()),
            ..base.clone()
        };
        let result = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some("claude-code tool_result: tool result: ok\nall passed".to_string()),
            ..base.clone()
        };
        let question = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some("claude-code tool_result: tool error: Answer questions?".to_string()),
            ..base.clone()
        };
        let approval = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some(
                "claude-code tool_use: tool request_permissions: write access".to_string(),
            ),
            ..base.clone()
        };
        let stderr = TiffanyProgressEvent {
            event_kind: Some("stderr".to_string()),
            content: Some("claude-code stderr: permission denied".to_string()),
            ..base.clone()
        };

        let tool_visible = visible_content(&tool).expect("tool visible");
        let tool_lines = output_event_lines(&tool, &tool_visible);
        assert_eq!(line_text(&tool_lines[1]), "  ↳ tool Bash: cargo test");

        let result_visible = visible_content(&result).expect("result visible");
        let result_lines = output_event_lines(&result, &result_visible);
        assert_eq!(line_text(&result_lines[1]), "  ✓ ok");
        assert_eq!(line_text(&result_lines[2]), "  │ all passed");

        let question_visible = visible_content(&question).expect("question visible");
        let question_lines = output_event_lines(&question, &question_visible);
        assert_eq!(
            line_text(&question_lines[1]),
            "  ? waiting for user input: Answer questions?"
        );
        assert_eq!(
            line_text(&question_lines[2]),
            "  next  /continue open worker-cc  open worker handoff to answer the question"
        );
        assert_eq!(
            line_text(&question_lines[3]),
            "  next  /thread worker-cc  inspect session, native id, and handoff state"
        );
        assert_eq!(
            line_text(&question_lines[4]),
            "  next  /process questions  show matching process events in this TUI"
        );
        assert_eq!(
            line_text(&question_lines[5]),
            "  next  /history kind question  show saved worker questions across native worker events"
        );
        let detailed_question_payload = serde_json::json!({
            "type": "tool_use",
            "name": "AskUserQuestion",
            "input": {
                "questions": [
                    { "question": "想了解比赛规则，还是直接写参赛 agent？" }
                ]
            }
        });
        let detailed_question = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some(format!("claude-code tool_use: {detailed_question_payload}")),
            ..base.clone()
        };
        let detailed_question_visible =
            visible_content(&detailed_question).expect("detailed question visible");
        let detailed_question_lines =
            output_event_lines(&detailed_question, &detailed_question_visible);
        assert_eq!(
            line_text(&detailed_question_lines[1]),
            "  ? question requested: 想了解比赛规则，还是直接写参赛 agent？"
        );
        assert_eq!(
            line_text(&detailed_question_lines[2]),
            "  next  /continue open worker-cc  open worker handoff to answer the question"
        );
        let native_question = TiffanyProgressEvent {
            native_session_id: Some("native-question-session".to_string()),
            ..question
        };
        let native_question_visible =
            visible_content(&native_question).expect("native question visible");
        let native_question_lines = output_event_lines(&native_question, &native_question_visible);
        assert_eq!(
            line_text(&native_question_lines[2]),
            "  next  /continue open worker-cc  reopen same native session to answer the worker question"
        );

        let approval_visible = visible_content(&approval).expect("approval visible");
        let approval_lines = output_event_lines(&approval, &approval_visible);
        assert_eq!(
            line_text(&approval_lines[1]),
            "  ⚠ waiting for permission approval: write access"
        );
        let detailed_approval_payload = serde_json::json!({
            "type": "tool_use",
            "name": "request_permissions",
            "input": {
                "permissions": ["network access", "write access"],
                "reason": "fetch Kaggle competition page"
            }
        });
        let detailed_approval = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some(format!("codex tool_use: {detailed_approval_payload}")),
            ..base.clone()
        };
        let detailed_approval_visible =
            visible_content(&detailed_approval).expect("detailed approval visible");
        let detailed_approval_lines =
            output_event_lines(&detailed_approval, &detailed_approval_visible);
        assert_eq!(
            line_text(&detailed_approval_lines[1]),
            "  ⚠ waiting for permission approval: network access / write access · fetch Kaggle competition page"
        );
        assert_eq!(
            line_text(&approval_lines[2]),
            "  next  /continue open worker-cc  open worker handoff to approve or deny"
        );
        assert_eq!(
            line_text(&approval_lines[4]),
            "  next  /process approvals  show matching process events in this TUI"
        );
        assert_eq!(
            line_text(&approval_lines[5]),
            "  next  /history kind approval  show saved approval requests across native worker events"
        );

        let native_approval = TiffanyProgressEvent {
            native_session_id: Some("5cee8032-e580-49f9-b0b8-95b0940c5692".to_string()),
            ..approval
        };
        let native_approval_visible =
            visible_content(&native_approval).expect("native approval visible");
        let native_approval_lines = output_event_lines(&native_approval, &native_approval_visible);
        assert_eq!(
            line_text(&native_approval_lines[2]),
            "  next  /continue open worker-cc  reopen same native session to approve or deny"
        );
        assert_eq!(
            line_text(&native_approval_lines[6]),
            "  next  native 5cee8032  same underlying CLI conversation will be resumed"
        );

        let stderr_visible = visible_content(&stderr).expect("stderr visible");
        let stderr_lines = output_event_lines(&stderr, &stderr_visible);
        assert_eq!(line_text(&stderr_lines[1]), "  ✗ permission denied");
        assert!(
            stderr_lines
                .iter()
                .map(line_text)
                .any(|line| line.contains("fix permission blocked execution"))
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
            recovery: None,
            event_kind: None,
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
            "worker answer · worker-cc · claude-code · minimax/MiniMax-M3 · 12345678"
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
    fn worker_trust_error_output_adds_actionable_fix_line() {
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-codex".to_string()),
            model: Some("gpt-5-codex".to_string()),
            provider: Some("openai".to_string()),
            ..worker_output_event(
                "worker output",
                "worker-codex",
                "codex stderr: Not inside a trusted directory and --skip-git-repo-check was not specified.",
            )
        };

        let visible = visible_content(&event).expect("visible trust error");
        let lines = output_event_lines(&event, &visible);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("worker stderr · worker-codex · openai/gpt-5-codex · 12345678"));
        assert!(text.contains("fix project trust or git check blocked execution"));
        assert!(text.contains("verify the runtime supports --skip-git-repo-check with /doctor"));
    }

    #[test]
    fn worker_native_session_recovery_output_is_labeled_and_not_captured_as_final() {
        for (kind, native, detail) in [
            (
                "native session busy",
                "native-busy",
                "waiting for saved session native-busy and retrying once with the same native session",
            ),
            (
                "native session missing",
                "native-missing",
                "cleared saved session native-missing and retrying once with a fresh native session",
            ),
        ] {
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
                    &format!("claude-code status: {kind} · {detail}"),
                )
            };

            let visible = visible_content(&event).expect("visible recovery output");
            let lines = output_event_lines(&event, &visible);
            let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

            assert!(text.contains(
                "worker session recovery · worker-cc · claude-code · anthropic/claude-sonnet-4-6 · 12345678"
            ));
            assert!(text.contains(kind));
            assert!(text.contains(native));
            assert!(text.contains("retrying once"));
            assert_eq!(final_output_from_event(&event, &visible), None);
        }
    }

    #[test]
    fn structured_worker_recovery_event_keeps_recovery_kind_visible() {
        let event: TiffanyProgressEvent = serde_json::from_value(serde_json::json!({
            "role": "worker",
            "status": "output",
            "message": "worker-cc recovery",
            "task_id": "12345678-0000-0000-0000-000000000000",
            "agent": "claude-code",
            "worker_role": "worker-cc",
            "model": "claude-sonnet-4-6",
            "provider": "anthropic",
            "worker_thread_id": "abcdef12-0000-0000-0000-000000000000",
            "native_session_id": "native-busy",
            "event_kind": "status",
            "recovery": "busy",
            "content": "native session busy · waiting for saved session native-busy and retry 1/6 after 500ms with the same native session"
        }))
        .expect("structured worker recovery event");

        let visible = visible_content(&event).expect("visible recovery output");
        let lines = output_event_lines(&event, &visible);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(
            line_text(&lines[0]),
            "⚠ worker session recovery · busy · worker-cc · claude-code · anthropic/claude-sonnet-4-6 · 12345678"
        );
        assert!(text.contains("retry 1/6"));
        assert_eq!(
            native_event_kind_from_progress(&event).as_deref(),
            Some("session_recovery")
        );
        assert!(output_scope(&event).contains(":session_recovery:busy"));
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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

        assert!(text.contains("user  你叫啥"));
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

        assert_eq!(
            visible_task_detail(task_prompt),
            VisibleTaskDetail {
                label: "user",
                text: "你好".to_string(),
            }
        );
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
    fn worker_waterfall_keeps_common_tool_results_and_answers_complete() {
        let long_tool_body = (1..=14)
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tool = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("tool_result".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                &format!("claude-code tool_result: tool result: {long_tool_body}"),
            )
        };

        let tool_visible = visible_content(&tool).expect("tool visible");
        let tool_text = output_event_lines(&tool, &tool_visible)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(tool_text.contains("line 1"));
        assert!(tool_text.contains("line 14"));
        assert!(tool_text.contains("line 7"));
        assert!(!tool_text.contains("line(s) hidden"));

        let long_answer = (1..=14)
            .map(|idx| format!("answer line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let answer = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("assistant".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                &format!("claude-code assistant: {long_answer}"),
            )
        };

        let answer_visible = visible_content(&answer).expect("answer visible");
        let answer_text = output_event_lines(&answer, &answer_visible)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(answer_text.contains("answer line 1"));
        assert!(answer_text.contains("answer line 14"));
        assert!(!answer_text.contains("line(s) hidden"));
    }

    #[test]
    fn worker_waterfall_groups_tool_call_and_result_into_one_block() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let call = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test --all".to_string()),
            ..base.clone()
        };
        let result = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some(
                "claude-code tool_result: tool result: tests passed\n2 passed".to_string(),
            ),
            ..base
        };

        let call_visible = visible_content(&call).expect("tool call visible");
        let result_visible = visible_content(&result).expect("tool result visible");
        let lines = worker_tool_pair_event_lines(&call, &call_visible, &result, &result_visible);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(
            line_text(&lines[0]),
            "↳ worker Bash · worker-cc · claude-code · 12345678"
        );
        assert!(text.contains("  ↳ tool Bash: cargo test --all"));
        assert!(text.contains("  ✓ tests passed"));
        assert!(text.contains("  │ 2 passed"));
        assert!(!text.contains("worker tool call"));
        assert!(!text.contains("worker tool result"));
    }

    #[test]
    fn worker_waterfall_collapses_duplicate_tool_pair_body() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let call = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool status: tests passed".to_string()),
            ..base.clone()
        };
        let result = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some("claude-code tool_result: tool result: tests passed".to_string()),
            ..base
        };

        let lines = worker_tool_pair_event_lines(&call, "tests passed", &result, "tests passed");
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        let body_hits = rendered
            .iter()
            .filter(|line| line.contains("tests passed"))
            .count();

        assert_eq!(body_hits, 1);
        assert!(rendered.iter().any(|line| line == "  ✓ tests passed"));
        assert!(!rendered.iter().any(|line| line == "  ↳ tests passed"));
    }

    #[test]
    fn bridge_state_dedupes_visible_duplicate_answer_events() {
        let assistant = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("assistant".to_string()),
            ..worker_output_event(
                "claude output",
                "claude-code",
                "claude-code assistant: useful summary",
            )
        };
        let result = TiffanyProgressEvent {
            event_kind: Some("result".to_string()),
            content: Some("claude-code result: useful summary".to_string()),
            ..assistant.clone()
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let mut state = BridgeState::default();
        state.emit_event(&app_event_tx, assistant);
        state.emit_event(&app_event_tx, result);

        let mut deltas = Vec::new();
        let mut inserted_history = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::TiffanyOrchestratorAnswerDelta { delta } => deltas.push(delta),
                AppEvent::InsertHistoryCell(_) => inserted_history = true,
                _ => {}
            }
        }

        assert_eq!(deltas, vec!["useful summary"]);
        assert!(!inserted_history);
    }

    #[test]
    fn bridge_state_does_not_dedupe_long_outputs_that_differ_after_summary_prefix() {
        let shared_prefix = "x".repeat(CONTROL_SUMMARY_MAX_CHARS + 32);
        let first = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("assistant".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                &format!("claude-code assistant: {shared_prefix}\nfirst unique ending"),
            )
        };
        let second = TiffanyProgressEvent {
            content: Some(format!(
                "claude-code assistant: {shared_prefix}\nsecond unique ending"
            )),
            ..first.clone()
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let mut state = BridgeState::default();
        state.emit_event(&app_event_tx, first);
        state.emit_event(&app_event_tx, second);

        let mut deltas = Vec::new();
        let mut inserted_history = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::TiffanyOrchestratorAnswerDelta { delta } => deltas.push(delta),
                AppEvent::InsertHistoryCell(_) => inserted_history = true,
                _ => {}
            }
        }
        let text = deltas.join("\n");

        assert_eq!(deltas.len(), 2);
        assert!(text.contains("first unique ending"));
        assert!(text.contains("second unique ending"));
        assert!(!inserted_history);
    }

    #[test]
    fn bridge_state_groups_adjacent_tool_call_and_result_into_one_cell() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let call = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test --all".to_string()),
            ..base.clone()
        };
        let result = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some(
                "claude-code tool_result: tool result: tests passed\n2 passed".to_string(),
            ),
            ..base
        };

        let cells = bridge_history_cells([call, result]);
        let text = cells
            .iter()
            .map(|cell| cell.join("\n"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(cells.len(), 1);
        assert!(text.contains("↳ worker Bash · worker-cc · claude-code · 12345678"));
        assert!(text.contains("  ↳ tool Bash: cargo test --all"));
        assert!(text.contains("  ✓ tests passed"));
        assert!(text.contains("  │ 2 passed"));
        assert!(!text.contains("worker tool call"));
        assert!(!text.contains("worker tool result"));
    }

    #[test]
    fn bridge_state_keeps_same_tool_result_text_for_different_tool_calls() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let call_one = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo fmt --check".to_string()),
            ..base.clone()
        };
        let result_one = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some("claude-code tool_result: tool result: ok".to_string()),
            ..base.clone()
        };
        let call_two = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test --all".to_string()),
            ..base.clone()
        };
        let result_two = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some("claude-code tool_result: tool result: ok".to_string()),
            ..base
        };

        let cells = bridge_history_cells([call_one, result_one, call_two, result_two]);
        let text = cells
            .iter()
            .map(|cell| cell.join("\n"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(cells.len(), 2);
        assert!(text.contains("cargo fmt --check"));
        assert!(text.contains("cargo test --all"));
        assert_eq!(text.matches("  ✓ ok").count(), 2);
    }

    #[test]
    fn worker_tool_pair_title_extracts_safe_tool_name() {
        assert_eq!(
            worker_tool_pair_title("worker-cc · 12345678", "tool WebFetch: https://example.com"),
            "worker WebFetch · worker-cc · 12345678"
        );
        assert_eq!(
            worker_tool_pair_title("worker-cc · 12345678", "tool: Bash cargo test"),
            "worker Bash · worker-cc · 12345678"
        );
        assert_eq!(
            worker_tool_pair_title("worker-cc · 12345678", "tool weird/name: run"),
            "worker tool · worker-cc · 12345678"
        );
    }

    #[test]
    fn bridge_state_flushes_unmatched_tool_call_before_next_output() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let call = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test --all".to_string()),
            ..base.clone()
        };
        let answer = TiffanyProgressEvent {
            event_kind: Some("assistant".to_string()),
            content: Some("claude-code assistant: tests are running".to_string()),
            ..base
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let mut state = BridgeState::default();
        state.emit_event(&app_event_tx, call);
        state.emit_event(&app_event_tx, answer);

        let mut history_cells = Vec::new();
        let mut deltas = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::InsertHistoryCell(cell) => {
                    history_cells.push(
                        cell.transcript_lines(u16::MAX)
                            .iter()
                            .map(line_text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                AppEvent::TiffanyOrchestratorAnswerDelta { delta } => deltas.push(delta),
                _ => {}
            }
        }
        let text = history_cells.join("\n--- cell ---\n");

        assert!(text.contains("worker tool call · worker-cc · claude-code · 12345678"));
        assert!(text.contains("  ↳ tool Bash: cargo test --all"));
        assert_eq!(history_cells.len(), 1);
        assert_eq!(deltas, vec!["tests are running"]);
    }

    #[test]
    fn worker_waterfall_keeps_large_tool_results_complete() {
        let long_tool_body = (1..=60)
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tool = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("tool_result".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                &format!("claude-code tool_result: tool result: {long_tool_body}"),
            )
        };

        let tool_visible = visible_content(&tool).expect("tool visible");
        let tool_text = output_event_lines(&tool, &tool_visible)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(tool_text.contains("line 1"));
        assert!(tool_text.contains("line 60"));
        assert!(tool_text.contains("line 30"));
        assert!(!tool_text.contains("line(s) hidden"));
    }

    #[test]
    fn worker_waterfall_compacts_large_tool_calls_but_not_worker_answers() {
        let long_tool_args = (1..=60)
            .map(|idx| format!("arg line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tool = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("tool_use".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                &format!("claude-code tool_use: tool Bash: {long_tool_args}"),
            )
        };

        let tool_visible = visible_content(&tool).expect("tool visible");
        let tool_text = output_event_lines(&tool, &tool_visible)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(tool_text.contains("arg line 1"));
        assert!(tool_text.contains("arg line 60"));
        assert!(tool_text.contains("line(s) hidden"));
        assert!(tool_text.contains("/process tool-call"));
        assert!(tool_text.contains("/history kind tool_use"));
        assert!(!tool_text.contains("arg line 30"));

        let long_diff = (1..=60)
            .map(|idx| format!("+diff line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let diff = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("diff".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                &format!("claude-code diff: diff --git a/lib.rs b/lib.rs\n{long_diff}"),
            )
        };
        let diff_visible = visible_content(&diff).expect("diff visible");
        let diff_text = output_event_lines(&diff, &diff_visible)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(diff_text.contains("diff line 1"));
        assert!(diff_text.contains("diff line 60"));
        assert!(!diff_text.contains("line(s) hidden"));
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
            "worker answer · claude-code · 12345678"
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

        let internal_tool_selection = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("tool_use".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code tool_use: tool ToolSearch: select:WebFetch",
            )
        };

        assert_eq!(visible_content(&internal_tool_selection), None);
    }

    #[test]
    fn worker_waterfall_normalizes_claude_tool_status_wrappers() {
        let system_note = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("system".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code system: Create venv and install deps",
            )
        };
        let visible = visible_content(&system_note).expect("system note visible");
        assert_eq!(visible, "Create venv and install deps");

        let created = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("tool_result".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code user: tool result: File created successfully at: /tmp/agent.py (file state is current in your context -- no need to Read it back)",
            )
        };
        let created_visible = visible_content(&created).expect("file update visible");
        assert_eq!(created_visible, "file created: /tmp/agent.py");
        assert!(output_title(&created).starts_with("worker file update"));

        let no_output = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            event_kind: Some("tool_result".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code user: tool result: (Bash completed with no output)",
            )
        };
        assert_eq!(visible_content(&no_output), None);
    }

    #[test]
    fn worker_waterfall_humanizes_native_codex_rollout_events() {
        let cases = [
            (
                "worker-codex",
                "codex",
                "tool_use",
                serde_json::json!({
                    "timestamp": "2026-06-26T00:00:01Z",
                    "item": {
                        "event_msg": {
                            "exec_command_begin": { "command": ["cargo", "test", "--all"] }
                        }
                    }
                })
                .to_string(),
                "worker tool call",
                "tool shell: cargo test --all",
            ),
            (
                "worker-codex",
                "codex",
                "tool_result",
                serde_json::json!({
                    "timestamp": "2026-06-26T00:00:02Z",
                    "item": {
                        "event_msg": {
                            "exec_command_end": { "exit_code": 0, "stdout": "ok" }
                        }
                    }
                })
                .to_string(),
                "worker tool result",
                "tool shell result: exit 0\nok",
            ),
            (
                "worker-codex",
                "codex",
                "diff",
                serde_json::json!({
                    "timestamp": "2026-06-26T00:00:03Z",
                    "item": {
                        "event_msg": {
                            "turn_diff": { "diff": "diff --git a/lib.rs b/lib.rs\n+done" }
                        }
                    }
                })
                .to_string(),
                "worker diff",
                "diff --git a/lib.rs b/lib.rs\n+done",
            ),
        ];

        for (role, agent, kind, raw, title_prefix, expected_visible) in cases {
            let summary = event_format::summarize_cli_stream_line(&raw, 8_000).expect("summary");
            let event = TiffanyProgressEvent {
                worker_role: Some(role.to_string()),
                event_kind: Some(kind.to_string()),
                ..worker_output_event(
                    "worker output",
                    agent,
                    &format!("{agent} {kind}: {}", summary.text),
                )
            };

            let visible = visible_content(&event).expect("visible content");
            assert_eq!(visible, expected_visible);
            assert!(output_title(&event).starts_with(title_prefix));
            assert!(!visible.contains('{'));
            assert!(!visible.contains("event_msg"));
        }
    }

    #[test]
    fn worker_waterfall_humanizes_native_approval_and_gemini_events() {
        let guardian = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "guardian_assessment",
                "status": "in_progress",
                "action": {
                    "type": "command",
                    "command": "rm -rf /tmp/guardian"
                }
            }
        })
        .to_string();
        let summary =
            event_format::summarize_cli_stream_line(&guardian, 8_000).expect("guardian summary");
        let approval = TiffanyProgressEvent {
            worker_role: Some("worker-codex".to_string()),
            event_kind: Some("approval".to_string()),
            ..worker_output_event(
                "worker output",
                "codex",
                &format!("codex approval: {}", summary.text),
            )
        };

        assert_eq!(
            visible_content(&approval).as_deref(),
            Some("waiting for permission approval: in_progress · command: rm -rf /tmp/guardian")
        );
        assert!(output_title(&approval).starts_with("worker approval"));

        let gemini = serde_json::json!({
            "toolCalls": [
                {
                    "name": "replace",
                    "displayName": "Edit",
                    "status": "cancelled",
                    "result": [
                        {
                            "functionResponse": {
                                "response": {
                                    "error": "[Operation Cancelled] Reason: User did not allow tool call"
                                }
                            }
                        }
                    ]
                }
            ]
        })
        .to_string();
        let summary =
            event_format::summarize_cli_stream_line(&gemini, 8_000).expect("gemini summary");
        let event = TiffanyProgressEvent {
            worker_role: Some("worker-gemini".to_string()),
            event_kind: Some("tool_result".to_string()),
            ..worker_output_event(
                "worker output",
                "gemini",
                &format!("gemini tool_result: {}", summary.text),
            )
        };

        assert_eq!(
            visible_content(&event).as_deref(),
            Some("tool Edit error: [Operation Cancelled] Reason: User did not allow tool call")
        );
        assert!(output_title(&event).starts_with("worker tool result"));
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
    fn bridge_marks_streamed_final_output_as_already_visible() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let mut state = BridgeState::default();
        let assistant = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event(
                "worker output",
                "claude-code",
                "claude-code assistant: 你好！",
            )
        };

        state.emit_event(&app_event_tx, assistant);

        assert_eq!(state.final_output.as_deref(), Some("你好！"));
        assert!(state.final_output_visible);
        let mut deltas = Vec::new();
        let mut inserted_history = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::TiffanyOrchestratorAnswerDelta { delta } => deltas.push(delta),
                AppEvent::InsertHistoryCell(_) => inserted_history = true,
                _ => {}
            }
        }
        assert_eq!(deltas, vec!["你好！"]);
        assert!(!inserted_history);
    }

    #[test]
    fn bridge_streams_incremental_worker_answer_suffixes() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let mut state = BridgeState::default();

        state.emit_event(
            &app_event_tx,
            TiffanyProgressEvent {
                worker_role: Some("worker-cc".to_string()),
                ..worker_output_event(
                    "worker output",
                    "claude-code",
                    "claude-code assistant: 你好",
                )
            },
        );
        state.emit_event(
            &app_event_tx,
            TiffanyProgressEvent {
                worker_role: Some("worker-cc".to_string()),
                ..worker_output_event(
                    "worker output",
                    "claude-code",
                    "claude-code assistant: 你好，世界",
                )
            },
        );
        state.finish_answer_stream(&app_event_tx);

        let mut deltas = Vec::new();
        let mut finished = false;
        let mut inserted_history = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::TiffanyOrchestratorAnswerDelta { delta } => deltas.push(delta),
                AppEvent::TiffanyOrchestratorAnswerFinished => finished = true,
                AppEvent::InsertHistoryCell(_) => inserted_history = true,
                _ => {}
            }
        }

        assert_eq!(state.streamed_answer_text.as_deref(), Some("你好，世界"));
        assert_eq!(deltas, vec!["你好", "，世界"]);
        assert!(finished);
        assert!(!inserted_history);
    }

    #[test]
    fn bridge_keeps_tool_output_in_waterfall_not_answer_stream() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let mut state = BridgeState::default();

        state.emit_event(
            &app_event_tx,
            TiffanyProgressEvent {
                worker_role: Some("worker-cc".to_string()),
                event_kind: Some("tool_call".to_string()),
                ..worker_output_event("worker output", "claude-code", "tool Bash: cargo test")
            },
        );
        state.flush_pending_tool_call(&app_event_tx);

        let mut saw_tool_history = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::TiffanyOrchestratorAnswerDelta { delta } => {
                    panic!("tool output was incorrectly streamed as answer delta: {delta}")
                }
                AppEvent::InsertHistoryCell(cell) => {
                    let text = cell
                        .transcript_lines(u16::MAX)
                        .iter()
                        .map(line_text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    saw_tool_history |=
                        text.contains("worker tool call") || text.contains("tool Bash: cargo test");
                }
                _ => {}
            }
        }

        assert!(saw_tool_history);
        assert!(state.streamed_answer_text.is_none());
    }

    #[test]
    fn output_dedupe_scope_merges_assistant_and_result_after_visible_cleanup() {
        let assistant = TiffanyProgressEvent {
            event_kind: Some("assistant".to_string()),
            ..worker_output_event(
                "claude output",
                "claude-code",
                "claude-code assistant: useful summary",
            )
        };
        let result = TiffanyProgressEvent {
            event_kind: Some("result".to_string()),
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
    fn output_dedupe_scope_merges_duplicate_answer_events_only() {
        let assistant = TiffanyProgressEvent {
            event_kind: Some("assistant".to_string()),
            ..worker_output_event(
                "claude output",
                "claude-code",
                "claude-code assistant: useful summary",
            )
        };
        let result = TiffanyProgressEvent {
            event_kind: Some("result".to_string()),
            content: Some("claude-code result: useful summary".to_string()),
            ..assistant.clone()
        };
        let tool = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool Bash: cargo test".to_string()),
            ..assistant.clone()
        };

        assert_eq!(output_scope(&assistant), output_scope(&result));
        assert_ne!(output_scope(&assistant), output_scope(&tool));
    }

    #[test]
    fn output_dedupe_scope_merges_duplicate_tool_result_events() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let direct = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some("claude-code tool_result: tool result: tests passed".to_string()),
            ..base.clone()
        };
        let user = TiffanyProgressEvent {
            event_kind: Some("user".to_string()),
            content: Some("claude-code user: tool result: tests passed".to_string()),
            ..base.clone()
        };
        let output = TiffanyProgressEvent {
            event_kind: Some("output".to_string()),
            content: Some("claude-code output: tool result: tests passed".to_string()),
            ..base
        };

        let direct_visible = visible_content(&direct).expect("direct visible");
        let user_visible = visible_content(&user).expect("user visible");
        let output_visible = visible_content(&output).expect("output visible");

        assert_eq!(direct_visible, user_visible);
        assert_eq!(direct_visible, output_visible);
        assert_eq!(output_scope(&direct), output_scope(&user));
        assert_eq!(output_scope(&direct), output_scope(&output));
        assert_eq!(
            normalized_output_key(&direct_visible),
            normalized_output_key(&user_visible)
        );
        assert_eq!(normalized_output_key(&direct_visible), "tests passed");
    }

    #[test]
    fn output_dedupe_scope_merges_duplicate_question_events() {
        let base = TiffanyProgressEvent {
            worker_role: Some("worker-cc".to_string()),
            ..worker_output_event("worker output", "claude-code", "placeholder")
        };
        let tool_call = TiffanyProgressEvent {
            event_kind: Some("tool_use".to_string()),
            content: Some("claude-code tool_use: tool AskUserQuestion".to_string()),
            ..base.clone()
        };
        let tool_error = TiffanyProgressEvent {
            event_kind: Some("tool_result".to_string()),
            content: Some("claude-code tool_result: tool error: Answer questions?".to_string()),
            ..base
        };

        let call_visible = visible_content(&tool_call).expect("question call visible");
        let error_visible = visible_content(&tool_error).expect("question error visible");

        assert_eq!(call_visible, "waiting for user input: Answer questions?");
        assert_eq!(call_visible, error_visible);
        assert_eq!(output_scope(&tool_call), output_scope(&tool_error));
        assert_eq!(
            normalized_output_key(&call_visible),
            normalized_output_key(&error_visible)
        );
    }

    #[test]
    fn hides_redundant_role_output_summaries() {
        assert!(is_redundant_role_output(
            "planner",
            "plan ready - 1 worker run(s)"
        ));
        assert!(is_redundant_role_output(
            "planner",
            "plan ready - 1 worker run(s)\n  1. answer the user"
        ));
        assert!(!is_redundant_role_output(
            "planner",
            "plan ready - 2 worker run(s)\n  1. inspect\n  2. implement"
        ));
        assert!(is_redundant_role_output(
            "planner",
            "plan ready - 0 worker run(s)"
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
            recovery: None,
            event_kind: None,
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
        assert!(is_redundant_role_output(&planner.role, &planner_visible));
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
        assert!(visible.contains("... 1 more"));
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
            recovery: None,
            event_kind: None,
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
            recovery: None,
            event_kind: None,
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
        assert!(text.contains("fix model not found"));
        assert!(text.contains("Check the role's provider/model in /role"));
        assert!(text.contains("provider API model name"));
        assert!(text.contains("--model-name <api-model>"));
        assert!(text.contains("/doctor"));
    }

    #[cfg(unix)]
    #[test]
    fn failure_lines_explain_provider_auth_errors() {
        use std::os::unix::process::ExitStatusExt;

        let lines = orchestrator_failure_lines(
            ExitStatus::from_raw(1 << 8),
            "claude-code stderr: authentication failed: invalid api key",
            "",
            &[
                "worker: worker-cc · claude-code · anthropic/claude-sonnet-4-6 · abcdef12"
                    .to_string(),
            ],
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("authentication failed"));
        assert!(text.contains("fix provider authentication failed"));
        assert!(text.contains("Set the provider key in /provider"));
        assert!(text.contains("worker-cc · claude-code · anthropic/claude-sonnet-4-6"));
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
            roles_command_args("options").expect("roles args"),
            strings(&["roles", "options"])
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
        assert_eq!(
            roles_command_args("delete worker-codex").expect("roles args"),
            strings(&["roles", "delete", "worker-codex"])
        );
        assert_eq!(
            roles_command_args("rm worker-gemini").expect("roles args"),
            strings(&["roles", "delete", "worker-gemini"])
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
    fn provider_args_default_to_provider_list() {
        assert_eq!(
            provider_command_args("").expect("provider args"),
            vec![strings(&["config", "provider", "list"])]
        );
        assert_eq!(
            provider_command_args("list").expect("provider args"),
            vec![strings(&["config", "provider", "list"])]
        );
        assert_eq!(
            provider_command_args("status").expect("provider args"),
            vec![strings(&["config", "provider", "list"])]
        );
    }

    #[test]
    fn provider_args_support_provider_detail() {
        assert_eq!(
            provider_command_args("openai").expect("provider args"),
            vec![strings(&["config", "provider", "show", "openai"])]
        );
        assert_eq!(
            provider_command_args("show openai").expect("provider args"),
            vec![strings(&["config", "provider", "show", "openai"])]
        );
        assert!(provider_command_args("show").is_err());
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
    fn native_history_import_args_target_import_native_subcommand() {
        assert_eq!(
            native_history_import_args(Path::new("/tmp/native-sessions.json")),
            strings(&[
                "sessions",
                "import-native",
                "--path",
                "/tmp/native-sessions.json"
            ])
        );
    }

    #[test]
    fn native_history_command_parse_accepts_sync_only_without_args() {
        assert!(matches!(
            NativeHistoryCommand::parse("sync").expect("history sync"),
            NativeHistoryCommand::Sync
        ));
        assert!(NativeHistoryCommand::parse("sync now").is_err());
        assert!(matches!(
            NativeHistoryCommand::parse("status").expect("history status"),
            NativeHistoryCommand::Status
        ));
        assert!(NativeHistoryCommand::parse("status now").is_err());
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
    fn scoped_orchestrator_command_sets_worker_scope_env() {
        let config = test_config("orchestrator", None);
        let command = scoped_orchestrator_command(&config);
        let envs = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            envs.get("TIFFANY_WORKER_SCOPE").map(String::as_str),
            Some("tui:test:/tmp/tiffany")
        );
    }

    #[test]
    fn tiffany_worker_scope_is_stable_for_tui_session_and_project_cwd() {
        let cwd = tempfile::tempdir().expect("tempdir");
        let first = new_tiffany_worker_scope(cwd.path(), "session-a");
        let second = new_tiffany_worker_scope(cwd.path(), "session-a");
        let other_session = new_tiffany_worker_scope(cwd.path(), "session-b");
        let fallback = new_tiffany_worker_scope(cwd.path(), "   ");

        assert_eq!(first, second);
        assert_ne!(first, other_session);
        assert_eq!(
            first,
            format!("tui:{}:session:session-a", cwd_key(cwd.path()))
        );
        assert_eq!(fallback, format!("tui:{}", cwd_key(cwd.path())));
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
