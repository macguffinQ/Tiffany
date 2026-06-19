//! Agent Client Protocol (ACP) stdio server.
//!
//! ACP uses newline-delimited JSON-RPC 2.0 over stdio. In this module stdout is
//! reserved for protocol messages only; diagnostics must go through tracing.

use crate::agent_events;
use crate::config::RoleConfig;
use crate::core::session_store::SessionStore;
use crate::core::types::Task;
use crate::pipeline::orchestrator::{Orchestrator, RunProgress};
use crate::runtime::{self, AGENT_RUNTIME_ROUTES};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

const JSONRPC_VERSION: &str = "2.0";
const ACP_PROTOCOL_VERSION: u32 = 1;

const ERR_PARSE: i64 = -32700;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;
const ERR_INTERNAL: i64 = -32603;
const ERR_SERVER: i64 = -32000;

#[derive(Clone)]
struct RpcWriter {
    sink: RpcSink,
}

#[derive(Clone)]
enum RpcSink {
    Stdout(Arc<Mutex<tokio::io::Stdout>>),
    #[cfg(test)]
    Memory(Arc<Mutex<Vec<Value>>>),
}

impl RpcWriter {
    fn new() -> Self {
        Self {
            sink: RpcSink::Stdout(Arc::new(Mutex::new(tokio::io::stdout()))),
        }
    }

    #[cfg(test)]
    fn memory() -> (Self, Arc<Mutex<Vec<Value>>>) {
        let messages = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                sink: RpcSink::Memory(messages.clone()),
            },
            messages,
        )
    }

    async fn send(&self, message: Value) -> Result<()> {
        match &self.sink {
            RpcSink::Stdout(stdout) => {
                let mut stdout = stdout.lock().await;
                let encoded = serde_json::to_vec(&message)?;
                stdout.write_all(&encoded).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
            #[cfg(test)]
            RpcSink::Memory(messages) => {
                messages.lock().await.push(message);
            }
        }
        Ok(())
    }

    async fn response(&self, id: Value, result: Value) -> Result<()> {
        self.send(json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "result": result,
        }))
        .await
    }

    async fn error(&self, id: Value, code: i64, message: impl Into<String>) -> Result<()> {
        self.send(json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "error": {
                "code": code,
                "message": message.into(),
            },
        }))
        .await
    }

    async fn notification(&self, method: &str, params: Value) -> Result<()> {
        self.send(json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": method,
            "params": params,
        }))
        .await
    }
}

#[derive(Default)]
struct AcpState {
    sessions: HashMap<String, AcpSession>,
}

struct AcpSession {
    cwd: PathBuf,
    agent_hint: Option<String>,
    pending_prompt: Option<PendingPrompt>,
    transcript: Vec<AcpChatMsg>,
    last_result_output: Option<String>,
    context_mode: AcpContextMode,
    context_cutoff: usize,
    context_summary_text: String,
    context_summary_upto: usize,
    last_context_messages: usize,
    last_context_chars: usize,
    title: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

struct PendingPrompt {
    request_id: Value,
    cancel_tx: oneshot::Sender<()>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AcpSessionSnapshot {
    version: u32,
    session_id: String,
    cwd: PathBuf,
    agent_hint: Option<String>,
    transcript: Vec<AcpChatMsg>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_result_output: Option<String>,
    context_mode: AcpContextMode,
    context_cutoff: usize,
    context_summary_text: String,
    context_summary_upto: usize,
    last_context_messages: usize,
    last_context_chars: usize,
    title: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl AcpSessionSnapshot {
    fn from_session(session_id: &str, session: &AcpSession) -> Self {
        Self {
            version: 1,
            session_id: session_id.to_string(),
            cwd: session.cwd.clone(),
            agent_hint: session.agent_hint.clone(),
            transcript: session.transcript.clone(),
            last_result_output: session.last_result_output.clone(),
            context_mode: session.context_mode,
            context_cutoff: session.context_cutoff,
            context_summary_text: session.context_summary_text.clone(),
            context_summary_upto: session.context_summary_upto,
            last_context_messages: session.last_context_messages,
            last_context_chars: session.last_context_chars,
            title: session.title.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
        }
    }

    fn into_session(self) -> AcpSession {
        let transcript_len = self.transcript.len();
        let context_cutoff = self.context_cutoff.min(transcript_len);
        let context_summary_upto = self
            .context_summary_upto
            .max(context_cutoff)
            .min(transcript_len);
        let updated_at = self.updated_at.max(self.created_at);

        AcpSession {
            cwd: self.cwd,
            agent_hint: self.agent_hint,
            pending_prompt: None,
            transcript: self.transcript,
            last_result_output: self.last_result_output,
            context_mode: self.context_mode,
            context_cutoff,
            context_summary_text: self.context_summary_text,
            context_summary_upto,
            last_context_messages: self.last_context_messages,
            last_context_chars: self.last_context_chars,
            title: self.title,
            created_at: self.created_at,
            updated_at,
        }
    }
}

struct PromptRun {
    prompt: String,
    agent_hint: Option<String>,
}

enum PromptAction {
    Run(PromptRun),
    SetAgent {
        agent_hint: Option<String>,
        label: String,
    },
    Context {
        args: Vec<String>,
    },
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AcpContextMode {
    Off,
    Compact,
    Full,
}

impl AcpContextMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Compact => "compact",
            Self::Full => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AcpChatRole {
    User,
    Assistant,
}

impl AcpChatRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AcpChatMsg {
    role: AcpChatRole,
    content: String,
}

impl AcpChatMsg {
    fn user(content: impl Into<String>) -> Self {
        Self {
            role: AcpChatRole::User,
            content: content.into(),
        }
    }

    fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: AcpChatRole::Assistant,
            content: content.into(),
        }
    }
}

/// Serve ACP over stdin/stdout until stdin closes or the client sends `exit`.
pub async fn serve_stdio(
    orchestrator: Arc<Orchestrator>,
    agent: String,
    roles: HashMap<String, RoleConfig>,
) -> Result<()> {
    let roles = Arc::new(roles);
    let default_agent_hint = runtime::normalize_agent_hint_with_roles(&agent, &roles);
    let state = Arc::new(Mutex::new(AcpState::default()));
    let writer = RpcWriter::new();

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let message = match serde_json::from_str::<Value>(trimmed) {
            Ok(message) => message,
            Err(err) => {
                tracing::warn!("ACP parse error: {}", err);
                writer
                    .error(Value::Null, ERR_PARSE, "parse error")
                    .await
                    .ok();
                continue;
            }
        };

        let should_exit = handle_message(
            message,
            orchestrator.clone(),
            writer.clone(),
            state.clone(),
            default_agent_hint.clone(),
            roles.clone(),
        )
        .await?;
        if should_exit {
            break;
        }
    }

    Ok(())
}

async fn handle_message(
    message: Value,
    orchestrator: Arc<Orchestrator>,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
    default_agent_hint: Option<String>,
    roles: Arc<HashMap<String, RoleConfig>>,
) -> Result<bool> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(false);
    };
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => {
            let Some(id) = id else {
                return Ok(false);
            };
            writer.response(id, initialize_response()).await?;
        }
        "initialized" => {
            if let Some(id) = id {
                writer.response(id, json!({})).await?;
            }
        }
        "session/new" => {
            let Some(id) = id else {
                return Ok(false);
            };
            handle_new_session(
                id,
                params,
                writer,
                state,
                default_agent_hint,
                roles.clone(),
                orchestrator.session_store.clone(),
            )
            .await?;
        }
        "session/list" => {
            let Some(id) = id else {
                return Ok(false);
            };
            handle_list_sessions(id, params, writer, orchestrator.session_store.clone()).await?;
        }
        "session/load" => {
            let Some(id) = id else {
                return Ok(false);
            };
            handle_load_or_resume_session(
                id,
                params,
                writer,
                state,
                default_agent_hint,
                orchestrator.session_store.clone(),
                true,
            )
            .await?;
        }
        "session/resume" => {
            let Some(id) = id else {
                return Ok(false);
            };
            handle_load_or_resume_session(
                id,
                params,
                writer,
                state,
                default_agent_hint,
                orchestrator.session_store.clone(),
                false,
            )
            .await?;
        }
        "session/prompt" => {
            let Some(id) = id else {
                return Ok(false);
            };
            handle_prompt(
                id,
                params,
                orchestrator.clone(),
                writer,
                state,
                roles.clone(),
                orchestrator.session_store.clone(),
            )
            .await?;
        }
        "session/cancel" => {
            handle_cancel(params, state).await?;
        }
        "session/close" => {
            let Some(id) = id else {
                return Ok(false);
            };
            handle_close_session(id, params, writer, state).await?;
        }
        "session/delete" => {
            let Some(id) = id else {
                return Ok(false);
            };
            handle_delete_session(
                id,
                params,
                writer,
                state,
                orchestrator.session_store.clone(),
            )
            .await?;
        }
        "logout" => {
            if let Some(id) = id {
                writer.response(id, json!({})).await?;
            }
        }
        "shutdown" => {
            if let Some(id) = id {
                writer.response(id, Value::Null).await?;
            }
        }
        "exit" => return Ok(true),
        other => {
            if let Some(id) = id {
                writer
                    .error(
                        id,
                        ERR_METHOD_NOT_FOUND,
                        format!("method not found: {other}"),
                    )
                    .await?;
            }
        }
    }

    Ok(false)
}

fn initialize_response() -> Value {
    json!({
        "protocolVersion": ACP_PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {
                "image": false,
                "audio": false,
                "embeddedContext": true
            },
            "mcpCapabilities": {
                "http": false,
                "sse": false
            },
            "sessionCapabilities": {
                "close": {},
                "delete": {},
                "list": {},
                "resume": {}
            },
            "auth": {}
        },
        "agentInfo": {
            "name": "orchestrator",
            "title": "Orchestrator",
            "version": env!("CARGO_PKG_VERSION")
        },
        "authMethods": []
    })
}

