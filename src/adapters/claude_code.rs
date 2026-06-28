//! Claude Code adapter — invokes `claude` CLI as a subprocess.

use crate::agent_events;
use crate::cc_config::CCConfig;
use crate::config::ProviderConfig;
use crate::core::session_store::{SessionReader, SessionStore};
use crate::core::types::{Event, Session, Task};
use crate::core::worker::{format_context, WorkerAdapter};
use crate::storage::worktree::WorktreePool;
use crate::task_policy::is_conversational_task;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use super::{stream_cli_session_events, StderrCapture};

pub struct ClaudeCodeAdapter {
    binary: String,
    model: String,
    use_agent_teams: bool,
    bypass_permissions: bool,
    worktree_pool: Arc<WorktreePool>,
    session_store: Arc<SessionStore>,
    cc_config: Arc<CCConfig>,
    providers: Arc<HashMap<String, ProviderConfig>>,
}

impl ClaudeCodeAdapter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binary: String,
        model: String,
        use_agent_teams: bool,
        bypass_permissions: bool,
        worktree_pool: Arc<WorktreePool>,
        session_store: Arc<SessionStore>,
        cc_config: Arc<CCConfig>,
        providers: Arc<HashMap<String, ProviderConfig>>,
    ) -> Self {
        Self {
            binary,
            model,
            use_agent_teams,
            bypass_permissions,
            worktree_pool,
            session_store,
            cc_config,
            providers,
        }
    }
}

