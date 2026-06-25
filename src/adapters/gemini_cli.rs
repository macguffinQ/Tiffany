//! Gemini CLI adapter — invokes `gemini` CLI as a subprocess.

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

use super::StderrCapture;

pub struct GeminiCLIAdapter {
    binary: String,
    model: String,
    worktree_pool: Arc<WorktreePool>,
    session_store: Arc<SessionStore>,
    providers: Arc<HashMap<String, ProviderConfig>>,
}

impl GeminiCLIAdapter {
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
impl WorkerAdapter for GeminiCLIAdapter {
    fn name(&self) -> &str {
        "gemini"
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
        session.native_session_id = None;

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

        let mut cmd = gemini_command(&self.binary, &model, &worktree, &full_prompt);
        if let Some(provider_id) = task.model_provider_hint.as_deref() {
            if let Some(provider) = self.providers.get(provider_id) {
                apply_gemini_provider_env(&mut cmd, provider);
            }
        }
        cmd.kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take();
        let stderr_capture = StderrCapture::default();
        let mut stderr_handle = None;

        if let Some(stderr) = stderr {
            let stderr_store = self.session_store.clone();
            let stderr_tx = event_tx.clone();
            let stderr_session_id = session.id;
            let stderr_task_id = task.id;
            let stderr_capture = stderr_capture.clone();
            stderr_handle = Some(tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::warn!(target = "gemini", "{}", line);
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

        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let payload = serde_json::from_str::<serde_json::Value>(&line)
                .unwrap_or_else(|_| serde_json::Value::String(line.clone()));
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
            let line = format!("gemini exited with status {}", status);
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
                .with_context(|| format!("finalizing failed gemini session {}", session.id))?;
            anyhow::bail!(
                "gemini exited with status {}{}",
                status,
                stderr_capture.error_suffix()
            );
        }

        Ok(crate::core::worker::WorkerHandle {
            session,
            kill: std::sync::Arc::new(|| {}),
        })
    }

    fn stream_events(&self, session: &Session) -> BoxStream<'static, Result<Event>> {
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

fn gemini_command(binary: &str, model: &str, worktree: &std::path::Path, prompt: &str) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(worktree)
        .arg("--model")
        .arg(model)
        .arg("--output-format")
        .arg("stream-json")
        .arg(prompt);
    cmd
}

fn apply_gemini_provider_env(cmd: &mut Command, provider: &ProviderConfig) {
    if !provider.kind.eq_ignore_ascii_case("google") {
        return;
    }
    if let Some(api_key) = provider
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        cmd.env("GEMINI_API_KEY", api_key.trim());
        cmd.env("GOOGLE_API_KEY", api_key.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_command_uses_current_dir_model_and_stream_json() {
        let cmd = gemini_command(
            "gemini",
            "gemini-2.5-pro",
            std::path::Path::new("/tmp/repo"),
            "do the work",
        );
        let args = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--model" && pair[1] == "gemini-2.5-pro"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--output-format" && pair[1] == "stream-json"));
        assert_eq!(args.last().map(String::as_str), Some("do the work"));
        assert!(!args.iter().any(|arg| arg == "--resume"));
        assert_eq!(
            cmd.as_std().get_current_dir(),
            Some(std::path::Path::new("/tmp/repo"))
        );
    }
}