async fn handle_new_session(
    id: Value,
    params: Value,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
    default_agent_hint: Option<String>,
    roles: Arc<HashMap<String, RoleConfig>>,
    store: Arc<SessionStore>,
) -> Result<()> {
    let cwd = match string_param(&params, "cwd") {
        Some(cwd) => PathBuf::from(cwd),
        None => {
            writer
                .error(id, ERR_INVALID_PARAMS, "session/new requires params.cwd")
                .await?;
            return Ok(());
        }
    };
    if !cwd.is_absolute() {
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                "session/new params.cwd must be absolute",
            )
            .await?;
        return Ok(());
    }

    let agent_hint = params
        .get("agent")
        .or_else(|| params.get("orchestratorAgent"))
        .and_then(Value::as_str)
        .map(|agent| runtime::normalize_agent_hint_with_roles(agent, &roles))
        .unwrap_or(default_agent_hint);

    let session_id = format!("sess_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    {
        let mut state = state.lock().await;
        state.sessions.insert(
            session_id.clone(),
            AcpSession {
                cwd,
                agent_hint,
                pending_prompt: None,
                transcript: Vec::new(),
                last_result_output: None,
                context_mode: AcpContextMode::Compact,
                context_cutoff: 0,
                context_summary_text: String::new(),
                context_summary_upto: 0,
                last_context_messages: 0,
                last_context_chars: 0,
                title: None,
                created_at: now,
                updated_at: now,
            },
        );
        if let Some(session) = state.sessions.get(&session_id) {
            save_acp_session(&store, &session_id, session)?;
        }
    }

    writer
        .response(id, json!({ "sessionId": session_id }))
        .await?;
    send_available_commands(&writer, &session_id).await?;
    Ok(())
}

async fn handle_list_sessions(
    id: Value,
    params: Value,
    writer: RpcWriter,
    store: Arc<SessionStore>,
) -> Result<()> {
    let cwd_filter = match string_param(&params, "cwd") {
        Some(cwd) => {
            let path = PathBuf::from(&cwd);
            if !path.is_absolute() {
                writer
                    .error(
                        id,
                        ERR_INVALID_PARAMS,
                        "session/list params.cwd must be absolute",
                    )
                    .await?;
                return Ok(());
            }
            Some(path)
        }
        None => None,
    };
    let cursor = string_param(&params, "cursor");
    let page = match parse_list_cursor(cursor.as_deref()) {
        Some(page) => page,
        None => {
            writer
                .error(
                    id,
                    ERR_INVALID_PARAMS,
                    "session/list params.cursor is invalid",
                )
                .await?;
            return Ok(());
        }
    };
    let page_size = 50usize;
    let sessions = list_acp_session_snapshots(&store, cwd_filter.as_ref(), page, page_size)?;
    let has_more = sessions.len() > page_size;
    let sessions = sessions.into_iter().take(page_size).collect::<Vec<_>>();
    let next_cursor = has_more.then(|| format!("offset:{}", page + page_size));

    let mut result = serde_json::Map::new();
    result.insert(
        "sessions".into(),
        json!(sessions
            .into_iter()
            .map(session_info_json)
            .collect::<Vec<_>>()),
    );
    if let Some(next_cursor) = next_cursor {
        result.insert("nextCursor".into(), json!(next_cursor));
    }
    writer.response(id, Value::Object(result)).await?;
    Ok(())
}

async fn handle_load_or_resume_session(
    id: Value,
    params: Value,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
    default_agent_hint: Option<String>,
    store: Arc<SessionStore>,
    replay_history: bool,
) -> Result<()> {
    let Some(session_id) = string_param(&params, "sessionId") else {
        let method_name = if replay_history {
            "session/load"
        } else {
            "session/resume"
        };
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                format!("{method_name} requires params.sessionId"),
            )
            .await?;
        return Ok(());
    };
    let cwd = match string_param(&params, "cwd") {
        Some(cwd) => PathBuf::from(cwd),
        None => {
            let method_name = if replay_history {
                "session/load"
            } else {
                "session/resume"
            };
            writer
                .error(
                    id,
                    ERR_INVALID_PARAMS,
                    format!("{method_name} requires params.cwd"),
                )
                .await?;
            return Ok(());
        }
    };
    if !cwd.is_absolute() {
        let method_name = if replay_history {
            "session/load"
        } else {
            "session/resume"
        };
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                format!("{method_name} params.cwd must be absolute"),
            )
            .await?;
        return Ok(());
    }

    let snapshot = match load_acp_session_snapshot(&store, &session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            writer
                .error(
                    id,
                    ERR_INVALID_PARAMS,
                    format!("unknown sessionId: {err:#}"),
                )
                .await?;
            return Ok(());
        }
    };
    if snapshot.cwd != cwd {
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                "session cwd does not match stored session",
            )
            .await?;
        return Ok(());
    }

    let replay_messages = snapshot.transcript.clone();
    let session = restore_acp_session(snapshot, default_agent_hint);
    let session_info_update = session_info_update_json(&session);
    {
        let mut state = state.lock().await;
        state.sessions.insert(session_id.clone(), session);
    }

    send_available_commands(&writer, &session_id).await?;
    send_session_info_update(&writer, &session_id, session_info_update).await?;
    if replay_history {
        replay_acp_transcript(&writer, &session_id, &replay_messages).await?;
    }
    let result = if replay_history {
        Value::Null
    } else {
        json!({})
    };
    writer.response(id, result).await?;
    Ok(())
}

fn restore_acp_session(
    snapshot: AcpSessionSnapshot,
    default_agent_hint: Option<String>,
) -> AcpSession {
    let mut session = snapshot.into_session();
    if session.agent_hint.is_none() {
        session.agent_hint = default_agent_hint;
    }
    session
}

async fn handle_prompt(
    id: Value,
    params: Value,
    orchestrator: Arc<Orchestrator>,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
    roles: Arc<HashMap<String, RoleConfig>>,
    store: Arc<SessionStore>,
) -> Result<()> {
    let Some(session_id) = string_param(&params, "sessionId") else {
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                "session/prompt requires params.sessionId",
            )
            .await?;
        return Ok(());
    };
    let prompt_text = match prompt_text_from_params(&params) {
        Ok(text) => text,
        Err(err) => {
            writer
                .error(id, ERR_INVALID_PARAMS, err.to_string())
                .await?;
            return Ok(());
        }
    };
    let action = classify_prompt_action_with_roles(&prompt_text, &roles);

    match action {
        PromptAction::Status => {
            let label = {
                let state = state.lock().await;
                match state.sessions.get(&session_id) {
                    Some(session) => agent_label(session.agent_hint.as_deref()),
                    None => {
                        writer
                            .error(id, ERR_INVALID_PARAMS, "unknown sessionId")
                            .await?;
                        return Ok(());
                    }
                }
            };
            send_agent_chunk(
                &writer,
                &session_id,
                &new_message_id(),
                format!("Current agent route: {label}."),
            )
            .await?;
            writer.response(id, stop_response("end_turn")).await?;
        }
        PromptAction::SetAgent { agent_hint, label } => {
            let session_info_update = {
                let mut state = state.lock().await;
                let Some(session) = state.sessions.get_mut(&session_id) else {
                    writer
                        .error(id, ERR_INVALID_PARAMS, "unknown sessionId")
                        .await?;
                    return Ok(());
                };
                session.agent_hint = agent_hint;
                touch_acp_session(session);
                save_acp_session(&store, &session_id, session)?;
                session_info_update_json(session)
            };
            send_session_info_update(&writer, &session_id, session_info_update).await?;
            send_agent_chunk(
                &writer,
                &session_id,
                &new_message_id(),
                format!("Agent route set to {label}."),
            )
            .await?;
            writer.response(id, stop_response("end_turn")).await?;
        }
        PromptAction::Context { args } => {
            let (message, session_info_update) = {
                let mut state = state.lock().await;
                let Some(session) = state.sessions.get_mut(&session_id) else {
                    writer
                        .error(id, ERR_INVALID_PARAMS, "unknown sessionId")
                        .await?;
                    return Ok(());
                };
                let message = handle_acp_context_command(session, &args);
                touch_acp_session(session);
                save_acp_session(&store, &session_id, session)?;
                (message, session_info_update_json(session))
            };
            send_session_info_update(&writer, &session_id, session_info_update).await?;
            send_agent_chunk(&writer, &session_id, &new_message_id(), message).await?;
            writer.response(id, stop_response("end_turn")).await?;
        }
        PromptAction::Run(run) => {
            start_prompt_run(id, session_id, run, orchestrator, writer, state, store).await?;
        }
    }

    Ok(())
}