#[async_trait]
impl WorkerAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn binary_hint(&self) -> Option<&str> {
        Some(&self.binary)
    }

    async fn start(
        &self,
        task: &Task,
        event_tx: Option<UnboundedSender<Event>>,
    ) -> Result<crate::core::worker::WorkerHandle> {
        let model = task
            .model_hint
            .clone()
            .unwrap_or_else(|| self.model.clone());
        let mut session = Session::new(task.id, self.name(), task.role);
        session.model = model.clone();
        session.parent_session_ids = task.parent_session_ids.clone();
        session.worker_thread_id = task.worker_thread_id;
        session.native_session_id = task.native_session_id.clone();

        // Allocate a worktree
        let repo_root = task
            .worktree
            .as_deref()
            .and_then(WorktreePool::detect_repo_root_from)
            .or_else(WorktreePool::detect_repo_root);
        let worktree = if let Some(thread_id) = task.worker_thread_id {
            self.worktree_pool
                .acquire_thread(thread_id, repo_root.as_deref())?
        } else {
            self.worktree_pool.acquire(task.id, repo_root.as_deref())?
        };
        session.worktree_path = Some(worktree.clone());

        // Build context from parent sessions
        let conversational = is_conversational_task(task);
        let history = if conversational {
            String::new()
        } else {
            format_context(self.session_store.as_ref(), &task.parent_session_ids)
                .await
                .unwrap_or_default()
        };

        // Load prior CC sessions from ~/.claude/projects/<slug>/sessions/*.jsonl
        let cc_context = if conversational {
            String::new()
        } else {
            let cc_sessions = crate::cc_session_import::load_cc_sessions(&worktree);
            crate::cc_session_import::format_cc_sessions_for_context(&cc_sessions, 8000, 5, 20)
        };

        // Load orchestrator's own AGENTS.md (the platform's personality file)
        let agent_md = crate::agent_md::AgentMd::load();

        // Capture sizes BEFORE moving (for tracing)
        let agent_md_len = agent_md.content.len();
        let cc_config_len = self.cc_config.system_prompt.len();
        let history_len = history.len();
        let cc_context_len = cc_context.len();

        // Build the full system prompt in priority order:
        //   1. AGENTS.md (orchestrator's own instructions — highest)
        //   2. CLAUDE.md (user's CC instructions — inherited)
        //   3. Prior session context (orchestrator's history)
        //   4. Prior CC sessions (inherited history)
        let mut parts: Vec<&str> = vec![];
        if !agent_md.content.is_empty() {
            let s = format!(
                "# Orchestrator agent instructions (AGENTS.md)\n\n{}",
                agent_md.content
            );
            parts.push(Box::leak(s.into_boxed_str()));
        }
        if conversational {
            parts.push(
                "For this turn, answer only the user's conversational question. Do not inspect \
                 or mention repository state, git status, files, branches, tools, or local \
                 changes unless the user explicitly asks for that.",
            );
        }
        let cc_md_prompt = self.cc_config.build_system_prompt(&history);
        if !cc_md_prompt.is_empty() {
            parts.push(Box::leak(cc_md_prompt.into_boxed_str()));
        }
        if !cc_context.is_empty() {
            parts.push(Box::leak(cc_context.into_boxed_str()));
        }
        let full_system_prompt = parts.join("\n\n");

        // (note: we leak these strings because the Command::arg needs &str with
        // a lifetime tied to the spawned process. In practice this is a few KB
        // per worker spawn, and the OS reclaims on process exit.)

        let provider_for_model = task.model_provider_hint.as_deref().and_then(|provider_id| {
            self.providers
                .get(provider_id)
                .map(|provider| (provider_id, provider))
        });
        let setting_sources = claude_setting_sources_for_provider(provider_for_model);

        let mut cmd = Command::new(&self.binary);
        cmd.args(claude_worker_args(ClaudeWorkerArgs {
            setting_sources,
            model: &model,
            worktree: &worktree,
            cc_agent_hint: task.cc_agent_hint.as_deref(),
            bypass_permissions: self.bypass_permissions,
            native_session_id: session.native_session_id.as_deref(),
            system_prompt: (!full_system_prompt.is_empty()).then_some(full_system_prompt.as_str()),
            prompt: &task.prompt,
        }))
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        if self.use_agent_teams {
            cmd.env("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1");
        }

        if let Some((provider_id, provider)) = provider_for_model {
            apply_claude_provider_env(&mut cmd, provider_id, provider, &model);
        }

        // Log what we injected (helps debugging)
        tracing::debug!(
            "injected system prompt: {} chars (AGENTS.md: {}, CC config: {}, history: {}, CC prior: {})",
            full_system_prompt.len(),
            agent_md_len,
            cc_config_len,
            history_len,
            cc_context_len,
        );

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take();
        let stderr_capture = StderrCapture::default();
        let mut stderr_handle = None;

        // Background stderr drainer (non-blocking, just logs)
        if let Some(stderr) = stderr {
            let stderr_store = self.session_store.clone();
            let stderr_tx = event_tx.clone();
            let stderr_session_id = session.id;
            let stderr_task_id = task.id;
            let stderr_capture = stderr_capture.clone();
            stderr_handle = Some(tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::warn!(target = "claude-code", "{}", line);
                    stderr_capture.push(line.clone());
                    let event = Event {
                        session_id: stderr_session_id,
                        task_id: stderr_task_id,
                        ts: Utc::now(),
                        kind: "stderr".into(),
                        payload: serde_json::json!({ "line": line }),
                    };
                    let _ = stderr_store.append(&event);
                    if let Some(tx) = &stderr_tx {
                        let _ = tx.send(event);
                    }
                }
            }));
        }

        // Drain stdout to the normalized session event log.
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let payload = serde_json::from_str::<serde_json::Value>(&line)
                .unwrap_or_else(|_| serde_json::Value::String(line.clone()));
            if session.native_session_id.is_none() {
                session.native_session_id = extract_claude_native_session_id(&payload);
            }
            let event = Event {
                session_id: session.id,
                task_id: task.id,
                ts: Utc::now(),
                kind: agent_events::classify_json_line(&line),
                payload,
            };
            let _ = self.session_store.append(&event);
            if let Some(tx) = &event_tx {
                let _ = tx.send(event);
            }
        }

        let status = child.wait().await?;
        if let Some(handle) = stderr_handle {
            let _ = handle.await;
        }
        if !status.success() {
            let line = format!("claude exited with status {}", status);
            let event = Event {
                session_id: session.id,
                task_id: task.id,
                ts: Utc::now(),
                kind: "process_exit".into(),
                payload: serde_json::json!({
                    "line": line,
                    "success": false,
                }),
            };
            let _ = self.session_store.append(&event);
            if let Some(tx) = &event_tx {
                let _ = tx.send(event);
            }
            session.ended_at = Some(Utc::now());
            self.session_store
                .finalize(&session)
                .with_context(|| format!("finalizing failed claude session {}", session.id))?;
            anyhow::bail!(
                "claude exited with status {}{}",
                status,
                stderr_capture.error_suffix()
            );
        }

        Ok(crate::core::worker::WorkerHandle {
            session,
            kill: std::sync::Arc::new(|| {}),
        })
    }

    fn stream_events<'a>(&self, session: &Session) -> BoxStream<'static, Result<Event>> {
        stream_cli_session_events(
            self.session_store.log_path(session.id),
            session.id,
            session.task_id,
            agent_events::classify_json_line,
        )
    }

    async fn cancel(&self, _session: &Session) -> Result<()> {
        Ok(())
    }

    async fn get_diff(&self, session: &Session) -> Result<String> {
        if let Some(path) = session.worktree_path.as_deref() {
            return self.worktree_pool.diff_path(path);
        }
        if let Some(thread_id) = session.worker_thread_id {
            return self.worktree_pool.diff_thread(thread_id);
        }
        self.worktree_pool.diff(session.task_id)
    }

    async fn build_context(
        &self,
        reader: &dyn SessionReader,
        parent_ids: &[Uuid],
    ) -> Result<String> {
        format_context(reader, parent_ids).await
    }
}

