//! Codex CLI adapter — invokes `codex` CLI as a subprocess.

use crate::agent_events;
use crate::config::ProviderConfig;
use crate::core::session_store::{SessionReader, SessionStore};
use crate::core::types::{Event, Session, Task};
use crate::core::worker::{format_context, WorkerAdapter};
use crate::storage::worktree::WorktreePool;
use crate::task_policy::is_conversational_task;
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

pub struct CodexCLIAdapter {
    binary: String,
    model: String,
    worktree_pool: Arc<WorktreePool>,
    session_store: Arc<SessionStore>,
    providers: Arc<HashMap<String, ProviderConfig>>,
}

impl CodexCLIAdapter {
    pub fn new(
        binary: String,
        model: String,
        worktree_pool: Arc<WorktreePool>,
        session_store: Arc<SessionStore>,
        providers: Arc<HashMap<String, ProviderConfig>>,
    ) -> Self {
        Self {
            binary,
            model,
            worktree_pool,
            session_store,
            providers,
        }
    }
}

#[async_trait]
impl WorkerAdapter for CodexCLIAdapter {
    fn name(&self) -> &str {
        "codex"
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

        let repo_root = task
            .worktree
            .as_deref()
            .and_then(WorktreePool::detect_repo_root_from)
            .or_else(WorktreePool::detect_repo_root);
        let worktree = self.worktree_pool.acquire(task.id, repo_root.as_deref())?;

        let conversational = is_conversational_task(task);
        let history = if conversational {
            String::new()
        } else {
            format_context(self.session_store.as_ref(), &task.parent_session_ids)
                .await
                .unwrap_or_default()
        };

        let full_prompt = if history.is_empty() {
            task.prompt.clone()
        } else {
            format!("{}\n\n---\nPrior context:\n{}", task.prompt, history)
        };

        // codex CLI usage: `codex exec --json --model <m> --cwd <d> <prompt>`
        // (Reads the OpenAI-style base_url + key from ~/.codex/config.toml)
        let mut cmd = Command::new(&self.binary);
        cmd.arg("exec")
            .arg("--json")
            .arg("--model")
            .arg(&model)
            .arg("--cwd")
            .arg(&worktree);

        if let Some(provider_id) = task.model_provider_hint.as_deref() {
            if let Some(provider) = self.providers.get(provider_id) {
                apply_codex_provider_config(&mut cmd, provider_id, provider);
            }
        }

        cmd.arg(&full_prompt)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Tell codex not to require git repo (we're in a worktree)
        cmd.env("CODEX_UNSAFE_ALLOW_NO_GIT", "1");

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take();

        if let Some(stderr) = stderr {
            let stderr_store = self.session_store.clone();
            let stderr_tx = event_tx.clone();
            let stderr_session_id = session.id;
            let stderr_task_id = task.id;
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::warn!(target = "codex", "{}", line);
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
            let line = format!("codex exited with status {}", status);
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
                .with_context(|| format!("finalizing failed codex session {}", session.id))?;
            anyhow::bail!("codex exited with status {}", status);
        }

        Ok(crate::core::worker::WorkerHandle {
            session,
            kill: std::sync::Arc::new(|| {}),
        })
    }

    fn stream_events<'a>(&self, session: &Session) -> BoxStream<'static, Result<Event>> {
        // Same tail-and-emit-heartbeat pattern as CC adapter.
        let path: PathBuf = self.session_store.log_path(session.id);
        let sess_id = session.id;
        let task_id = session.task_id;
        Box::pin(stream::unfold(
            (path, 0u64),
            move |(path, mut pos)| async move {
                use tokio::io::{AsyncReadExt, AsyncSeekExt};
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
                            (path, pos),
                        ));
                    }
                };
                if file.seek(std::io::SeekFrom::Start(pos)).await.is_err() {
                    return None;
                }
                let mut buf = String::new();
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
                        (path, pos),
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
                Some((Ok(event), (path, pos)))
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

fn apply_codex_provider_config(cmd: &mut Command, provider_id: &str, provider: &ProviderConfig) {
    let kind = provider.kind.to_ascii_lowercase();
    if kind != "openai" && kind != "ollama" {
        return;
    }
    cmd.arg("-c").arg(format!(
        "model_provider={}",
        toml_string_literal(provider_id)
    ));
    if provider_id == "openai" {
        if let Some(base_url) = provider
            .base_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            cmd.arg("-c").arg(format!(
                "openai_base_url={}",
                toml_string_literal(base_url.trim())
            ));
        }
        if let Some(api_key) = provider
            .api_key
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            cmd.env("OPENAI_API_KEY", api_key.trim());
        }
        return;
    }

    let prefix = format!("model_providers.{provider_id}");
    cmd.arg("-c")
        .arg(format!(
            "{prefix}.name={}",
            toml_string_literal(provider_id)
        ))
        .arg("-c")
        .arg(format!(
            "{prefix}.wire_api={}",
            toml_string_literal("responses")
        ));
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        cmd.arg("-c").arg(format!(
            "{prefix}.base_url={}",
            toml_string_literal(base_url.trim())
        ));
    }
    if let Some(api_key) = provider
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let env_key = codex_provider_env_key(provider_id);
        cmd.env(&env_key, api_key.trim());
        cmd.arg("-c").arg(format!(
            "{prefix}.env_key={}",
            toml_string_literal(&env_key)
        ));
    }
}

fn codex_provider_env_key(provider_id: &str) -> String {
    let normalized = provider_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("TIFFANY_{normalized}_API_KEY")
}

fn toml_string_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}