async fn start_prompt_run(
    id: Value,
    session_id: String,
    run: PromptRun,
    orchestrator: Arc<Orchestrator>,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
    store: Arc<SessionStore>,
) -> Result<()> {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let (cwd, agent_hint, prompt, session_info_update) = {
        let mut state = state.lock().await;
        let Some(session) = state.sessions.get_mut(&session_id) else {
            writer
                .error(id, ERR_INVALID_PARAMS, "unknown sessionId")
                .await?;
            return Ok(());
        };
        if session.pending_prompt.is_some() {
            writer
                .error(id, ERR_SERVER, "session already has an active prompt")
                .await?;
            return Ok(());
        }
        let agent_hint = run.agent_hint.or_else(|| session.agent_hint.clone());
        let contextual_prompt = build_acp_contextual_prompt(session, &run.prompt);
        session.last_context_messages = contextual_prompt.message_count;
        session.last_context_chars = contextual_prompt.context_chars;
        let prompt = contextual_prompt.prompt;
        let user_prompt = run.prompt;
        let title_changed = session.title.is_none();
        if session.title.is_none() {
            session.title = Some(title_from_prompt(&user_prompt));
        }
        session.transcript.push(AcpChatMsg::user(user_prompt));
        touch_acp_session(session);
        session.pending_prompt = Some(PendingPrompt {
            request_id: id.clone(),
            cancel_tx,
        });
        save_acp_session(&store, &session_id, session)?;
        let session_info_update = title_changed.then(|| session_info_update_json(session));
        (session.cwd.clone(), agent_hint, prompt, session_info_update)
    };

    if let Some(update) = session_info_update {
        send_session_info_update(&writer, &session_id, update).await?;
    }

    tokio::spawn(run_prompt_task(
        id,
        session_id,
        prompt,
        cwd,
        agent_hint,
        orchestrator,
        writer,
        state,
        store,
        cancel_rx,
    ));

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_prompt_task(
    id: Value,
    session_id: String,
    prompt: String,
    cwd: PathBuf,
    agent_hint: Option<String>,
    orchestrator: Arc<Orchestrator>,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
    store: Arc<SessionStore>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let message_id = new_message_id();
    let route = agent_label(agent_hint.as_deref());
    if let Err(err) = send_agent_chunk(
        &writer,
        &session_id,
        &message_id,
        format!("Starting orchestrator run via {route}."),
    )
    .await
    {
        tracing::warn!("failed to send ACP start update: {}", err);
    }

    let mut task = Task::new(prompt);
    task.worktree = Some(cwd);
    task.agent_hint = agent_hint;

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<RunProgress>();
    let mut run_handle =
        tokio::spawn(async move { orchestrator.run_with_progress(task, progress_tx).await });
    let mut capture = AcpRunCapture::default();

    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                run_handle.abort();
                let _ = run_handle.await;
                let _ = send_agent_chunk(&writer, &session_id, &message_id, "Cancelled.").await;
                if let Some(update) =
                    append_acp_assistant_message(
                        state.clone(),
                        store.clone(),
                        &session_id,
                        "Cancelled.",
                        None,
                    )
                    .await
                {
                    let _ = send_session_info_update(&writer, &session_id, update).await;
                }
                let _ = writer.response(id.clone(), stop_response("cancelled")).await;
                break;
            }
            event = progress_rx.recv() => {
                if let Some(event) = event {
                    capture.record(&event);
                    if let Err(err) = send_progress_update(&writer, &session_id, &message_id, &event, &mut capture).await {
                        tracing::warn!("failed to send ACP progress update: {}", err);
                    }
                }
            }
            joined = &mut run_handle => {
                while let Ok(event) = progress_rx.try_recv() {
                    capture.record(&event);
                    if let Err(err) = send_progress_update(&writer, &session_id, &message_id, &event, &mut capture).await {
                        tracing::warn!("failed to send trailing ACP progress update: {}", err);
                    }
                }

                match joined {
                    Ok(Ok(tasks)) => {
                        let final_output = capture.final_output().map(ToOwned::to_owned);
                        let summary = format_acp_done_message(tasks.len(), final_output.as_deref());
                        let _ = send_agent_chunk(&writer, &session_id, &message_id, summary.clone()).await;
                        if let Some(update) =
                            append_acp_assistant_message(
                                state.clone(),
                                store.clone(),
                                &session_id,
                                summary,
                                Some(final_output),
                            ).await
                        {
                            let _ = send_session_info_update(&writer, &session_id, update).await;
                        }
                        let _ = writer.response(id.clone(), stop_response("end_turn")).await;
                    }
                    Ok(Err(err)) => {
                        let summary = format!("Failed: {err:#}");
                        let _ = send_agent_chunk(&writer, &session_id, &message_id, summary.clone()).await;
                        if let Some(update) =
                            append_acp_assistant_message(
                                state.clone(),
                                store.clone(),
                                &session_id,
                                summary,
                                None,
                            ).await
                        {
                            let _ = send_session_info_update(&writer, &session_id, update).await;
                        }
                        let _ = writer.error(id.clone(), ERR_SERVER, format!("{err:#}")).await;
                    }
                    Err(err) if err.is_cancelled() => {
                        if let Some(update) =
                            append_acp_assistant_message(
                                state.clone(),
                                store.clone(),
                                &session_id,
                                "Cancelled.",
                                None,
                            )
                            .await
                        {
                            let _ = send_session_info_update(&writer, &session_id, update).await;
                        }
                        let _ = writer.response(id.clone(), stop_response("cancelled")).await;
                    }
                    Err(err) => {
                        if let Some(update) = append_acp_assistant_message(
                            state.clone(),
                            store.clone(),
                            &session_id,
                            format!("Prompt task failed: {err}"),
                            None,
                        )
                        .await
                        {
                            let _ = send_session_info_update(&writer, &session_id, update).await;
                        }
                        let _ = writer.error(id.clone(), ERR_INTERNAL, format!("prompt task failed: {err}")).await;
                    }
                }
                break;
            }
        }
    }

    clear_pending_prompt(state, &session_id, &id).await;
}

async fn clear_pending_prompt(state: Arc<Mutex<AcpState>>, session_id: &str, request_id: &Value) {
    let mut state = state.lock().await;
    if let Some(session) = state.sessions.get_mut(session_id) {
        let should_clear = session
            .pending_prompt
            .as_ref()
            .map(|pending| pending.request_id == *request_id)
            .unwrap_or(false);
        if should_clear {
            session.pending_prompt = None;
        }
    }
}

async fn append_acp_assistant_message(
    state: Arc<Mutex<AcpState>>,
    store: Arc<SessionStore>,
    session_id: &str,
    content: impl Into<String>,
    last_result_output: Option<Option<String>>,
) -> Option<Value> {
    let mut state = state.lock().await;
    if let Some(session) = state.sessions.get_mut(session_id) {
        session
            .transcript
            .push(AcpChatMsg::assistant(content.into()));
        if let Some(last_result_output) = last_result_output {
            session.last_result_output = last_result_output;
        }
        touch_acp_session(session);
        if let Err(err) = save_acp_session(&store, session_id, session) {
            tracing::warn!("failed to save ACP session {}: {:#}", session_id, err);
        }
        return Some(session_info_update_json(session));
    }
    None
}

fn touch_acp_session(session: &mut AcpSession) {
    session.updated_at = Utc::now();
}

fn title_from_prompt(prompt: &str) -> String {
    let collapsed = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(
        if collapsed.is_empty() {
            "Untitled ACP session"
        } else {
            &collapsed
        },
        80,
    )
}

struct AcpContextualPrompt {
    prompt: String,
    message_count: usize,
    context_chars: usize,
}

fn build_acp_contextual_prompt(
    session: &mut AcpSession,
    current_prompt: &str,
) -> AcpContextualPrompt {
    refresh_acp_context_summary(session);
    let entries = selected_acp_context_entries(session);
    let summary = session.context_summary_text.trim();
    if session.context_mode == AcpContextMode::Off || (entries.is_empty() && summary.is_empty()) {
        return AcpContextualPrompt {
            prompt: current_prompt.to_string(),
            message_count: 0,
            context_chars: 0,
        };
    }

    let context = format_acp_context_block(summary, &entries);
    let prompt = format!(
        "You are continuing a multi-turn ACP conversation.\n\
         Use the conversation context to resolve follow-ups, pronouns, and references.\n\
         The current user request below is the highest priority.\n\n\
         Conversation context:\n{}\n\n\
         ---\n\
         Current user request:\n{}",
        context, current_prompt
    );

    AcpContextualPrompt {
        prompt,
        message_count: entries.len() + usize::from(!summary.is_empty()),
        context_chars: context.chars().count(),
    }
}

fn handle_acp_context_command(session: &mut AcpSession, args: &[String]) -> String {
    match args.first().map(String::as_str).unwrap_or("status") {
        "status" | "show" | "info" => {
            refresh_acp_context_summary(session);
            format_acp_context_status(session)
        }
        "on" | "compact" => {
            session.context_mode = AcpContextMode::Compact;
            refresh_acp_context_summary(session);
            format_acp_context_status(session)
        }
        "full" => {
            session.context_mode = AcpContextMode::Full;
            refresh_acp_context_summary(session);
            format_acp_context_status(session)
        }
        "off" | "none" | "disable" | "disabled" => {
            session.context_mode = AcpContextMode::Off;
            format_acp_context_status(session)
        }
        "clear" | "reset" => {
            session.context_cutoff = session.transcript.len();
            session.context_summary_text.clear();
            session.context_summary_upto = session.context_cutoff;
            session.last_result_output = None;
            session.last_context_messages = 0;
            session.last_context_chars = 0;
            "Context memory cleared for future ACP prompts. Visible client history is unchanged."
                .into()
        }
        other => format!(
            "Unknown context option: {other}\n\nCommands:\n/context\n/context compact\n/context full\n/context off\n/context clear"
        ),
    }
}

fn format_acp_context_status(session: &AcpSession) -> String {
    let entries = selected_acp_context_entries(session);
    let summary = session.context_summary_text.trim();
    format!(
        "Context memory:\n- mode: {}\n- rolling summary: {}\n- recent messages: {}\n- last injection: {} item(s), {} chars\n\nCommands:\n- /context compact\n- /context full\n- /context off\n- /context clear",
        session.context_mode.as_str(),
        if summary.is_empty() {
            "none".into()
        } else {
            format!(
                "{} chars through message {}",
                summary.chars().count(),
                session.context_summary_upto
            )
        },
        entries.len(),
        session.last_context_messages,
        session.last_context_chars
    )
}