struct ClaudeWorkerArgs<'a> {
    setting_sources: &'a str,
    model: &'a str,
    worktree: &'a std::path::Path,
    cc_agent_hint: Option<&'a str>,
    bypass_permissions: bool,
    native_session_id: Option<&'a str>,
    system_prompt: Option<&'a str>,
    prompt: &'a str,
}

fn claude_worker_args(input: ClaudeWorkerArgs<'_>) -> Vec<String> {
    let mut args = vec![
        "--print".to_string(),
        "--setting-sources".to_string(),
        input.setting_sources.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--model".to_string(),
        input.model.to_string(),
        "--add-dir".to_string(),
        input.worktree.display().to_string(),
        "--verbose".to_string(),
    ];
    if let Some(session_id) = input
        .native_session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
    {
        args.push("--resume".to_string());
        args.push(session_id.to_string());
    }
    if let Some(agent) = input
        .cc_agent_hint
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        args.push("--agent".to_string());
        args.push(agent.to_string());
    }
    if input.bypass_permissions {
        args.push("--permission-mode".to_string());
        args.push("bypassPermissions".to_string());
    }
    if let Some(system_prompt) = input
        .system_prompt
        .filter(|prompt| !prompt.trim().is_empty())
    {
        args.push("--append-system-prompt".to_string());
        args.push(system_prompt.to_string());
    }
    args.push(input.prompt.to_string());
    args
}

fn claude_setting_sources_for_provider(provider: Option<(&str, &ProviderConfig)>) -> &'static str {
    match provider {
        None => "user,project,local",
        Some((provider_id, provider))
            if claude_provider_uses_native_settings(provider_id, provider) =>
        {
            "user,project,local"
        }
        Some(_) => "project,local",
    }
}

fn claude_provider_uses_native_settings(_provider_id: &str, provider: &ProviderConfig) -> bool {
    provider.kind.eq_ignore_ascii_case("anthropic")
        && provider
            .api_key
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        && provider
            .base_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
}

fn extract_claude_native_session_id(value: &serde_json::Value) -> Option<String> {
    find_string_field(value, &["session_id", "sessionId"]).filter(|id| !id.trim().is_empty())
}

