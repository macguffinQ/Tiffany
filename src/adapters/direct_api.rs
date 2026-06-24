//! Direct-API adapter: uses a ModelProvider to call LLMs directly,
//! for roles that don't need a CLI (planner/critic/reviewer with no Agent Teams).

use crate::core::provider::{ChatMessage, ChatRequest, ModelProvider};
use crate::core::session_store::SessionStore;
use crate::core::types::{Event, Role, Session, Task};
use crate::core::worker::{format_context, WorkerAdapter};
use crate::storage::worktree::WorktreePool;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

pub struct DirectAPIAdapter {
    provider: Arc<dyn ModelProvider>,
    model: String,
    role: Role,
    worktree_pool: Arc<WorktreePool>,
    session_store: Arc<SessionStore>,
}

impl DirectAPIAdapter {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        model: String,
        role: Role,
        worktree_pool: Arc<WorktreePool>,
        session_store: Arc<SessionStore>,
    ) -> Self {
        Self {
            provider,
            model,
            role,
            worktree_pool,
            session_store,
        }
    }
}

#[async_trait]
impl WorkerAdapter for DirectAPIAdapter {
    fn name(&self) -> &str {
        "direct-api"
    }

    async fn start(
        &self,
        task: &Task,
        event_tx: Option<UnboundedSender<Event>>,
    ) -> Result<crate::core::worker::WorkerHandle> {
        let mut session = Session::new(task.id, self.name(), task.role);
        session.model = self.model.clone();
        session.parent_session_ids = task.parent_session_ids.clone();
        session.worker_thread_id = task.worker_thread_id;
        session.native_session_id = task.native_session_id.clone();

        // Allocate worktree (even though direct API may not write to it,
        // we keep the contract consistent).
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

        let history = format_context(self.session_store.as_ref(), &task.parent_session_ids)
            .await
            .unwrap_or_default();

        let system = format!(
            "You are the {} role in a multi-agent orchestrator. Working directory: {}\n{}",
            self.role.as_str(),
            worktree.display(),
            history
        );
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: system,
                },
                ChatMessage {
                    role: "user".into(),
                    content: task.prompt.clone(),
                },
            ],
            max_tokens: Some(2048),
            temperature: Some(0.2),
        };

        let sess_id = session.id;
        let task_id = task.id;
        let event = Event {
            session_id: sess_id,
            task_id,
            ts: Utc::now(),
            kind: "llm_request".into(),
            payload: serde_json::to_value(&req).unwrap_or_default(),
        };
        let _ = self.session_store.append(&event);
        if let Some(tx) = &event_tx {
            let _ = tx.send(event);
        }

        match self.provider.chat(req).await {
            Ok(resp) => {
                session.token_in = resp.input_tokens;
                session.token_out = resp.output_tokens;
                session.cost_usd = resp.cost_usd;
                let event = Event {
                    session_id: sess_id,
                    task_id,
                    ts: Utc::now(),
                    kind: "llm_response".into(),
                    payload: serde_json::to_value(&resp).unwrap_or_default(),
                };
                let _ = self.session_store.append(&event);
                if let Some(tx) = &event_tx {
                    let _ = tx.send(event);
                }
            }
            Err(e) => {
                let event = Event {
                    session_id: sess_id,
                    task_id,
                    ts: Utc::now(),
                    kind: "llm_error".into(),
                    payload: serde_json::json!({ "error": e.to_string() }),
                };
                let _ = self.session_store.append(&event);
                if let Some(tx) = &event_tx {
                    let _ = tx.send(event);
                }
                return Err(e);
            }
        }

        Ok(crate::core::worker::WorkerHandle {
            session,
            kill: std::sync::Arc::new(|| {}),
        })
    }

    fn stream_events<'a>(&self, session: &Session) -> BoxStream<'static, Result<Event>> {
        let path = self.session_store.log_path(session.id);
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
                    kind: "llm".into(),
                    payload: serde_json::from_str(&line).unwrap_or(serde_json::json!({})),
                };
                Some((Ok(event), (path, pos)))
            },
        ))
    }

    async fn cancel(&self, _session: &Session) -> Result<()> {
        Ok(())
    }

    async fn get_diff(&self, _session: &Session) -> Result<String> {
        Ok(String::new())
    }

    async fn build_context(
        &self,
        reader: &dyn crate::core::session_store::SessionReader,
        parent_ids: &[uuid::Uuid],
    ) -> Result<String> {
        format_context(reader, parent_ids).await
    }
}