fn refresh_acp_context_summary(session: &mut AcpSession) {
    if session.context_mode == AcpContextMode::Off {
        return;
    }

    let (message_limit, _, per_message_limit) = acp_context_limits(session.context_mode);
    let trigger = acp_summary_trigger(session.context_mode);
    let start = session.context_cutoff.min(session.transcript.len());
    let summary_start = session
        .context_summary_upto
        .max(start)
        .min(session.transcript.len());
    let available = session.transcript.len().saturating_sub(summary_start);
    if available <= message_limit + trigger {
        return;
    }

    let absorb_end = session.transcript.len().saturating_sub(message_limit);
    if absorb_end <= summary_start {
        return;
    }

    let mut pieces = Vec::new();
    if !session.context_summary_text.trim().is_empty() {
        pieces.push(session.context_summary_text.trim().to_string());
    }
    pieces.extend(
        session.transcript[summary_start..absorb_end]
            .iter()
            .filter_map(|msg| {
                let content = acp_context_content(msg.role, &msg.content);
                let content = content.trim();
                (!content.is_empty()).then(|| {
                    format!(
                        "{}: {}",
                        msg.role.as_str(),
                        truncate_chars(&content.replace('\n', " "), per_message_limit)
                    )
                })
            }),
    );

    session.context_summary_text = truncate_acp_summary(&pieces.join("\n"), session.context_mode);
    session.context_summary_upto = absorb_end;
}

fn selected_acp_context_entries(session: &AcpSession) -> Vec<AcpChatMsg> {
    if session.context_mode == AcpContextMode::Off {
        return vec![];
    }

    let (message_limit, total_limit, per_message_limit) = acp_context_limits(session.context_mode);
    let start = session
        .context_summary_upto
        .max(session.context_cutoff)
        .min(session.transcript.len());
    let mut entries: Vec<AcpChatMsg> = session.transcript[start..]
        .iter()
        .filter_map(|msg| {
            let content = acp_context_content(msg.role, &msg.content);
            let content = truncate_chars(content.trim(), per_message_limit);
            (!content.is_empty()).then(|| AcpChatMsg {
                role: msg.role,
                content,
            })
        })
        .collect();
    append_last_acp_result_context(session, &mut entries, per_message_limit);

    if entries.len() > message_limit {
        entries = entries.split_off(entries.len() - message_limit);
    }

    while entries.len() > 1 && format_acp_context_entries(&entries).chars().count() > total_limit {
        entries.remove(0);
    }

    entries
}

fn append_last_acp_result_context(
    session: &AcpSession,
    entries: &mut Vec<AcpChatMsg>,
    per_message_limit: usize,
) {
    if session.context_cutoff >= session.transcript.len() {
        return;
    }
    let Some(result) = session.last_result_output.as_deref().map(str::trim) else {
        return;
    };
    if result.is_empty() {
        return;
    }
    if entries
        .iter()
        .any(|entry| entry.role == AcpChatRole::Assistant && entry.content.contains(result))
    {
        return;
    }
    entries.push(AcpChatMsg::assistant(truncate_chars(
        &format!("Last run result:\n{result}"),
        per_message_limit,
    )));
}

fn acp_context_content(role: AcpChatRole, content: &str) -> String {
    let trimmed = content.trim();
    if role != AcpChatRole::Assistant {
        return trimmed.to_string();
    }
    if is_acp_result_capture_notice(trimmed) {
        return String::new();
    }
    if let Some((_, result)) = trimmed.split_once("Final result:") {
        let plain = result
            .trim()
            .strip_prefix("-------------")
            .unwrap_or(result.trim())
            .trim()
            .trim_matches('`')
            .trim();
        if !plain.is_empty() {
            return plain.to_string();
        }
    }
    trimmed.to_string()
}

fn is_acp_result_capture_notice(content: &str) -> bool {
    content.starts_with("✓ done")
        && (content.contains("Result captured.") || content.contains("No final text was emitted"))
}

fn acp_context_limits(mode: AcpContextMode) -> (usize, usize, usize) {
    match mode {
        AcpContextMode::Off => (0, 0, 0),
        AcpContextMode::Compact => (8, 4_000, 700),
        AcpContextMode::Full => (24, 12_000, 1_800),
    }
}

fn acp_summary_trigger(mode: AcpContextMode) -> usize {
    match mode {
        AcpContextMode::Off => usize::MAX,
        AcpContextMode::Compact => 10,
        AcpContextMode::Full => 28,
    }
}

fn truncate_acp_summary(summary: &str, mode: AcpContextMode) -> String {
    let max = match mode {
        AcpContextMode::Off => 0,
        AcpContextMode::Compact => 1_200,
        AcpContextMode::Full => 2_400,
    };
    truncate_chars(summary.trim(), max)
}

fn format_acp_context_block(summary: &str, entries: &[AcpChatMsg]) -> String {
    let recent = format_acp_context_entries(entries);
    match (summary.trim().is_empty(), recent.is_empty()) {
        (true, true) => String::new(),
        (true, false) => recent,
        (false, true) => format!("Rolling summary:\n{}", summary.trim()),
        (false, false) => format!(
            "Rolling summary:\n{}\n\nRecent messages:\n{}",
            summary.trim(),
            recent
        ),
    }
}

fn format_acp_context_entries(entries: &[AcpChatMsg]) -> String {
    entries
        .iter()
        .map(|msg| format!("{}:\n{}", msg.role.as_str(), msg.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Default)]
struct AcpRunCapture {
    final_output: Option<String>,
    last_worker_output: Option<String>,
    visible_output_keys: Vec<String>,
}

impl AcpRunCapture {
    fn record(&mut self, event: &RunProgress) {
        match event {
            RunProgress::WorkerOutput { content, .. } | RunProgress::RoleOutput { content, .. } => {
                self.remember_output(content);
            }
            _ => {}
        }
    }

    fn remember_output(&mut self, content: &str) {
        let display = agent_events::humanize_jsonish(content, 240_000);
        if display.trim().is_empty() || agent_events::is_low_value_output(&display) {
            return;
        }

        if let Some(final_output) = agent_events::final_output_candidate(content, 240_000) {
            remember_better_acp_text(&mut self.final_output, final_output);
        }
        remember_better_acp_text(&mut self.last_worker_output, display);
    }

    fn should_show_output(&mut self, scope: &str, content: &str, max: usize) -> Option<String> {
        let display = agent_events::humanize_jsonish(content, max);
        if display.trim().is_empty() || agent_events::is_low_value_output(&display) {
            return None;
        }
        let key = agent_events::normalized_output_key(content, max)
            .unwrap_or_else(|| agent_events::sanitize_text(&display, max));
        let scoped_key = format!("{scope}:{key}");
        if self
            .visible_output_keys
            .iter()
            .any(|seen| seen == &scoped_key)
        {
            return None;
        }
        self.visible_output_keys.push(scoped_key);
        if self.visible_output_keys.len() > 160 {
            self.visible_output_keys.remove(0);
        }
        Some(display)
    }

    fn final_output(&self) -> Option<&str> {
        self.final_output
            .as_deref()
            .or(self.last_worker_output.as_deref())
    }
}

fn remember_better_acp_text(slot: &mut Option<String>, candidate: String) {
    let candidate = candidate.trim().to_string();
    if candidate.is_empty() {
        return;
    }
    let should_replace = slot
        .as_ref()
        .map(|current| candidate.chars().count() > current.chars().count())
        .unwrap_or(true);
    if should_replace {
        *slot = Some(candidate);
    }
}

fn format_acp_done_message(task_count: usize, final_output: Option<&str>) -> String {
    let mut out = format!("✓ done — {task_count} sub-task(s) completed");
    match final_output.map(str::trim).filter(|text| !text.is_empty()) {
        Some(_) => {
            out.push_str("\n\nResult captured. Continue with a follow-up, or inspect the run process for details.");
        }
        None => {
            out.push_str("\n\nNo final text was emitted by the worker.");
        }
    }
    out
}

async fn handle_cancel(params: Value, state: Arc<Mutex<AcpState>>) -> Result<()> {
    let Some(session_id) = string_param(&params, "sessionId") else {
        return Ok(());
    };
    let pending = {
        let mut state = state.lock().await;
        state
            .sessions
            .get_mut(&session_id)
            .and_then(|session| session.pending_prompt.take())
    };
    if let Some(pending) = pending {
        let _ = pending.cancel_tx.send(());
    }
    Ok(())
}

async fn handle_close_session(
    id: Value,
    params: Value,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
) -> Result<()> {
    let Some(session_id) = string_param(&params, "sessionId") else {
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                "session/close requires params.sessionId",
            )
            .await?;
        return Ok(());
    };

    let pending = {
        let mut state = state.lock().await;
        state
            .sessions
            .remove(&session_id)
            .and_then(|session| session.pending_prompt)
    };
    if let Some(pending) = pending {
        let _ = pending.cancel_tx.send(());
    }

    writer.response(id, json!({})).await?;
    Ok(())
}

async fn handle_delete_session(
    id: Value,
    params: Value,
    writer: RpcWriter,
    state: Arc<Mutex<AcpState>>,
    store: Arc<SessionStore>,
) -> Result<()> {
    let Some(session_id) = string_param(&params, "sessionId") else {
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                "session/delete requires params.sessionId",
            )
            .await?;
        return Ok(());
    };
    if !is_valid_acp_session_id(&session_id) {
        writer
            .error(
                id,
                ERR_INVALID_PARAMS,
                "session/delete params.sessionId is invalid",
            )
            .await?;
        return Ok(());
    }

    let pending = {
        let mut state = state.lock().await;
        state
            .sessions
            .remove(&session_id)
            .and_then(|session| session.pending_prompt)
    };
    if let Some(pending) = pending {
        let _ = pending.cancel_tx.send(());
    }
    delete_acp_session_snapshot(&store, &session_id)?;

    writer.response(id, json!({})).await?;
    Ok(())
}

fn acp_state_dir(store: &SessionStore) -> PathBuf {
    store.log_dir().join("acp")
}

fn acp_session_path(store: &SessionStore, session_id: &str) -> PathBuf {
    debug_assert!(is_valid_acp_session_id(session_id));
    acp_state_dir(store).join(format!("{session_id}.json"))
}

