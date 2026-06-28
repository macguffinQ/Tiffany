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
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use super::{stream_cli_session_events, StderrCapture};

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

        let mut cmd = gemini_command(
            &self.binary,
            &model,
            &worktree,
            session.native_session_id.as_deref(),
            &full_prompt,
        );
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
            if session.native_session_id.is_none() {
                session.native_session_id = extract_gemini_native_session_id(&payload);
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

fn gemini_command(
    binary: &str,
    model: &str,
    worktree: &std::path::Path,
    native_session_id: Option<&str>,
    prompt: &str,
) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(worktree)
        .arg("--model")
        .arg(model)
        .arg("--output-format")
        .arg("stream-json");
    if let Some(session_id) = native_session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
    {
        cmd.arg("--resume").arg(session_id);
    }
    cmd.arg(prompt);
    cmd
}

fn extract_gemini_native_session_id(value: &serde_json::Value) -> Option<String> {
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
            None,
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

    #[test]
    fn gemini_command_resumes_saved_native_session_handle() {
        let cmd = gemini_command(
            "gemini",
            "gemini-2.5-pro",
            std::path::Path::new("/tmp/repo"),
            Some("d2f88206-a6ed-4612-bc7e-ac77f3fae1ab"),
            "continue the work",
        );
        let args = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--resume"
                    && pair[1] == "d2f88206-a6ed-4612-bc7e-ac77f3fae1ab")
        );
        assert_eq!(args.last().map(String::as_str), Some("continue the work"));
    }

    #[test]
    fn extracts_gemini_native_session_id_from_stream_init() {
        let top_level = serde_json::json!({
            "type": "init",
            "session_id": "d2f88206-a6ed-4612-bc7e-ac77f3fae1ab",
            "model": "gemini-2.5-pro"
        });
        assert_eq!(
            extract_gemini_native_session_id(&top_level).as_deref(),
            Some("d2f88206-a6ed-4612-bc7e-ac77f3fae1ab")
        );

        let nested = serde_json::json!({
            "session": {
                "sessionId": "nested-gemini-session"
            }
        });
        assert_eq!(
            extract_gemini_native_session_id(&nested).as_deref(),
            Some("nested-gemini-session")
        );
    }
}
