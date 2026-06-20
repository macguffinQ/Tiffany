//! Claude Code adapter — invokes `claude` CLI as a subprocess.

use crate::agent_events;
use crate::cc_config::CCConfig;
use crate::config::ProviderConfig;
use crate::core::session_store::{SessionReader, SessionStore};
use crate::core::types::{Event, Session, Task};
use crate::core::worker::{format_context, WorkerAdapter};
use crate::storage::worktree::WorktreePool;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

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

        // Allocate a worktree
        let repo_root = task
            .worktree
            .as_deref()
            .and_then(WorktreePool::detect_repo_root_from)
            .or_else(WorktreePool::detect_repo_root);
        let worktree = self.worktree_pool.acquire(task.id, repo_root.as_deref())?;

        // Build context from parent sessions
        let history = format_context(self.session_store.as_ref(), &task.parent_session_ids)
            .await
            .unwrap_or_default();

        // Load prior CC sessions from ~/.claude/projects/<slug>/sessions/*.jsonl
        let cc_sessions = crate::cc_session_import::load_cc_sessions(&worktree);
        let cc_context =
            crate::cc_session_import::format_cc_sessions_for_context(&cc_sessions, 8000, 5, 20);

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

        let mut cmd = Command::new(&self.binary);
        cmd.args(claude_worker_args(
            &model,
            &worktree,
            task.cc_agent_hint.as_deref(),
            self.bypass_permissions,
            (!full_system_prompt.is_empty()).then_some(full_system_prompt.as_str()),
            &task.prompt,
        ))
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        if self.use_agent_teams {
            cmd.env("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1");
        }

        if let Some(provider_id) = task.model_provider_hint.as_deref() {
            if let Some(provider) = self.providers.get(provider_id) {
                apply_claude_provider_env(&mut cmd, provider_id, provider, &model);
            }
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

        // Background stderr drainer (non-blocking, just logs)
        if let Some(stderr) = stderr {
            let stderr_store = self.session_store.clone();
            let stderr_tx = event_tx.clone();
            let stderr_session_id = session.id;
            let stderr_task_id = task.id;
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::warn!(target = "claude-code", "{}", line);
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
            });
        }

        // Drain stdout to the normalized session event log.
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let event = Event {
                session_id: session.id,
                task_id: task.id,
                ts: Utc::now(),
                kind: agent_events::classify_json_line(&line),
                payload: serde_json::from_str(&line)
                    .unwrap_or(serde_json::Value::String(line.clone())),
            };
            let _ = self.session_store.append(&event);
            if let Some(tx) = &event_tx {
                let _ = tx.send(event);
            }
        }

        let status = child.wait().await?;
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
            anyhow::bail!("claude exited with status {}", status);
        }

        Ok(crate::core::worker::WorkerHandle {
            session,
            kill: std::sync::Arc::new(|| {}),
        })
    }

    fn stream_events<'a>(&self, session: &Session) -> BoxStream<'static, Result<Event>> {
        let path: PathBuf = self.session_store.log_path(session.id);
        let sess_id = session.id;
        let task_id = session.task_id;
        Box::pin(stream::unfold(
            (path, 0u64, false),
            move |(path, mut pos, done)| async move {
                use tokio::io::AsyncSeekExt;
                if done {
                    return None;
                }
                // Open and seek to pos
                let mut file = match tokio::fs::File::open(&path).await {
                    Ok(f) => f,
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        return Some((
                            Ok(Event {
                                session_id: sess_id,
                                task_id,
                                ts: Utc::now(),
                                kind: "heartbeat".into(),
                                payload: serde_json::json!({}),
                            }),
                            (path, pos, done),
                        ));
                    }
                };
                if file.seek(std::io::SeekFrom::Start(pos)).await.is_err() {
                    return None;
                }
                let mut buf = String::new();
                use tokio::io::AsyncReadExt;
                if file.read_to_string(&mut buf).await.is_err() {
                    return None;
                }
                if buf.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    return Some((
                        Ok(Event {
                            session_id: sess_id,
                            task_id,
                            ts: Utc::now(),
                            kind: "heartbeat".into(),
                            payload: serde_json::json!({}),
                        }),
                        (path, pos, done),
                    ));
                }
                pos += buf.len() as u64;
                let line = buf.trim_end().lines().last().unwrap_or("").to_string();
                let event = Event {
                    session_id: sess_id,
                    task_id,
                    ts: Utc::now(),
                    kind: agent_events::classify_json_line(&line),
                    payload: serde_json::from_str(&line).unwrap_or(serde_json::Value::String(line)),
                };
                Some((Ok(event), (path, pos, done)))
            },
        ))
    }

    async fn cancel(&self, _session: &Session) -> Result<()> {
        Ok(())
    }

    async fn get_diff(&self, session: &Session) -> Result<String> {
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

fn claude_worker_args(
    model: &str,
    worktree: &std::path::Path,
    cc_agent_hint: Option<&str>,
    bypass_permissions: bool,
    system_prompt: Option<&str>,
    prompt: &str,
) -> Vec<String> {
    let mut args = vec![
        "--print".to_string(),
        "--setting-sources".to_string(),
        "project,local".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--model".to_string(),
        model.to_string(),
        "--add-dir".to_string(),
        worktree.display().to_string(),
        "--verbose".to_string(),
    ];
    if let Some(agent) = cc_agent_hint
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        args.push("--agent".to_string());
        args.push(agent.to_string());
    }
    if bypass_permissions {
        args.push("--permission-mode".to_string());
        args.push("bypassPermissions".to_string());
    }
    if let Some(system_prompt) = system_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        args.push("--append-system-prompt".to_string());
        args.push(system_prompt.to_string());
    }
    args.push(prompt.to_string());
    args
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
        let args = claude_worker_args(
            "sonnet",
            std::path::Path::new("/tmp/project"),
            Some("reviewer"),
            true,
            Some("system"),
            "do the work",
        );

        assert!(args.windows(2).any(|pair| pair == ["--agent", "reviewer"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "bypassPermissions"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--append-system-prompt", "system"]));
        assert_eq!(args.last().map(String::as_str), Some("do the work"));
    }

    #[test]
    fn claude_worker_args_skip_empty_cc_subagent() {
        let args = claude_worker_args(
            "sonnet",
            std::path::Path::new("/tmp/project"),
            Some("  "),
            false,
            None,
            "do the work",
        );

        assert!(!args.iter().any(|arg| arg == "--agent"));
        assert!(!args.iter().any(|arg| arg == "--permission-mode"));
        assert!(!args.iter().any(|arg| arg == "--append-system-prompt"));
    }
}