fn save_acp_session(store: &SessionStore, session_id: &str, session: &AcpSession) -> Result<()> {
    if !is_valid_acp_session_id(session_id) {
        anyhow::bail!("invalid ACP session id: {session_id}");
    }
    let dir = acp_state_dir(store);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = acp_session_path(store, session_id);
    let snapshot = AcpSessionSnapshot::from_session(session_id, session);
    let body = serde_json::to_string_pretty(&snapshot)?;
    let tmp_path = dir.join(format!("{session_id}.{}.tmp", Uuid::new_v4().simple()));
    std::fs::write(&tmp_path, body).with_context(|| format!("writing {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

fn load_acp_session_snapshot(store: &SessionStore, session_id: &str) -> Result<AcpSessionSnapshot> {
    if !is_valid_acp_session_id(session_id) {
        anyhow::bail!("invalid ACP session id: {session_id}");
    }
    let path = acp_session_path(store, session_id);
    let body =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let snapshot: AcpSessionSnapshot =
        serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    if snapshot.version != 1 {
        anyhow::bail!(
            "unsupported ACP session snapshot version {}",
            snapshot.version
        );
    }
    if snapshot.session_id != session_id {
        anyhow::bail!(
            "ACP session id mismatch: requested {}, file contains {}",
            session_id,
            snapshot.session_id
        );
    }
    Ok(snapshot)
}

fn delete_acp_session_snapshot(store: &SessionStore, session_id: &str) -> Result<()> {
    if !is_valid_acp_session_id(session_id) {
        anyhow::bail!("invalid ACP session id: {session_id}");
    }
    let path = acp_session_path(store, session_id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("deleting {}", path.display())),
    }
}

fn list_acp_session_snapshots(
    store: &SessionStore,
    cwd_filter: Option<&PathBuf>,
    offset: usize,
    limit: usize,
) -> Result<Vec<AcpSessionSnapshot>> {
    let dir = acp_state_dir(store);
    let mut snapshots = Vec::new();
    if !dir.exists() {
        return Ok(snapshots);
    }

    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(body) => body,
            Err(err) => {
                tracing::warn!("could not read ACP session {}: {}", path.display(), err);
                continue;
            }
        };
        let snapshot = match serde_json::from_str::<AcpSessionSnapshot>(&body) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                tracing::warn!("could not parse ACP session {}: {}", path.display(), err);
                continue;
            }
        };
        if snapshot.version != 1 || !is_valid_acp_session_id(&snapshot.session_id) {
            tracing::warn!("skipping invalid ACP session {}", path.display());
            continue;
        }
        if cwd_filter
            .map(|cwd| snapshot.cwd.as_path() != cwd.as_path())
            .unwrap_or(false)
        {
            continue;
        }
        snapshots.push(snapshot);
    }

    snapshots.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    Ok(snapshots
        .into_iter()
        .skip(offset)
        .take(limit.saturating_add(1))
        .collect())
}

fn parse_list_cursor(cursor: Option<&str>) -> Option<usize> {
    match cursor {
        None => Some(0),
        Some(raw) => raw.strip_prefix("offset:")?.parse::<usize>().ok(),
    }
}

fn is_valid_acp_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn session_info_json(snapshot: AcpSessionSnapshot) -> Value {
    json!({
        "sessionId": snapshot.session_id,
        "cwd": snapshot.cwd,
        "title": snapshot.title,
        "updatedAt": snapshot.updated_at.to_rfc3339(),
        "_meta": {
            "messageCount": snapshot.transcript.len(),
            "contextMode": snapshot.context_mode.as_str(),
            "agentRoute": agent_label(snapshot.agent_hint.as_deref()),
        }
    })
}

fn session_info_update_json(session: &AcpSession) -> Value {
    json!({
        "sessionUpdate": "session_info_update",
        "title": session.title.clone(),
        "updatedAt": session.updated_at.to_rfc3339(),
        "_meta": {
            "messageCount": session.transcript.len(),
            "contextMode": session.context_mode.as_str(),
            "agentRoute": agent_label(session.agent_hint.as_deref()),
        }
    })
}

async fn send_session_info_update(
    writer: &RpcWriter,
    session_id: &str,
    update: Value,
) -> Result<()> {
    writer
        .notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": update,
            }),
        )
        .await
}

async fn replay_acp_transcript(
    writer: &RpcWriter,
    session_id: &str,
    messages: &[AcpChatMsg],
) -> Result<()> {
    for msg in messages {
        match msg.role {
            AcpChatRole::User => {
                send_user_chunk(writer, session_id, &new_user_message_id(), &msg.content).await?;
            }
            AcpChatRole::Assistant => {
                send_agent_chunk(writer, session_id, &new_message_id(), &msg.content).await?;
            }
        }
    }
    Ok(())
}

async fn send_available_commands(writer: &RpcWriter, session_id: &str) -> Result<()> {
    writer
        .notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": available_commands_json()
                }
            }),
        )
        .await
}

fn available_commands_json() -> Value {
    let route_hint = AGENT_RUNTIME_ROUTES
        .iter()
        .map(|route| route.label)
        .collect::<Vec<_>>()
        .join(" | ");
    let mut commands = vec![{
        json!({
        "name": "agent",
        "description": "Select worker route: auto, claude, codex, or role name",
        "input": { "hint": route_hint }
        })
    }];
    for route in AGENT_RUNTIME_ROUTES
        .iter()
        .filter(|route| route.label != "auto")
    {
        commands.push(json!({
            "name": route.label,
            "description": route.description,
            "input": { "hint": "task prompt" }
        }));
    }
    commands.push(json!({
            "name": "auto",
            "description": "Use configured orchestrator routing"
    }));
    commands.push(json!({
            "name": "context",
            "description": "Show or change multi-turn context memory",
            "input": { "hint": "compact | full | off | clear" }
    }));
    Value::Array(commands)
}

async fn send_progress_update(
    writer: &RpcWriter,
    session_id: &str,
    message_id: &str,
    event: &RunProgress,
    capture: &mut AcpRunCapture,
) -> Result<()> {
    match event {
        RunProgress::WorkerStarted {
            task_id,
            agent,
            role,
            ..
        } => {
            send_tool_call(
                writer,
                session_id,
                &task_id.to_string(),
                format!("{role} worker ({agent})"),
                "execute",
                "in_progress",
            )
            .await
        }
        RunProgress::WorkerOutput {
            task_id,
            agent,
            role,
            content,
        } => {
            let scope = format!("worker:{role}:{agent}:{}", short_uuid(task_id));
            let Some(text) = capture.should_show_output(&scope, content, 4000) else {
                return Ok(());
            };
            send_tool_call_content(writer, session_id, &task_id.to_string(), text).await
        }
        RunProgress::WorkerDone { task_id, ok, .. } => {
            let status = if *ok { "completed" } else { "failed" };
            send_tool_call_update_status(writer, session_id, &task_id.to_string(), status).await
        }
        _ => {
            let Some(text) = progress_text(event, capture) else {
                return Ok(());
            };
            send_agent_chunk(writer, session_id, message_id, text).await
        }
    }
}

async fn send_agent_chunk(
    writer: &RpcWriter,
    session_id: &str,
    message_id: &str,
    text: impl Into<String>,
) -> Result<()> {
    writer
        .notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": message_id,
                    "content": {
                        "type": "text",
                        "text": text.into()
                    }
                }
            }),
        )
        .await
}

async fn send_user_chunk(
    writer: &RpcWriter,
    session_id: &str,
    message_id: &str,
    text: &str,
) -> Result<()> {
    writer
        .notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "user_message_chunk",
                    "messageId": message_id,
                    "content": {
                        "type": "text",
                        "text": text
                    }
                }
            }),
        )
        .await
}

async fn send_tool_call(
    writer: &RpcWriter,
    session_id: &str,
    tool_call_id: &str,
    title: impl Into<String>,
    kind: &str,
    status: &str,
) -> Result<()> {
    writer
        .notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": tool_call_id,
                    "title": title.into(),
                    "kind": kind,
                    "status": status
                }
            }),
        )
        .await
}

async fn send_tool_call_content(
    writer: &RpcWriter,
    session_id: &str,
    tool_call_id: &str,
    text: String,
) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    writer
        .notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": tool_call_id,
                    "content": [
                        {
                            "type": "content",
                            "content": {
                                "type": "text",
                                "text": text
                            }
                        }
                    ]
                }
            }),
        )
        .await
}

async fn send_tool_call_update_status(
    writer: &RpcWriter,
    session_id: &str,
    tool_call_id: &str,
    status: &str,
) -> Result<()> {
    writer
        .notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": tool_call_id,
                    "status": status
                }
            }),
        )
        .await
}

fn progress_text(event: &RunProgress, capture: &mut AcpRunCapture) -> Option<String> {
    match event {
        RunProgress::Planning => Some("Planning task.".into()),
        RunProgress::Planned { sub_task_count } => {
            Some(format!("Planned {sub_task_count} sub-task(s)."))
        }
        RunProgress::Critiquing { round } => Some(format!("Critiquing plan, round {round}.")),
        RunProgress::CritiqueResult { approved, issues } => {
            if *approved {
                Some("Plan critique approved.".into())
            } else {
                Some(format!("Plan critique found {issues} issue(s)."))
            }
        }
        RunProgress::Replanning { attempt } => Some(format!("Replanning, attempt {attempt}.")),
        RunProgress::Executing { sub_task_count } => {
            Some(format!("Executing {sub_task_count} sub-task(s)."))
        }
        RunProgress::RoleOutput { role, content } => {
            if agent_events::is_redundant_role_output(role, content, 1600) {
                return None;
            }
            let text = capture.should_show_output(&format!("role:{role}"), content, 1600)?;
            Some(format!("{role}: {text}"))
        }
        RunProgress::Reviewing { task_id } => {
            Some(format!("Reviewing task {}.", short_uuid(task_id)))
        }
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            if *approved {
                Some(format!("Review approved task {}.", short_uuid(task_id)))
            } else {
                Some(format!(
                    "Review rejected task {} with {issues} issue(s).",
                    short_uuid(task_id)
                ))
            }
        }
        RunProgress::Done { task_count } => Some(format!("Done: {task_count} task(s) completed.")),
        RunProgress::Failed(msg) => Some(format!(
            "Failed: {}",
            agent_events::humanize_jsonish(msg, 1600)
        )),
        RunProgress::WorkerStarted { .. }
        | RunProgress::WorkerOutput { .. }
        | RunProgress::WorkerDone { .. } => None,
    }
}

