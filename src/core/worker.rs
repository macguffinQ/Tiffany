//! The WorkerAdapter trait — the swap-point for any coding agent.

use crate::core::session_store::SessionReader;
use crate::core::types::{Event, Session, Task};
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

/// A handle to a running worker. Caller can call .kill() to abort the
/// subprocess immediately (no more waiting for natural exit or 60s timeout).
pub struct WorkerHandle {
    pub session: Session,
    pub kill: KillFn,
}

/// Type for the kill closure that aborts a worker mid-flight.
pub type KillFn = Arc<dyn Fn() + Send + Sync>;

#[async_trait]
pub trait WorkerAdapter: Send + Sync {
    /// Stable identifier (e.g. "claude-code", "codex", "direct").
    fn name(&self) -> &str;

    /// Start a worker for `task`. Returns a WorkerHandle (session + kill fn).
    /// The actual work happens in the background; events stream via `stream_events`.
    /// The caller can .kill() to abort the subprocess immediately.
    async fn start(
        &self,
        task: &Task,
        event_tx: Option<UnboundedSender<Event>>,
    ) -> anyhow::Result<WorkerHandle>;

    /// Stream events for a running session.
    fn stream_events(&self, session: &Session) -> BoxStream<'static, anyhow::Result<Event>>;

    /// Cancel a running worker.
    async fn cancel(&self, session: &Session) -> anyhow::Result<()>;

    /// Return a unified diff of changes this worker made.
    async fn get_diff(&self, session: &Session) -> anyhow::Result<String>;

    /// Build a context string from parent sessions, for injection into the
    /// new worker's prompt. This is the "shared session log" feature.
    async fn build_context(
        &self,
        reader: &dyn SessionReader,
        parent_ids: &[Uuid],
    ) -> anyhow::Result<String>;
}

/// Format parent sessions into an injectable context block.
pub async fn format_context(
    reader: &dyn SessionReader,
    parent_ids: &[Uuid],
) -> anyhow::Result<String> {
    use std::fmt::Write as _;
    if parent_ids.is_empty() {
        return Ok(String::new());
    }
    let sessions = reader.get_many(parent_ids).await?;
    let mut out = String::new();
    out.push_str("# Prior session context\n\n");
    for s in &sessions {
        let _ = writeln!(
            out,
            "## Session {} ({} / {}, started {})\n",
            s.id,
            s.agent,
            s.role.as_str(),
            s.started_at.to_rfc3339(),
        );
        if let Some(ended) = s.ended_at {
            let _ = writeln!(out, "ended: {}\n", ended.to_rfc3339());
        }
        if !s.files_touched.is_empty() {
            let _ = writeln!(out, "files touched: {}\n", s.files_touched.join(", "));
        }
        out.push_str("\n");
    }
    Ok(out)
}