fn find_string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(text) = map.get(*key).and_then(|value| value.as_str()) {
                    return Some(text.to_string());
                }
            }
            for child in map.values() {
                if let Some(found) = find_string_field(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for child in items {
                if let Some(found) = find_string_field(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn apply_claude_provider_env(
    cmd: &mut Command,
    provider_id: &str,
    provider: &ProviderConfig,
    model: &str,
) {
    let base_url = claude_base_url_for_provider(provider_id, provider);
    if !provider.kind.eq_ignore_ascii_case("anthropic") && base_url.is_none() {
        return;
    }
    if let Some(api_key) = provider
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        cmd.env("ANTHROPIC_AUTH_TOKEN", api_key.trim());
        cmd.env("ANTHROPIC_API_KEY", api_key.trim());
    }
    if let Some(base_url) = base_url {
        cmd.env("ANTHROPIC_BASE_URL", base_url);
    }
    for key in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
    ] {
        cmd.env(key, model);
    }
}

fn claude_base_url_for_provider(provider_id: &str, provider: &ProviderConfig) -> Option<String> {
    let base_url = provider.base_url.as_deref()?.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return None;
    }
    if provider.kind.eq_ignore_ascii_case("anthropic") {
        return Some(base_url.to_string());
    }
    if provider_id.eq_ignore_ascii_case("minimax") {
        let root = base_url.strip_suffix("/v1").unwrap_or(base_url);
        return Some(format!("{}/anthropic", root.trim_end_matches('/')));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_worker_args_include_cc_subagent_when_set() {
        let args = claude_worker_args(ClaudeWorkerArgs {
            setting_sources: "project,local",
            model: "sonnet",
            worktree: std::path::Path::new("/tmp/project"),
            cc_agent_hint: Some("reviewer"),
            bypass_permissions: true,
            native_session_id: Some("123e4567-e89b-42d3-a456-426614174000"),
            system_prompt: Some("system"),
            prompt: "do the work",
        });

        assert!(args
            .windows(2)
            .any(|pair| pair == ["--output-format", "stream-json"]));
        assert!(
            args.iter().any(|arg| arg == "--verbose"),
            "Claude Code 2.1.193+ requires --verbose with stream-json output"
        );
        assert!(args.windows(2).any(|pair| pair == ["--agent", "reviewer"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "bypassPermissions"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--append-system-prompt", "system"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--resume", "123e4567-e89b-42d3-a456-426614174000"]));
        assert!(!args.iter().any(|arg| arg == "--session-id"));
        assert_eq!(args.last().map(String::as_str), Some("do the work"));
    }

    #[test]
    fn claude_worker_args_skip_empty_cc_subagent() {
        let args = claude_worker_args(ClaudeWorkerArgs {
            setting_sources: "project,local",
            model: "sonnet",
            worktree: std::path::Path::new("/tmp/project"),
            cc_agent_hint: Some("  "),
            bypass_permissions: false,
            native_session_id: Some("  "),
            system_prompt: None,
            prompt: "do the work",
        });

        assert!(!args.iter().any(|arg| arg == "--agent"));
        assert!(!args.iter().any(|arg| arg == "--permission-mode"));
        assert!(!args.iter().any(|arg| arg == "--append-system-prompt"));
        assert!(!args.iter().any(|arg| arg == "--session-id"));
        assert!(!args.iter().any(|arg| arg == "--resume"));
    }

    #[test]
    fn extracts_claude_native_session_id_from_stream_json() {
        let top_level = serde_json::json!({
            "type": "system",
            "session_id": "claude-session-1"
        });
        assert_eq!(
            extract_claude_native_session_id(&top_level).as_deref(),
            Some("claude-session-1")
        );

        let nested = serde_json::json!({
            "event": {
                "message": {
                    "sessionId": "claude-session-2"
                }
            }
        });
        assert_eq!(
            extract_claude_native_session_id(&nested).as_deref(),
            Some("claude-session-2")
        );

        let missing = serde_json::json!({"type": "assistant"});
        assert!(extract_claude_native_session_id(&missing).is_none());
    }

    #[test]
    fn claude_worker_args_use_requested_setting_sources() {
        let args = claude_worker_args(ClaudeWorkerArgs {
            setting_sources: "user,project,local",
            model: "sonnet",
            worktree: std::path::Path::new("/tmp/project"),
            cc_agent_hint: None,
            bypass_permissions: false,
            native_session_id: None,
            system_prompt: None,
            prompt: "hi",
        });

        assert!(args
            .windows(2)
            .any(|pair| { pair[0] == "--setting-sources" && pair[1] == "user,project,local" }));
    }

    #[test]
    fn claude_native_settings_are_used_when_provider_has_no_explicit_credentials() {
        let provider = ProviderConfig {
            kind: "anthropic".into(),
            api_key: None,
            base_url: None,
        };

        assert_eq!(
            claude_setting_sources_for_provider(Some(("real-anthropic", &provider))),
            "user,project,local"
        );
        assert_eq!(
            claude_setting_sources_for_provider(None),
            "user,project,local"
        );
    }

    #[test]
    fn claude_provider_isolation_is_used_for_explicit_provider_credentials() {
        let with_key = ProviderConfig {
            kind: "anthropic".into(),
            api_key: Some("sk-test".into()),
            base_url: None,
        };
        let with_base_url = ProviderConfig {
            kind: "anthropic".into(),
            api_key: None,
            base_url: Some("https://example.invalid".into()),
        };
        let compatible = ProviderConfig {
            kind: "openai".into(),
            api_key: Some("sk-test".into()),
            base_url: Some("https://api.minimaxi.com/v1".into()),
        };

        assert_eq!(
            claude_setting_sources_for_provider(Some(("anthropic", &with_key))),
            "project,local"
        );
        assert_eq!(
            claude_setting_sources_for_provider(Some(("anthropic", &with_base_url))),
            "project,local"
        );
        assert_eq!(
            claude_setting_sources_for_provider(Some(("minimax", &compatible))),
            "project,local"
        );
    }
}