fn stop_response(reason: &str) -> Value {
    json!({ "stopReason": reason })
}

fn prompt_text_from_params(params: &Value) -> Result<String> {
    let prompt = params
        .get("prompt")
        .ok_or_else(|| anyhow!("session/prompt requires params.prompt"))?;
    let mut parts = Vec::new();
    append_content_text(prompt, &mut parts);
    let text = parts.join("\n\n").trim().to_string();
    if text.is_empty() {
        anyhow::bail!("session/prompt params.prompt did not contain text");
    }
    Ok(text)
}

fn append_content_text(value: &Value, parts: &mut Vec<String>) {
    if let Some(text) = value.as_str() {
        if !text.trim().is_empty() {
            parts.push(text.trim().to_string());
        }
        return;
    }

    if let Some(items) = value.as_array() {
        for item in items {
            append_content_text(item, parts);
        }
        return;
    }

    let Some(object) = value.as_object() else {
        return;
    };

    match object.get("type").and_then(Value::as_str) {
        Some("text") => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
        Some("resource") => {
            if let Some(resource) = object.get("resource") {
                let uri = resource
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or("resource");
                if let Some(text) = resource.get("text").and_then(Value::as_str) {
                    parts.push(format!("Resource {uri}:\n{text}"));
                } else if let Some(blob) = resource.get("blob").and_then(Value::as_str) {
                    parts.push(format!(
                        "Resource {uri}: embedded blob ({} bytes).",
                        blob.len()
                    ));
                }
            }
        }
        Some("resource_link") => {
            let uri = object
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or("resource");
            let name = object.get("name").and_then(Value::as_str).unwrap_or(uri);
            parts.push(format!("Resource link {name}: {uri}"));
        }
        Some("image") => {
            let mime = object
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image");
            parts.push(format!("Image content omitted ({mime})."));
        }
        Some("audio") => {
            let mime = object
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("audio");
            parts.push(format!("Audio content omitted ({mime})."));
        }
        _ => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }
}

#[cfg(test)]
fn classify_prompt_action(prompt: &str) -> PromptAction {
    classify_prompt_action_with_roles(prompt, &HashMap::new())
}

fn classify_prompt_action_with_roles(
    prompt: &str,
    roles: &HashMap<String, RoleConfig>,
) -> PromptAction {
    let trimmed = prompt.trim();
    if trimmed.eq_ignore_ascii_case("/agent") {
        return PromptAction::Status;
    }
    if let Some(rest) = slash_command_arg(trimmed, "agent") {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let agent = parts.next().unwrap_or_default();
        if agent.is_empty() {
            return PromptAction::Status;
        }
        let agent_hint = runtime::normalize_agent_hint_with_roles(agent, roles);
        let label = runtime::agent_label(agent_hint.as_deref());
        return PromptAction::SetAgent { agent_hint, label };
    }
    if let Some(rest) = slash_command_arg(trimmed, "context") {
        return PromptAction::Context {
            args: rest.split_whitespace().map(str::to_string).collect(),
        };
    }
    for route in AGENT_RUNTIME_ROUTES {
        if let Some(rest) = slash_command_arg(trimmed, route.label) {
            let rest = rest.trim();
            let agent_hint = route
                .runtime
                .and_then(|runtime_id| runtime::default_worker_role_for_runtime(roles, runtime_id))
                .or_else(|| route.role_hint.map(str::to_string));
            if rest.is_empty() {
                return PromptAction::SetAgent {
                    agent_hint,
                    label: route.label.into(),
                };
            }
            return PromptAction::Run(PromptRun {
                prompt: rest.to_string(),
                agent_hint,
            });
        }
    }

    PromptAction::Run(PromptRun {
        prompt: prompt.to_string(),
        agent_hint: None,
    })
}

fn slash_command_arg<'a>(prompt: &'a str, command: &str) -> Option<&'a str> {
    let prefix = format!("/{command}");
    let rest = prompt.strip_prefix(&prefix)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim_start())
    } else {
        None
    }
}

#[cfg(test)]
fn normalize_agent_hint(agent: &str) -> Option<String> {
    runtime::normalize_agent_hint(agent)
}

fn agent_label(agent_hint: Option<&str>) -> String {
    runtime::agent_label(agent_hint)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, c) in s.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        if c.is_control() && c != '\n' && c != '\t' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

fn string_param(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .or_else(|| {
            if key == "sessionId" {
                params.get("session_id")
            } else {
                None
            }
        })
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn new_message_id() -> String {
    format!("msg_agent_{}", Uuid::new_v4().simple())
}

fn new_user_message_id() -> String {
    format!("msg_user_{}", Uuid::new_v4().simple())
}

fn short_uuid(id: &Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoleConfig;
    use crate::core::types::{
        CritiqueOutput, Event, PlanOutput, ReviewContext, ReviewOutput, Role, Session,
    };
    use crate::core::worker::{WorkerAdapter, WorkerHandle};
    use crate::roles::critic::Critic;
    use crate::roles::planner::Planner;
    use crate::roles::reviewer::Reviewer;
    use crate::roles::router::CapabilityRouter;
    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};

    fn test_session() -> AcpSession {
        let now = Utc::now();
        AcpSession {
            cwd: PathBuf::from("/tmp"),
            agent_hint: None,
            pending_prompt: None,
            transcript: Vec::new(),
            last_result_output: None,
            context_mode: AcpContextMode::Compact,
            context_cutoff: 0,
            context_summary_text: String::new(),
            context_summary_upto: 0,
            last_context_messages: 0,
            last_context_chars: 0,
            title: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn temp_store() -> (tempfile::TempDir, SessionStore) {
        let temp = tempfile::tempdir().unwrap();
        let log_dir = temp.path().join("logs");
        let db_path = temp.path().join("sessions.sqlite");
        let store = SessionStore::open(&log_dir, &db_path).unwrap();
        (temp, store)
    }

    struct AcpTestPlanner;

    #[async_trait]
    impl Planner for AcpTestPlanner {
        async fn plan(&self, top_task: &Task) -> Result<PlanOutput> {
            let mut task = top_task.clone();
            task.id = Uuid::new_v4();
            Ok(PlanOutput {
                sub_tasks: vec![task],
                rationale: "test plan".into(),
                estimated_cost_usd: 0.0,
            })
        }

        async fn replan(&self, top_task: &Task, _critique: &CritiqueOutput) -> Result<PlanOutput> {
            self.plan(top_task).await
        }
    }

    struct AcpTestCritic;

    #[async_trait]
    impl Critic for AcpTestCritic {
        async fn critique(&self, _top_task: &Task, _plan: &PlanOutput) -> Result<CritiqueOutput> {
            Ok(CritiqueOutput {
                approved: true,
                issues: vec![],
                suggestions: vec![],
            })
        }
    }

    struct AcpTestReviewer;

    #[async_trait]
    impl Reviewer for AcpTestReviewer {
        async fn review(&self, _task: &Task, _ctx: &ReviewContext) -> Result<ReviewOutput> {
            Ok(ReviewOutput {
                approved: true,
                issues: vec![],
            })
        }
    }

    struct AcpTestAdapter;

    #[async_trait]
    impl WorkerAdapter for AcpTestAdapter {
        fn name(&self) -> &str {
            "test-worker"
        }

        async fn start(
            &self,
            task: &Task,
            event_tx: Option<mpsc::UnboundedSender<Event>>,
        ) -> Result<WorkerHandle> {
            let session = Session::new(task.id, self.name(), Role::Worker);
            if let Some(tx) = event_tx {
                let _ = tx.send(Event {
                    session_id: session.id,
                    task_id: task.id,
                    ts: Utc::now(),
                    kind: "assistant".into(),
                    payload: json!({
                        "message": {
                            "content": [
                                { "type": "text", "text": "working through ACP prompt" }
                            ]
                        }
                    }),
                });
                let _ = tx.send(Event {
                    session_id: session.id,
                    task_id: task.id,
                    ts: Utc::now(),
                    kind: "result".into(),
                    payload: json!({ "result": "ACP prompt completed." }),
                });
            }
            Ok(WorkerHandle {
                session,
                kill: Arc::new(|| {}),
            })
        }

        fn stream_events(&self, _session: &Session) -> BoxStream<'static, Result<Event>> {
            Box::pin(stream::empty())
        }

        async fn cancel(&self, _session: &Session) -> Result<()> {
            Ok(())
        }

        async fn get_diff(&self, _session: &Session) -> Result<String> {
            Ok(String::new())
        }

        async fn build_context(
            &self,
            _reader: &dyn crate::core::session_store::SessionReader,
            _parent_ids: &[Uuid],
        ) -> Result<String> {
            Ok(String::new())
        }
    }

    fn test_orchestrator(store: Arc<SessionStore>) -> Orchestrator {
        let mut roles = HashMap::new();
        roles.insert(
            "worker-cc".into(),
            RoleConfig {
                model: "test-model".into(),
                runtime: "test-runtime".into(),
                agent_teams: false,
            },
        );

        let mut adapters: HashMap<String, Arc<dyn WorkerAdapter>> = HashMap::new();
        adapters.insert("test-runtime".into(), Arc::new(AcpTestAdapter));

        Orchestrator::new(
            Arc::new(AcpTestPlanner),
            Arc::new(AcpTestCritic),
            Arc::new(AcpTestReviewer),
            Arc::new(CapabilityRouter::new(&roles, &[])),
            adapters,
            store,
            1,
            true,
            false,
        )
    }

    async fn drain_acp_messages(messages: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
        messages.lock().await.clone()
    }

    #[test]
    fn normalizes_builtin_agent_names() {
        assert_eq!(normalize_agent_hint("auto"), None);
        assert_eq!(normalize_agent_hint("claude"), Some("worker-cc".into()));
        assert_eq!(normalize_agent_hint("codex"), Some("worker-codex".into()));
        assert_eq!(normalize_agent_hint("worker-x"), Some("worker-x".into()));
    }

    #[tokio::test]
    async fn acp_protocol_flow_initializes_creates_session_and_runs_prompt() {
        let (_temp, store) = temp_store();
        let store = Arc::new(store);
        let orchestrator = Arc::new(test_orchestrator(store.clone()));
        let state = Arc::new(Mutex::new(AcpState::default()));
        let (writer, messages) = RpcWriter::memory();
        let roles = Arc::new(HashMap::new());

        handle_message(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": 1 }
            }),
            orchestrator.clone(),
            writer.clone(),
            state.clone(),
            Some("worker-cc".into()),
            roles.clone(),
        )
        .await
        .unwrap();

        handle_message(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/new",
                "params": { "cwd": "/tmp" }
            }),
            orchestrator.clone(),
            writer.clone(),
            state.clone(),
            Some("worker-cc".into()),
            roles.clone(),
        )
        .await
        .unwrap();

        let session_id = drain_acp_messages(&messages)
            .await
            .into_iter()
            .find(|message| message.get("id") == Some(&json!(2)))
            .and_then(|message| {
                message
                    .get("result")
                    .and_then(|result| result.get("sessionId"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .expect("session/new response");

        handle_message(
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [
                        { "type": "text", "text": "complete an ACP smoke test" }
                    ]
                }
            }),
            orchestrator,
            writer,
            state,
            Some("worker-cc".into()),
            roles,
        )
        .await
        .unwrap();

        let mut final_messages = Vec::new();
        for _ in 0..50 {
            final_messages = drain_acp_messages(&messages).await;
            if final_messages.iter().any(|message| {
                message.get("id") == Some(&json!(3))
                    && message
                        .get("result")
                        .and_then(|result| result.get("stopReason"))
                        == Some(&json!("end_turn"))
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert!(final_messages.iter().any(|message| {
            message.get("id") == Some(&json!(1))
                && message
                    .get("result")
                    .and_then(|result| result.get("agentCapabilities"))
                    .is_some()
        }));
        assert!(final_messages.iter().any(|message| {
            message.pointer("/params/update/sessionUpdate")
                == Some(&json!("available_commands_update"))
        }));
        assert!(final_messages.iter().any(|message| {
            message.pointer("/params/update/sessionUpdate") == Some(&json!("tool_call"))
                && message.pointer("/params/update/kind") == Some(&json!("execute"))
        }));
        assert!(final_messages.iter().any(|message| {
            message.pointer("/params/update/sessionUpdate") == Some(&json!("tool_call_update"))
                && message.to_string().contains("working through ACP prompt")
        }));
        assert!(final_messages.iter().any(|message| {
            message.pointer("/params/update/sessionUpdate") == Some(&json!("agent_message_chunk"))
                && message.to_string().contains("Result captured.")
                && !message.to_string().contains("ACP prompt completed.")
        }));
        assert!(final_messages.iter().any(|message| {
            message.get("id") == Some(&json!(3))
                && message
                    .get("result")
                    .and_then(|result| result.get("stopReason"))
                    == Some(&json!("end_turn"))
        }));
    }

    #[test]
    fn extracts_text_prompt_blocks() {
        let params = json!({
            "prompt": [
                { "type": "text", "text": "Do the task" },
                {
                    "type": "resource",
                    "resource": {
                        "uri": "file:///tmp/a.rs",
                        "text": "fn main() {}"
                    }
                }
            ]
        });

        let text = prompt_text_from_params(&params).unwrap();
        assert!(text.contains("Do the task"));
        assert!(text.contains("Resource file:///tmp/a.rs"));
        assert!(text.contains("fn main() {}"));
    }

    #[test]
    fn classifies_slash_commands() {
        match classify_prompt_action("/claude fix tests") {
            PromptAction::Run(run) => {
                assert_eq!(run.prompt, "fix tests");
                assert_eq!(run.agent_hint.as_deref(), Some("worker-cc"));
            }
            _ => panic!("expected run"),
        }

        match classify_prompt_action("/agent codex") {
            PromptAction::SetAgent { agent_hint, label } => {
                assert_eq!(agent_hint.as_deref(), Some("worker-codex"));
                assert_eq!(label, "codex");
            }
            _ => panic!("expected set agent"),
        }

        match classify_prompt_action("/context full") {
            PromptAction::Context { args } => {
                assert_eq!(args, vec!["full"]);
            }
            _ => panic!("expected context command"),
        }
    }

    #[test]
    fn classifies_claude_route_with_configured_worker_role() {
        let roles = HashMap::from([(
            "worker-cc-minimax".into(),
            RoleConfig {
                model: "minimax".into(),
                runtime: "claude-code".into(),
                agent_teams: true,
            },
        )]);

        match classify_prompt_action_with_roles("/claude fix tests", &roles) {
            PromptAction::Run(run) => {
                assert_eq!(run.prompt, "fix tests");
                assert_eq!(run.agent_hint.as_deref(), Some("worker-cc-minimax"));
            }
            _ => panic!("expected run"),
        }

        match classify_prompt_action_with_roles("/agent claude", &roles) {
            PromptAction::SetAgent { agent_hint, .. } => {
                assert_eq!(agent_hint.as_deref(), Some("worker-cc-minimax"));
            }
            _ => panic!("expected set agent"),
        }
    }

    #[test]
    fn acp_contextual_prompt_includes_prior_turns() {
        let mut session = test_session();
        session
            .transcript
            .push(AcpChatMsg::user("build the TUI queue controls"));
        session.transcript.push(AcpChatMsg::assistant(
            "✓ done — 1 sub-task(s) completed\n\nResult captured. Continue with a follow-up, or inspect the run process for details.",
        ));
        session.last_result_output = Some("Queue controls were added.".into());

        let built = build_acp_contextual_prompt(&mut session, "what changed?");

        assert!(built.prompt.contains("multi-turn ACP conversation"));
        assert!(built.prompt.contains("user:\nbuild the TUI queue controls"));
        assert!(built.prompt.contains("assistant:\nLast run result:"));
        assert!(built.prompt.contains("Queue controls were added."));
        assert!(!built.prompt.contains("Result captured."));
        assert!(built
            .prompt
            .contains("Current user request:\nwhat changed?"));
        assert_eq!(built.message_count, 2);
        assert!(built.context_chars > 0);
    }

    #[test]
    fn acp_context_clear_keeps_transcript_but_stops_injection() {
        let mut session = test_session();
        session.transcript.push(AcpChatMsg::user("old"));
        session.last_result_output = Some("old result".into());

        let clear = handle_acp_context_command(&mut session, &["clear".into()]);
        let built = build_acp_contextual_prompt(&mut session, "new");

        assert!(clear.contains("Context memory cleared"));
        assert_eq!(session.transcript.len(), 1);
        assert!(session.last_result_output.is_none());
        assert_eq!(built.prompt, "new");
        assert_eq!(built.message_count, 0);
    }

    #[test]
    fn acp_context_rolls_older_messages_into_summary() {
        let mut session = test_session();
        for idx in 0..24 {
            session
                .transcript
                .push(AcpChatMsg::user(format!("request {idx}")));
            session
                .transcript
                .push(AcpChatMsg::assistant(format!("answer {idx}")));
        }

        let built = build_acp_contextual_prompt(&mut session, "continue");

        assert!(built.prompt.contains("Rolling summary:"));
        assert!(built.prompt.contains("Recent messages:"));
        assert!(session.context_summary_upto > 0);
        assert!(!session.context_summary_text.is_empty());
        assert!(built.prompt.contains("request 0"));
        assert!(built.prompt.contains("answer 23"));
    }

    #[test]
    fn acp_final_result_is_extracted_from_worker_output() {
        let mut capture = AcpRunCapture::default();
        let task_id = Uuid::new_v4();

        capture.record(&RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: "claude-code result: {\"result\":\"Implemented ACP context memory.\"}".into(),
        });

        let final_output = capture.final_output().expect("final output");
        assert_eq!(final_output, "Implemented ACP context memory.");

        let message = format_acp_done_message(1, Some(final_output));
        assert!(message.contains("✓ done"));
        assert!(message.contains("Result captured."));
        assert!(!message.contains("```text"));
        assert!(!message.contains("Final result:"));
        assert!(!message.contains("Implemented ACP context memory."));
        assert!(!message.contains("{\"result\""));
    }

    #[test]
    fn acp_capture_dedupes_visible_worker_output() {
        let mut capture = AcpRunCapture::default();

        let first = capture.should_show_output("worker", "claude assistant: useful summary", 200);
        let second = capture.should_show_output("worker", "claude result: useful summary", 200);
        let role =
            capture.should_show_output("role:reviewer", "claude result: useful summary", 200);

        assert_eq!(first.as_deref(), Some("claude assistant: useful summary"));
        assert!(second.is_none());
        assert_eq!(role.as_deref(), Some("claude result: useful summary"));
    }

    #[test]
    fn acp_progress_text_humanizes_role_json() {
        let mut capture = AcpRunCapture::default();
        let text = progress_text(
            &RunProgress::RoleOutput {
                role: "diagnostic".into(),
                content: "```json\n{\"approved\":false,\"issues\":[\"missing test\"]}\n```".into(),
            },
            &mut capture,
        )
        .expect("progress text");

        assert_eq!(
            text,
            "diagnostic: needs changes: 1 issue(s)\n  - missing test"
        );
        assert!(!text.contains('{'));

        let text = progress_text(
            &RunProgress::RoleOutput {
                role: "critic".into(),
                content: r#"{"approved":false,"issues":["missing test"]}"#.into(),
            },
            &mut capture,
        )
        .expect("critic issue progress text");

        assert_eq!(text, "critic: needs changes: 1 issue(s)\n  - missing test");
        assert!(!text.contains('{'));
    }

    #[test]
    fn initialize_advertises_persistent_session_capabilities() {
        let response = initialize_response();
        let caps = &response["agentCapabilities"];

        assert_eq!(caps["loadSession"], true);
        assert!(caps["sessionCapabilities"]["list"].is_object());
        assert!(caps["sessionCapabilities"]["resume"].is_object());
        assert!(caps["sessionCapabilities"]["close"].is_object());
        assert!(caps["sessionCapabilities"]["delete"].is_object());
    }

    #[test]
    fn acp_session_snapshot_roundtrips_context_state() {
        let mut session = test_session();
        session.cwd = PathBuf::from("/tmp/project");
        session.agent_hint = Some("worker-codex".into());
        session.transcript.push(AcpChatMsg::user("first"));
        session
            .transcript
            .push(AcpChatMsg::assistant("first answer"));
        session.context_mode = AcpContextMode::Full;
        session.context_cutoff = 1;
        session.context_summary_text = "summary".into();
        session.context_summary_upto = 2;
        session.last_result_output = Some("first result".into());
        session.last_context_messages = 2;
        session.last_context_chars = 40;
        session.title = Some("first".into());

        let (_temp, store) = temp_store();
        save_acp_session(&store, "sess_roundtrip", &session).unwrap();

        let snapshot = load_acp_session_snapshot(&store, "sess_roundtrip").unwrap();
        assert_eq!(snapshot.session_id, "sess_roundtrip");
        assert_eq!(snapshot.cwd, PathBuf::from("/tmp/project"));
        assert_eq!(snapshot.agent_hint.as_deref(), Some("worker-codex"));
        assert_eq!(snapshot.transcript.len(), 2);
        assert_eq!(snapshot.last_result_output.as_deref(), Some("first result"));

        let restored = snapshot.into_session();
        assert_eq!(restored.context_mode, AcpContextMode::Full);
        assert_eq!(restored.context_cutoff, 1);
        assert_eq!(restored.context_summary_text, "summary");
        assert_eq!(restored.context_summary_upto, 2);
        assert_eq!(restored.last_result_output.as_deref(), Some("first result"));
        assert_eq!(restored.title.as_deref(), Some("first"));
        assert!(restored.pending_prompt.is_none());
    }

    #[test]
    fn acp_snapshot_load_rejects_invalid_session_ids() {
        let (_temp, store) = temp_store();
        let session = test_session();

        let save_err = save_acp_session(&store, "../bad", &session).unwrap_err();
        assert!(save_err.to_string().contains("invalid ACP session id"));

        let load_err = load_acp_session_snapshot(&store, "../bad").unwrap_err();
        assert!(load_err.to_string().contains("invalid ACP session id"));
    }

    #[test]
    fn acp_delete_session_snapshot_is_idempotent() {
        let (_temp, store) = temp_store();
        let session = test_session();

        save_acp_session(&store, "sess_delete", &session).unwrap();
        assert!(load_acp_session_snapshot(&store, "sess_delete").is_ok());

        delete_acp_session_snapshot(&store, "sess_delete").unwrap();
        delete_acp_session_snapshot(&store, "sess_delete").unwrap();

        assert!(load_acp_session_snapshot(&store, "sess_delete").is_err());
        let snapshots = list_acp_session_snapshots(&store, None, 0, 10).unwrap();
        assert!(snapshots.is_empty());
    }

    #[test]
    fn acp_list_sessions_sorts_filters_and_paginates() {
        let (_temp, store) = temp_store();
        let mut old = test_session();
        old.cwd = PathBuf::from("/tmp/project-a");
        old.title = Some("old".into());
        old.updated_at = DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut new = test_session();
        new.cwd = PathBuf::from("/tmp/project-a");
        new.title = Some("new".into());
        new.updated_at = DateTime::parse_from_rfc3339("2025-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut other = test_session();
        other.cwd = PathBuf::from("/tmp/project-b");
        other.title = Some("other".into());
        other.updated_at = DateTime::parse_from_rfc3339("2025-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        save_acp_session(&store, "sess_old", &old).unwrap();
        save_acp_session(&store, "sess_new", &new).unwrap();
        save_acp_session(&store, "sess_other", &other).unwrap();

        let filtered =
            list_acp_session_snapshots(&store, Some(&PathBuf::from("/tmp/project-a")), 0, 10)
                .unwrap();
        let ids = filtered
            .iter()
            .map(|snapshot| snapshot.session_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["sess_new", "sess_old"]);

        let first_page = list_acp_session_snapshots(&store, None, 0, 1).unwrap();
        assert_eq!(first_page.len(), 2);
        assert_eq!(first_page[0].session_id, "sess_other");

        let second_page = list_acp_session_snapshots(&store, None, 1, 1).unwrap();
        assert_eq!(second_page[0].session_id, "sess_new");
    }

    #[test]
    fn session_info_json_contains_acp_metadata() {
        let mut session = test_session();
        session.cwd = PathBuf::from("/tmp/project");
        session.agent_hint = Some("worker-cc".into());
        session.title = Some("Fix TUI".into());
        session.transcript.push(AcpChatMsg::user("fix tui"));

        let snapshot = AcpSessionSnapshot::from_session("sess_info", &session);
        let info = session_info_json(snapshot);

        assert_eq!(info["sessionId"], "sess_info");
        assert_eq!(info["cwd"], "/tmp/project");
        assert_eq!(info["title"], "Fix TUI");
        assert_eq!(info["_meta"]["messageCount"], 1);
        assert_eq!(info["_meta"]["contextMode"], "compact");
        assert_eq!(info["_meta"]["agentRoute"], "claude");
    }

    #[test]
    fn session_info_update_contains_title_and_message_count() {
        let mut session = test_session();
        session.title = Some("Persist ACP".into());
        session.transcript.push(AcpChatMsg::user("persist acp"));

        let update = session_info_update_json(&session);

        assert_eq!(update["sessionUpdate"], "session_info_update");
        assert_eq!(update["title"], "Persist ACP");
        assert_eq!(update["_meta"]["messageCount"], 1);
        assert!(update["updatedAt"].as_str().is_some());
    }

    #[test]
    fn restore_acp_session_preserves_activity_time_and_applies_default_agent() {
        let created_at = DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let updated_at = DateTime::parse_from_rfc3339("2025-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let snapshot = AcpSessionSnapshot {
            version: 1,
            session_id: "sess_restore".into(),
            cwd: PathBuf::from("/tmp"),
            agent_hint: None,
            transcript: vec![AcpChatMsg::user("hello")],
            last_result_output: None,
            context_mode: AcpContextMode::Compact,
            context_cutoff: 0,
            context_summary_text: String::new(),
            context_summary_upto: 0,
            last_context_messages: 0,
            last_context_chars: 0,
            title: Some("hello".into()),
            created_at,
            updated_at,
        };

        let restored = restore_acp_session(snapshot, Some("worker-codex".into()));

        assert_eq!(restored.agent_hint.as_deref(), Some("worker-codex"));
        assert_eq!(restored.created_at, created_at);
        assert_eq!(restored.updated_at, updated_at);
    }

    #[test]
    fn available_commands_include_agent_routes_and_context() {
        let commands = available_commands_json();
        let names = commands
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|command| command.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["agent", "claude", "codex", "auto", "context"]);
    }

    #[test]
    fn appending_acp_assistant_message_returns_session_info_update() {
        tokio_test::block_on(async {
            let (_temp, store) = temp_store();
            let store = Arc::new(store);
            let mut state = AcpState::default();
            let mut session = test_session();
            session.title = Some("Append".into());
            state.sessions.insert("sess_append".into(), session);
            let state = Arc::new(Mutex::new(state));

            let update =
                append_acp_assistant_message(state, store, "sess_append", "answer", None).await;

            let update = update.expect("session info update");
            assert_eq!(update["sessionUpdate"], "session_info_update");
            assert_eq!(update["_meta"]["messageCount"], 1);
            assert_eq!(update["title"], "Append");
        });
    }

    #[test]
    fn acp_snapshot_restore_clamps_context_indices() {
        let now = Utc::now();
        let snapshot = AcpSessionSnapshot {
            version: 1,
            session_id: "sess_clamp".into(),
            cwd: PathBuf::from("/tmp"),
            agent_hint: None,
            transcript: vec![AcpChatMsg::user("only message")],
            last_result_output: None,
            context_mode: AcpContextMode::Compact,
            context_cutoff: 99,
            context_summary_text: "summary".into(),
            context_summary_upto: 100,
            last_context_messages: 0,
            last_context_chars: 0,
            title: None,
            created_at: now,
            updated_at: now,
        };

        let restored = snapshot.into_session();

        assert_eq!(restored.context_cutoff, 1);
        assert_eq!(restored.context_summary_upto, 1);
    }

    #[test]
    fn parse_list_cursor_accepts_only_offset_tokens() {
        assert_eq!(parse_list_cursor(None), Some(0));
        assert_eq!(parse_list_cursor(Some("offset:50")), Some(50));
        assert_eq!(parse_list_cursor(Some("bad")), None);
        assert_eq!(parse_list_cursor(Some("offset:not-a-number")), None);
    }
}
