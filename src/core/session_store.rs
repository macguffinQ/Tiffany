//! Persistent session log: JSONL files + SQLite index.

use crate::core::types::{Event, Role, Session};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Thread-safe wrapper over the SQLite connection.
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<SessionStoreInner>,
}

struct SessionStoreInner {
    log_dir: std::path::PathBuf,
    db: Mutex<Connection>,
}

#[async_trait::async_trait]
pub trait SessionReader: Send + Sync {
    async fn get_many(&self, ids: &[Uuid]) -> Result<Vec<Session>>;
    async fn list(&self, limit: u32) -> Result<Vec<Session>>;
}

#[derive(Debug, Clone)]
pub struct WorkerThread {
    pub id: Uuid,
    pub scope: String,
    pub role: String,
    pub runtime: String,
    pub agent: String,
    pub model: String,
    pub provider: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub native_session_id: Option<String>,
    pub last_session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeConversation {
    pub id: String,
    pub cwd: String,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub turns: Vec<NativeTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeTurn {
    pub turn_index: u32,
    pub user_prompt: String,
    pub result: String,
    pub captured_at_unix: u64,
    pub events: Vec<NativeEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeEvent {
    pub event_index: u32,
    pub role: String,
    pub status: String,
    pub title: String,
    pub kind: Option<String>,
    pub content: Option<String>,
    pub agent: Option<String>,
    pub worker_role: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub task_id: Option<String>,
    pub worker_thread_id: Option<String>,
    pub native_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeImportReport {
    pub conversations: usize,
    pub turns: usize,
    pub events: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TuiJob {
    pub id: Uuid,
    pub prompt: String,
    pub status: String,
    pub route: Option<String>,
    pub role: Option<String>,
    pub task_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub worker_thread_id: Option<Uuid>,
    pub native_session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Cross-process guard for one worker thread.
///
/// The terminal UI starts a fresh orchestrator subprocess for each prompt. An
/// in-memory mutex only serializes work inside one subprocess, but Claude Code
/// rejects concurrent `--resume <session>` users with "Session ID ... is already
/// in use". This guard keeps a role's native CLI session single-owner across
/// subprocesses while preserving stable reuse after the previous run exits.
pub struct WorkerThreadLease {
    _file: File,
    _thread_id: Uuid,
}

impl SessionStore {
    pub fn open(log_dir: &Path, db_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(log_dir)
            .with_context(|| format!("creating {}", log_dir.display()))?;
        let db =
            Connection::open(db_path).with_context(|| format!("opening {}", db_path.display()))?;
        db.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                agent TEXT NOT NULL,
                role TEXT NOT NULL,
                model TEXT NOT NULL DEFAULT '',
                started_at TEXT NOT NULL,
                ended_at TEXT,
                parent_session_ids TEXT NOT NULL DEFAULT '[]',
                worker_thread_id TEXT,
                native_session_id TEXT,
                worktree_path TEXT,
                token_in INTEGER NOT NULL DEFAULT 0,
                token_out INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL NOT NULL DEFAULT 0,
                files_touched TEXT NOT NULL DEFAULT '[]'
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at DESC);
            CREATE TABLE IF NOT EXISTS events (
                session_id TEXT NOT NULL,
                ts TEXT NOT NULL,
                type TEXT NOT NULL,
                payload TEXT NOT NULL,
                PRIMARY KEY (session_id, ts)
            );
            CREATE TABLE IF NOT EXISTS worker_threads (
                id TEXT PRIMARY KEY,
                scope TEXT NOT NULL DEFAULT 'global',
                role TEXT NOT NULL,
                runtime TEXT NOT NULL,
                agent TEXT NOT NULL,
                model TEXT NOT NULL,
                provider TEXT,
                worktree_path TEXT,
                native_session_id TEXT,
                last_session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(scope, role)
            );
            CREATE TABLE IF NOT EXISTS native_conversations (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL UNIQUE,
                created_at_unix INTEGER NOT NULL,
                updated_at_unix INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS native_turns (
                conversation_id TEXT NOT NULL,
                turn_index INTEGER NOT NULL,
                user_prompt TEXT NOT NULL,
                result TEXT NOT NULL,
                captured_at_unix INTEGER NOT NULL,
                PRIMARY KEY (conversation_id, turn_index),
                FOREIGN KEY (conversation_id) REFERENCES native_conversations(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS native_events (
                conversation_id TEXT NOT NULL,
                turn_index INTEGER NOT NULL,
                event_index INTEGER NOT NULL,
                role TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                kind TEXT,
                content TEXT,
                agent TEXT,
                worker_role TEXT,
                model TEXT,
                provider TEXT,
                task_id TEXT,
                worker_thread_id TEXT,
                native_session_id TEXT,
                PRIMARY KEY (conversation_id, turn_index, event_index),
                FOREIGN KEY (conversation_id, turn_index) REFERENCES native_turns(conversation_id, turn_index) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_native_turns_captured ON native_turns(captured_at_unix DESC);
            CREATE INDEX IF NOT EXISTS idx_native_events_worker_role ON native_events(worker_role);
            CREATE INDEX IF NOT EXISTS idx_native_events_worker_thread ON native_events(worker_thread_id);
            CREATE TABLE IF NOT EXISTS tui_jobs (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                status TEXT NOT NULL,
                route TEXT,
                role TEXT,
                task_id TEXT,
                session_id TEXT,
                worker_thread_id TEXT,
                native_session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                started_at TEXT,
                ended_at TEXT,
                result TEXT,
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_tui_jobs_status_updated ON tui_jobs(status, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_tui_jobs_updated ON tui_jobs(updated_at DESC);
            "#,
        )?;
        migrate_worker_threads_table(&db)?;
        migrate_sessions_table(&db)?;
        migrate_native_events_table(&db)?;
        migrate_tui_jobs_table(&db)?;
        Ok(Self {
            inner: Arc::new(SessionStoreInner {
                log_dir: log_dir.to_path_buf(),
                db: Mutex::new(db),
            }),
        })
    }

    pub fn log_dir(&self) -> &Path {
        &self.inner.log_dir
    }

    pub fn tui_state_dir(&self) -> std::path::PathBuf {
        self.inner.log_dir.join("tui")
    }

    pub fn log_path(&self, session_id: Uuid) -> std::path::PathBuf {
        self.inner.log_dir.join(format!("{}.jsonl", session_id))
    }

    /// Append an event to the session's JSONL log and the index.
    pub fn append(&self, event: &Event) -> Result<()> {
        use std::io::Write;
        let path = self.log_path(event.session_id);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let line = serde_json::to_string(event)?;
        writeln!(f, "{}", line)?;

        let db = self.inner.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO events (session_id, ts, type, payload) VALUES (?,?,?,?)",
            params![
                event.session_id.to_string(),
                event.ts.to_rfc3339(),
                event.kind,
                event.payload.to_string(),
            ],
        )?;
        Ok(())
    }

    /// Finalize a session (writes its metadata to the index).
    pub fn finalize(&self, session: &Session) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        db.execute(
            r#"INSERT OR REPLACE INTO sessions
            (id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, worker_thread_id, native_session_id, worktree_path, token_in, token_out, cost_usd, files_touched)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
            params![
                session.id.to_string(),
                session.task_id.to_string(),
                session.agent,
                session.role.as_str(),
                session.model,
                session.started_at.to_rfc3339(),
                session.ended_at.map(|t| t.to_rfc3339()),
                serde_json::to_string(
                    &session
                        .parent_session_ids
                        .iter()
                        .map(|u| u.to_string())
                        .collect::<Vec<_>>()
                )?,
                session.worker_thread_id.map(|id| id.to_string()),
                session.native_session_id.as_deref(),
                session
                    .worktree_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                session.token_in as i64,
                session.token_out as i64,
                session.cost_usd,
                serde_json::to_string(&session.files_touched)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_or_create_worker_thread(
        &self,
        role: &str,
        runtime: &str,
        agent: &str,
        model: &str,
        provider: Option<&str>,
    ) -> Result<WorkerThread> {
        let scope = current_worker_thread_scope();
        self.get_or_create_worker_thread_scoped(&scope, role, runtime, agent, model, provider)
    }

    pub fn get_or_create_worker_thread_scoped(
        &self,
        scope: &str,
        role: &str,
        runtime: &str,
        agent: &str,
        model: &str,
        provider: Option<&str>,
    ) -> Result<WorkerThread> {
        let db = self.inner.db.lock().unwrap();
        let scope = normalize_worker_thread_scope(scope);
        let existing = select_worker_thread_by_role(&db, &scope, role)?;
        let now = Utc::now();
        if let Some(mut thread) = existing {
            let provider_changed = thread.provider.as_deref() != provider;
            let runtime_changed = thread.runtime != runtime;
            let agent_changed = thread.agent != agent;
            let model_changed = thread.model != model;
            if runtime_changed || provider_changed || agent_changed || model_changed {
                thread.runtime = runtime.to_string();
                thread.agent = agent.to_string();
                thread.model = model.to_string();
                thread.provider = provider.map(str::to_string);
                thread.updated_at = now;
                if runtime_changed || provider_changed || agent_changed || model_changed {
                    thread.native_session_id = None;
                }
                update_worker_thread_row(&db, &thread)?;
            }
            return Ok(thread);
        }

        let thread = WorkerThread {
            id: Uuid::new_v4(),
            scope,
            role: role.to_string(),
            runtime: runtime.to_string(),
            agent: agent.to_string(),
            model: model.to_string(),
            provider: provider.map(str::to_string),
            worktree_path: None,
            native_session_id: None,
            last_session_id: None,
            created_at: now,
            updated_at: now,
        };
        db.execute(
            r#"INSERT INTO worker_threads
            (id, scope, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"#,
            params![
                thread.id.to_string(),
                thread.scope.as_str(),
                thread.role.as_str(),
                thread.runtime.as_str(),
                thread.agent.as_str(),
                thread.model.as_str(),
                thread.provider.as_deref(),
                thread
                    .worktree_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                thread.native_session_id.as_deref(),
                thread.last_session_id.map(|id| id.to_string()),
                thread.created_at.to_rfc3339(),
                thread.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(thread)
    }

    pub fn update_worker_thread_after_session(
        &self,
        thread_id: Uuid,
        native_session_id: Option<&str>,
        last_session_id: Uuid,
        worktree_path: Option<&Path>,
    ) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        let mut thread = select_worker_thread_by_id(&db, thread_id)?
            .with_context(|| format!("worker thread not found: {thread_id}"))?;
        if let Some(native_session_id) = native_session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            thread.native_session_id = Some(native_session_id.to_string());
        }
        thread.last_session_id = Some(last_session_id);
        if let Some(path) = worktree_path {
            thread.worktree_path = Some(path.to_path_buf());
        }
        thread.updated_at = Utc::now();
        update_worker_thread_row(&db, &thread)?;
        Ok(())
    }

    pub fn worker_thread_by_role(&self, role: &str) -> Result<Option<WorkerThread>> {
        let scope = current_worker_thread_scope();
        self.worker_thread_by_role_scoped(&scope, role)
    }

    pub fn worker_thread_by_role_scoped(
        &self,
        scope: &str,
        role: &str,
    ) -> Result<Option<WorkerThread>> {
        let db = self.inner.db.lock().unwrap();
        select_worker_thread_by_role(&db, &normalize_worker_thread_scope(scope), role)
    }

    pub fn worker_threads_in_current_scope_family(&self) -> Result<Vec<WorkerThread>> {
        let scope = current_worker_thread_scope();
        self.worker_threads_in_scope_family(&scope)
    }

    pub fn worker_threads_in_scope_family(&self, scope: &str) -> Result<Vec<WorkerThread>> {
        let family = worker_thread_scope_family(scope);
        let db = self.inner.db.lock().unwrap();
        let threads = select_all_worker_threads(&db)?
            .into_iter()
            .filter(|thread| worker_thread_scope_family(&thread.scope) == family)
            .collect();
        Ok(threads)
    }

    pub fn worker_thread_by_id(&self, thread_id: Uuid) -> Result<Option<WorkerThread>> {
        let db = self.inner.db.lock().unwrap();
        select_worker_thread_by_id(&db, thread_id)
    }

    pub fn list_worker_threads(&self) -> Result<Vec<WorkerThread>> {
        let scope = current_worker_thread_scope();
        self.list_worker_threads_scoped(&scope)
    }

    pub fn list_worker_threads_scoped(&self, scope: &str) -> Result<Vec<WorkerThread>> {
        let db = self.inner.db.lock().unwrap();
        select_worker_threads(&db, &normalize_worker_thread_scope(scope))
    }

    pub async fn acquire_worker_thread_lease(&self, thread_id: Uuid) -> Result<WorkerThreadLease> {
        let path = self.worker_thread_lock_path(thread_id)?;
        tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("opening worker thread lock {}", path.display()))?;
            file.lock_exclusive()
                .with_context(|| format!("locking worker thread {}", thread_id))?;
            Ok(WorkerThreadLease {
                _file: file,
                _thread_id: thread_id,
            })
        })
        .await
        .context("joining worker thread lease task")?
    }

    fn worker_thread_lock_path(&self, thread_id: Uuid) -> Result<PathBuf> {
        let dir = self.inner.log_dir.join("locks").join("worker-threads");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(dir.join(format!("{thread_id}.lock")))
    }

    pub fn clear_worker_thread_native_session(&self, thread_id: Uuid) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        db.execute(
            "UPDATE worker_threads SET native_session_id = NULL, updated_at = ? WHERE id = ?",
            params![Utc::now().to_rfc3339(), thread_id.to_string()],
        )?;
        Ok(())
    }

    pub fn create_tui_job(
        &self,
        prompt: &str,
        status: &str,
        route: Option<&str>,
        role: Option<&str>,
    ) -> Result<TuiJob> {
        let now = Utc::now();
        let job = TuiJob {
            id: Uuid::new_v4(),
            prompt: prompt.to_string(),
            status: status.to_string(),
            route: route.map(str::to_string),
            role: role.map(str::to_string),
            task_id: None,
            session_id: None,
            worker_thread_id: None,
            native_session_id: None,
            created_at: now,
            updated_at: now,
            started_at: (status == "running").then_some(now),
            ended_at: None,
            result: None,
            error: None,
        };
        let db = self.inner.db.lock().unwrap();
        insert_or_replace_tui_job(&db, &job)?;
        Ok(job)
    }

    pub fn set_tui_job_status(
        &self,
        id: Uuid,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        let now = Utc::now();
        let ended_at = if matches!(
            status,
            "done" | "failed" | "cancelled" | "removed" | "skipped"
        ) {
            Some(now.to_rfc3339())
        } else {
            None
        };
        db.execute(
            r#"UPDATE tui_jobs
               SET status = ?,
                   updated_at = ?,
                   started_at = CASE WHEN ? = 'running' AND started_at IS NULL THEN ? ELSE started_at END,
                   ended_at = COALESCE(?, ended_at),
                   result = COALESCE(?, result),
                   error = COALESCE(?, error)
               WHERE id = ?"#,
            params![
                status,
                now.to_rfc3339(),
                status,
                now.to_rfc3339(),
                ended_at,
                result,
                error,
                id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn update_tui_job_prompt(&self, id: Uuid, prompt: &str) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        db.execute(
            "UPDATE tui_jobs SET prompt = ?, updated_at = ? WHERE id = ?",
            params![prompt, Utc::now().to_rfc3339(), id.to_string()],
        )?;
        Ok(())
    }

    pub fn attach_tui_job_session(
        &self,
        id: Uuid,
        session_id: Option<Uuid>,
        native_session_id: Option<&str>,
    ) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        let session = session_id
            .map(|session_id| select_session_by_id(&db, session_id))
            .transpose()?
            .flatten();
        let session_id = session.as_ref().map(|session| session.id.to_string());
        let task_id = session.as_ref().map(|session| session.task_id.to_string());
        let worker_thread_id = session
            .as_ref()
            .and_then(|session| session.worker_thread_id)
            .map(|id| id.to_string());
        let native_session_id = native_session_id.map(str::to_string).or_else(|| {
            session
                .as_ref()
                .and_then(|session| session.native_session_id.clone())
        });
        db.execute(
            r#"UPDATE tui_jobs
               SET session_id = COALESCE(?, session_id),
                   task_id = COALESCE(?, task_id),
                   worker_thread_id = COALESCE(?, worker_thread_id),
                   native_session_id = COALESCE(?, native_session_id),
                   updated_at = ?
               WHERE id = ?"#,
            params![
                session_id,
                task_id,
                worker_thread_id,
                native_session_id.as_deref(),
                Utc::now().to_rfc3339(),
                id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn attach_tui_job_worker(
        &self,
        id: Uuid,
        task_id: Option<Uuid>,
        role: Option<&str>,
        worker_thread_id: Option<Uuid>,
        native_session_id: Option<&str>,
    ) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        db.execute(
            r#"UPDATE tui_jobs
               SET task_id = COALESCE(?, task_id),
                   role = COALESCE(role, ?),
                   worker_thread_id = COALESCE(?, worker_thread_id),
                   native_session_id = COALESCE(?, native_session_id),
                   updated_at = ?
               WHERE id = ?"#,
            params![
                task_id.map(|id| id.to_string()),
                role,
                worker_thread_id.map(|id| id.to_string()),
                native_session_id,
                Utc::now().to_rfc3339(),
                id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn attach_tui_job_completed_task(
        &self,
        id: Uuid,
        task_id: Uuid,
        role: Option<&str>,
    ) -> Result<()> {
        let db = self.inner.db.lock().unwrap();
        let session = select_session_by_task_id(&db, task_id)?;
        let session_id = session.as_ref().map(|session| session.id.to_string());
        let worker_thread_id = session
            .as_ref()
            .and_then(|session| session.worker_thread_id)
            .map(|id| id.to_string());
        let native_session_id = session
            .as_ref()
            .and_then(|session| session.native_session_id.as_deref());
        db.execute(
            r#"UPDATE tui_jobs
               SET task_id = COALESCE(?, task_id),
                   role = COALESCE(role, ?),
                   session_id = COALESCE(?, session_id),
                   worker_thread_id = COALESCE(?, worker_thread_id),
                   native_session_id = COALESCE(?, native_session_id),
                   updated_at = ?
               WHERE id = ?"#,
            params![
                task_id.to_string(),
                role,
                session_id,
                worker_thread_id,
                native_session_id,
                Utc::now().to_rfc3339(),
                id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn get_tui_job(&self, id: Uuid) -> Result<Option<TuiJob>> {
        let db = self.inner.db.lock().unwrap();
        select_tui_job_by_id(&db, id)
    }

    pub fn find_tui_jobs_by_id_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<TuiJob>> {
        let prefix = prefix.trim();
        if prefix.is_empty() || !prefix.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-') {
            return Ok(Vec::new());
        }
        let db = self.inner.db.lock().unwrap();
        select_tui_jobs_by_id_prefix(&db, prefix, limit)
    }

    pub fn list_tui_jobs(&self, limit: usize) -> Result<Vec<TuiJob>> {
        let db = self.inner.db.lock().unwrap();
        select_tui_jobs(&db, limit)
    }

    pub fn list_active_tui_jobs(&self) -> Result<Vec<TuiJob>> {
        let db = self.inner.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, prompt, status, route, role, task_id, session_id, worker_thread_id, native_session_id, created_at, updated_at, started_at, ended_at, result, error
             FROM tui_jobs WHERE status IN ('queued', 'running') ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_tui_job)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_many(&self, ids: &[Uuid]) -> Result<Vec<Session>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, worker_thread_id, native_session_id, worktree_path, token_in, token_out, cost_usd, files_touched
             FROM sessions WHERE id IN ({})",
            placeholders
        );
        let db = self.inner.db.lock().unwrap();
        let mut stmt = db.prepare(&sql)?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(ids.iter().map(|u| u.to_string())),
                Self::row_to_session,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list(&self, limit: u32) -> Result<Vec<Session>> {
        let db = self.inner.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, worker_thread_id, native_session_id, worktree_path, token_in, token_out, cost_usd, files_touched
             FROM sessions ORDER BY started_at DESC LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn upsert_native_conversation(
        &self,
        conversation: &NativeConversation,
    ) -> Result<NativeImportReport> {
        let db = self.inner.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        tx.execute(
            r#"INSERT INTO native_conversations (id, cwd, created_at_unix, updated_at_unix)
               VALUES (?,?,?,?)
               ON CONFLICT(id) DO UPDATE SET
                   cwd=excluded.cwd,
                   created_at_unix=excluded.created_at_unix,
                   updated_at_unix=excluded.updated_at_unix"#,
            params![
                conversation.id.as_str(),
                conversation.cwd.as_str(),
                conversation.created_at_unix as i64,
                conversation.updated_at_unix as i64,
            ],
        )?;
        tx.execute(
            "DELETE FROM native_turns WHERE conversation_id = ?",
            params![conversation.id.as_str()],
        )?;

        let mut event_count = 0usize;
        for turn in &conversation.turns {
            tx.execute(
                r#"INSERT INTO native_turns
                   (conversation_id, turn_index, user_prompt, result, captured_at_unix)
                   VALUES (?,?,?,?,?)"#,
                params![
                    conversation.id.as_str(),
                    turn.turn_index as i64,
                    turn.user_prompt.as_str(),
                    turn.result.as_str(),
                    turn.captured_at_unix as i64,
                ],
            )?;
            for event in &turn.events {
                tx.execute(
                    r#"INSERT INTO native_events
                       (conversation_id, turn_index, event_index, role, status, title, kind, content, agent, worker_role, model, provider, task_id, worker_thread_id, native_session_id)
                       VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
                    params![
                        conversation.id.as_str(),
                        turn.turn_index as i64,
                        event.event_index as i64,
                        event.role.as_str(),
                        event.status.as_str(),
                        event.title.as_str(),
                        event.kind.as_deref(),
                        event.content.as_deref(),
                        event.agent.as_deref(),
                        event.worker_role.as_deref(),
                        event.model.as_deref(),
                        event.provider.as_deref(),
                        event.task_id.as_deref(),
                        event.worker_thread_id.as_deref(),
                        event.native_session_id.as_deref(),
                    ],
                )?;
                event_count += 1;
            }
        }
        tx.commit()?;
        Ok(NativeImportReport {
            conversations: 1,
            turns: conversation.turns.len(),
            events: event_count,
        })
    }

    pub fn append_native_turn(
        &self,
        conversation_id: &str,
        cwd: &str,
        user_prompt: &str,
        result: &str,
        captured_at_unix: u64,
        events: &[NativeEvent],
    ) -> Result<NativeImportReport> {
        let db = self.inner.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        let existing = tx
            .query_row(
                "SELECT id, created_at_unix FROM native_conversations WHERE cwd = ?",
                params![cwd],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let (conversation_id, created_at_unix) = existing
            .map(|(id, created)| (id, created.max(0) as u64))
            .unwrap_or_else(|| (conversation_id.to_string(), captured_at_unix));
        tx.execute(
            r#"INSERT INTO native_conversations (id, cwd, created_at_unix, updated_at_unix)
               VALUES (?,?,?,?)
               ON CONFLICT(id) DO UPDATE SET
                   cwd=excluded.cwd,
                   updated_at_unix=excluded.updated_at_unix"#,
            params![
                conversation_id.as_str(),
                cwd,
                created_at_unix as i64,
                captured_at_unix as i64,
            ],
        )?;
        let turn_index = tx.query_row(
            "SELECT COALESCE(MAX(turn_index) + 1, 0) FROM native_turns WHERE conversation_id = ?",
            params![conversation_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        tx.execute(
            r#"INSERT INTO native_turns
               (conversation_id, turn_index, user_prompt, result, captured_at_unix)
               VALUES (?,?,?,?,?)"#,
            params![
                conversation_id.as_str(),
                turn_index,
                user_prompt,
                result,
                captured_at_unix as i64,
            ],
        )?;
        for (idx, event) in events.iter().enumerate() {
            tx.execute(
                r#"INSERT INTO native_events
                   (conversation_id, turn_index, event_index, role, status, title, kind, content, agent, worker_role, model, provider, task_id, worker_thread_id, native_session_id)
                   VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
                params![
                    conversation_id.as_str(),
                    turn_index,
                    idx as i64,
                    event.role.as_str(),
                    event.status.as_str(),
                    event.title.as_str(),
                    event.kind.as_deref(),
                    event.content.as_deref(),
                    event.agent.as_deref(),
                    event.worker_role.as_deref(),
                    event.model.as_deref(),
                    event.provider.as_deref(),
                    event.task_id.as_deref(),
                    event.worker_thread_id.as_deref(),
                    event.native_session_id.as_deref(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(NativeImportReport {
            conversations: 1,
            turns: 1,
            events: events.len(),
        })
    }

    pub fn native_conversation_by_cwd(&self, cwd: &str) -> Result<Option<NativeConversation>> {
        let db = self.inner.db.lock().unwrap();
        let header = db
            .query_row(
                "SELECT id, cwd, created_at_unix, updated_at_unix FROM native_conversations WHERE cwd = ?",
                params![cwd],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, cwd, created_at_unix, updated_at_unix)) = header else {
            return Ok(None);
        };
        let turns = select_native_turns(&db, &id)?;
        Ok(Some(NativeConversation {
            id,
            cwd,
            created_at_unix: created_at_unix.max(0) as u64,
            updated_at_unix: updated_at_unix.max(0) as u64,
            turns,
        }))
    }

    pub fn list_native_conversations(&self) -> Result<Vec<NativeConversation>> {
        let db = self.inner.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, cwd, created_at_unix, updated_at_unix FROM native_conversations ORDER BY updated_at_unix DESC, cwd ASC",
        )?;
        let headers = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        headers
            .into_iter()
            .map(|(id, cwd, created_at_unix, updated_at_unix)| {
                Ok(NativeConversation {
                    turns: select_native_turns(&db, &id)?,
                    id,
                    cwd,
                    created_at_unix: created_at_unix.max(0) as u64,
                    updated_at_unix: updated_at_unix.max(0) as u64,
                })
            })
            .collect()
    }

    pub fn resolve_selector(&self, selector: &str) -> Result<Session> {
        let selector = selector.trim();
        if selector.is_empty() || selector == "last" || selector == "." {
            return self
                .list(1)
                .context("listing sessions")?
                .into_iter()
                .next()
                .context("No sessions yet.");
        }

        if let Ok(id) = Uuid::parse_str(selector) {
            return self
                .get_many(&[id])
                .with_context(|| format!("reading session {selector}"))?
                .into_iter()
                .next()
                .with_context(|| format!("Session not found: {selector}"));
        }

        let matches = self
            .list(10_000)
            .context("listing sessions")?
            .into_iter()
            .filter(|session| session.id.to_string().starts_with(selector))
            .collect::<Vec<_>>();

        match matches.len() {
            0 => bail!("Session not found: {selector}"),
            1 => Ok(matches.into_iter().next().unwrap()),
            count => bail!("Ambiguous session prefix: {selector} ({count} matches)"),
        }
    }

    pub fn grep(&self, pattern: &str) -> Result<Vec<(Session, Event)>> {
        // Scan all JSONL files for the pattern. Simple but effective.
        let mut hits = vec![];
        for entry in std::fs::read_dir(&self.inner.log_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            for line in content.lines() {
                if line.contains(pattern) {
                    if let Ok(ev) = serde_json::from_str::<Event>(line) {
                        if let Ok(sessions) = self.get_many(&[ev.session_id]) {
                            if let Some(s) = sessions.into_iter().next() {
                                hits.push((s, ev));
                            }
                        }
                    }
                }
            }
        }
        Ok(hits)
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let agent: String = row.get(2)?;
        let role: String = row.get(3)?;
        let model: String = row.get(4)?;
        let started_at: String = row.get(5)?;
        let ended_at: Option<String> = row.get(6)?;
        let parent_str: String = row.get(7)?;
        let worker_thread_id: Option<String> = row.get(8)?;
        let native_session_id: Option<String> = row.get(9)?;
        let worktree_path: Option<String> = row.get(10)?;
        let token_in: i64 = row.get(11)?;
        let token_out: i64 = row.get(12)?;
        let cost_usd: f64 = row.get(13)?;
        let files_str: String = row.get(14)?;

        let parent_ids: Vec<Uuid> = serde_json::from_str(&parent_str).unwrap_or_default();
        let worker_thread_id = worker_thread_id.and_then(|id| Uuid::parse_str(&id).ok());
        let files_touched: Vec<String> = serde_json::from_str(&files_str).unwrap_or_default();
        let role = match role.as_str() {
            "orchestrator" => Role::Orchestrator,
            "planner" => Role::Planner,
            "critic" => Role::Critic,
            "router" => Role::Router,
            "reviewer" => Role::Reviewer,
            "ab_judge" => Role::AbJudge,
            _ => Role::Worker,
        };

        let id_uuid = Uuid::parse_str(&id).map_err(|_e| rusqlite::Error::InvalidQuery)?;
        let task_id_uuid = Uuid::parse_str(&task_id).map_err(|_e| rusqlite::Error::InvalidQuery)?;
        let started_at_dt = DateTime::parse_from_rfc3339(&started_at)
            .map_err(|_e| rusqlite::Error::InvalidQuery)?
            .with_timezone(&Utc);
        let ended_at_dt = ended_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|_e| rusqlite::Error::InvalidQuery)?;

        Ok(Session {
            id: id_uuid,
            task_id: task_id_uuid,
            agent,
            role,
            model,
            started_at: started_at_dt,
            ended_at: ended_at_dt,
            parent_session_ids: parent_ids,
            worker_thread_id,
            native_session_id,
            worktree_path: worktree_path.map(PathBuf::from),
            token_in: token_in.max(0) as u64,
            token_out: token_out.max(0) as u64,
            cost_usd,
            files_touched,
        })
    }
}

impl Drop for WorkerThreadLease {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

fn migrate_worker_threads_table(db: &Connection) -> Result<()> {
    if !table_has_column(db, "worker_threads", "scope")? {
        db.execute(
            "ALTER TABLE worker_threads ADD COLUMN scope TEXT NOT NULL DEFAULT 'global'",
            [],
        )?;
    }
    if worker_threads_table_needs_rebuild(db)? {
        db.execute_batch(
            r#"
            ALTER TABLE worker_threads RENAME TO worker_threads_old;
            CREATE TABLE worker_threads (
                id TEXT PRIMARY KEY,
                scope TEXT NOT NULL DEFAULT 'global',
                role TEXT NOT NULL,
                runtime TEXT NOT NULL,
                agent TEXT NOT NULL,
                model TEXT NOT NULL,
                provider TEXT,
                worktree_path TEXT,
                native_session_id TEXT,
                last_session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(scope, role)
            );
            INSERT OR REPLACE INTO worker_threads
                (id, scope, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at)
            SELECT
                id,
                COALESCE(NULLIF(scope, ''), 'global'),
                role,
                runtime,
                agent,
                model,
                provider,
                worktree_path,
                native_session_id,
                last_session_id,
                created_at,
                updated_at
            FROM worker_threads_old;
            DROP TABLE worker_threads_old;
            "#,
        )?;
    }
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_worker_threads_scope_role ON worker_threads(scope, role)",
        [],
    )?;
    Ok(())
}

fn worker_threads_table_needs_rebuild(db: &Connection) -> Result<bool> {
    let sql: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'worker_threads'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(sql) = sql else {
        return Ok(false);
    };
    let normalized = sql
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Ok(normalized.contains("role text not null unique")
        || !normalized.contains("unique(scope, role)"))
}

fn migrate_sessions_table(db: &Connection) -> Result<()> {
    for (column, definition) in [
        ("worker_thread_id", "TEXT"),
        ("native_session_id", "TEXT"),
        ("worktree_path", "TEXT"),
    ] {
        if !table_has_column(db, "sessions", column)? {
            db.execute(
                &format!("ALTER TABLE sessions ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn migrate_native_events_table(db: &Connection) -> Result<()> {
    for (column, definition) in [
        ("kind", "TEXT"),
        ("model", "TEXT"),
        ("provider", "TEXT"),
        ("task_id", "TEXT"),
        ("worker_thread_id", "TEXT"),
        ("native_session_id", "TEXT"),
    ] {
        if table_has_column(db, "native_events", column)? {
            continue;
        }
        db.execute(
            &format!("ALTER TABLE native_events ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn migrate_tui_jobs_table(db: &Connection) -> Result<()> {
    for (column, definition) in [
        ("route", "TEXT"),
        ("role", "TEXT"),
        ("task_id", "TEXT"),
        ("session_id", "TEXT"),
        ("worker_thread_id", "TEXT"),
        ("native_session_id", "TEXT"),
        ("started_at", "TEXT"),
        ("ended_at", "TEXT"),
        ("result", "TEXT"),
        ("error", "TEXT"),
    ] {
        if table_has_column(db, "tui_jobs", column)? {
            continue;
        }
        db.execute(
            &format!("ALTER TABLE tui_jobs ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn select_native_turns(db: &Connection, conversation_id: &str) -> Result<Vec<NativeTurn>> {
    let mut stmt = db.prepare(
        "SELECT turn_index, user_prompt, result, captured_at_unix
         FROM native_turns WHERE conversation_id = ? ORDER BY turn_index ASC",
    )?;
    let rows = stmt
        .query_map(params![conversation_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(turn_index, user_prompt, result, captured_at_unix)| {
            Ok(NativeTurn {
                turn_index: turn_index.max(0) as u32,
                user_prompt,
                result,
                captured_at_unix: captured_at_unix.max(0) as u64,
                events: select_native_events(db, conversation_id, turn_index.max(0) as u32)?,
            })
        })
        .collect()
}

fn select_native_events(
    db: &Connection,
    conversation_id: &str,
    turn_index: u32,
) -> Result<Vec<NativeEvent>> {
    let mut stmt = db.prepare(
        "SELECT event_index, role, status, title, kind, content, agent, worker_role, model, provider, task_id, worker_thread_id, native_session_id
         FROM native_events WHERE conversation_id = ? AND turn_index = ? ORDER BY event_index ASC",
    )?;
    let rows = stmt
        .query_map(params![conversation_id, turn_index as i64], |row| {
            Ok(NativeEvent {
                event_index: row.get::<_, i64>(0)?.max(0) as u32,
                role: row.get(1)?,
                status: row.get(2)?,
                title: row.get(3)?,
                kind: row.get(4)?,
                content: row.get(5)?,
                agent: row.get(6)?,
                worker_role: row.get(7)?,
                model: row.get(8)?,
                provider: row.get(9)?,
                task_id: row.get(10)?,
                worker_thread_id: row.get(11)?,
                native_session_id: row.get(12)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn insert_or_replace_tui_job(db: &Connection, job: &TuiJob) -> Result<()> {
    db.execute(
        r#"INSERT OR REPLACE INTO tui_jobs
           (id, prompt, status, route, role, task_id, session_id, worker_thread_id, native_session_id, created_at, updated_at, started_at, ended_at, result, error)
           VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
        params![
            job.id.to_string(),
            job.prompt.as_str(),
            job.status.as_str(),
            job.route.as_deref(),
            job.role.as_deref(),
            job.task_id.map(|id| id.to_string()),
            job.session_id.map(|id| id.to_string()),
            job.worker_thread_id.map(|id| id.to_string()),
            job.native_session_id.as_deref(),
            job.created_at.to_rfc3339(),
            job.updated_at.to_rfc3339(),
            job.started_at.map(|ts| ts.to_rfc3339()),
            job.ended_at.map(|ts| ts.to_rfc3339()),
            job.result.as_deref(),
            job.error.as_deref(),
        ],
    )?;
    Ok(())
}

fn select_tui_job_by_id(db: &Connection, id: Uuid) -> Result<Option<TuiJob>> {
    db.query_row(
        "SELECT id, prompt, status, route, role, task_id, session_id, worker_thread_id, native_session_id, created_at, updated_at, started_at, ended_at, result, error
         FROM tui_jobs WHERE id = ?",
        params![id.to_string()],
        row_to_tui_job,
    )
    .optional()
    .map_err(Into::into)
}

fn select_tui_jobs_by_id_prefix(
    db: &Connection,
    prefix: &str,
    limit: usize,
) -> Result<Vec<TuiJob>> {
    let mut stmt = db.prepare(
        "SELECT id, prompt, status, route, role, task_id, session_id, worker_thread_id, native_session_id, created_at, updated_at, started_at, ended_at, result, error
         FROM tui_jobs WHERE id LIKE ? ORDER BY updated_at DESC LIMIT ?",
    )?;
    let rows = stmt
        .query_map(params![format!("{prefix}%"), limit as i64], row_to_tui_job)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn select_tui_jobs(db: &Connection, limit: usize) -> Result<Vec<TuiJob>> {
    let mut stmt = db.prepare(
        "SELECT id, prompt, status, route, role, task_id, session_id, worker_thread_id, native_session_id, created_at, updated_at, started_at, ended_at, result, error
         FROM tui_jobs ORDER BY updated_at DESC LIMIT ?",
    )?;
    let rows = stmt
        .query_map(params![limit as i64], row_to_tui_job)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn row_to_tui_job(row: &rusqlite::Row) -> rusqlite::Result<TuiJob> {
    let id: String = row.get(0)?;
    let task_id: Option<String> = row.get(5)?;
    let session_id: Option<String> = row.get(6)?;
    let worker_thread_id: Option<String> = row.get(7)?;
    Ok(TuiJob {
        id: Uuid::parse_str(&id).map_err(|_e| rusqlite::Error::InvalidQuery)?,
        prompt: row.get(1)?,
        status: row.get(2)?,
        route: row.get(3)?,
        role: row.get(4)?,
        task_id: parse_optional_uuid(task_id),
        session_id: session_id.and_then(|id| Uuid::parse_str(&id).ok()),
        worker_thread_id: parse_optional_uuid(worker_thread_id),
        native_session_id: row.get(8)?,
        created_at: parse_rfc3339_row(row.get::<_, String>(9)?)?,
        updated_at: parse_rfc3339_row(row.get::<_, String>(10)?)?,
        started_at: parse_optional_rfc3339_row(row.get(11)?)?,
        ended_at: parse_optional_rfc3339_row(row.get(12)?)?,
        result: row.get(13)?,
        error: row.get(14)?,
    })
}

fn parse_optional_uuid(raw: Option<String>) -> Option<Uuid> {
    raw.and_then(|id| Uuid::parse_str(&id).ok())
}

fn select_session_by_id(db: &Connection, id: Uuid) -> Result<Option<Session>> {
    db.query_row(
        "SELECT id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, worker_thread_id, native_session_id, worktree_path, token_in, token_out, cost_usd, files_touched
         FROM sessions WHERE id = ?",
        params![id.to_string()],
        SessionStore::row_to_session,
    )
    .optional()
    .map_err(Into::into)
}

fn select_session_by_task_id(db: &Connection, task_id: Uuid) -> Result<Option<Session>> {
    db.query_row(
        "SELECT id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, worker_thread_id, native_session_id, worktree_path, token_in, token_out, cost_usd, files_touched
         FROM sessions WHERE task_id = ? ORDER BY started_at DESC LIMIT 1",
        params![task_id.to_string()],
        SessionStore::row_to_session,
    )
    .optional()
    .map_err(Into::into)
}

fn parse_rfc3339_row(raw: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_e| rusqlite::Error::InvalidQuery)
}

fn parse_optional_rfc3339_row(raw: Option<String>) -> rusqlite::Result<Option<DateTime<Utc>>> {
    raw.map(parse_rfc3339_row).transpose()
}

fn table_has_column(db: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn select_worker_thread_by_role(
    db: &Connection,
    scope: &str,
    role: &str,
) -> Result<Option<WorkerThread>> {
    let mut stmt = db.prepare(
        "SELECT id, scope, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at
         FROM worker_threads WHERE scope = ? AND role = ?",
    )?;
    let mut rows = stmt.query(params![scope, role])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row_to_worker_thread(row)?));
    }
    Ok(None)
}

fn select_worker_thread_by_id(db: &Connection, id: Uuid) -> Result<Option<WorkerThread>> {
    let mut stmt = db.prepare(
        "SELECT id, scope, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at
         FROM worker_threads WHERE id = ?",
    )?;
    let mut rows = stmt.query(params![id.to_string()])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row_to_worker_thread(row)?));
    }
    Ok(None)
}

fn select_worker_threads(db: &Connection, scope: &str) -> Result<Vec<WorkerThread>> {
    let mut stmt = db.prepare(
        "SELECT id, scope, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at
         FROM worker_threads WHERE scope = ? ORDER BY updated_at DESC, role ASC",
    )?;
    let rows = stmt
        .query_map(params![scope], row_to_worker_thread)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn select_all_worker_threads(db: &Connection) -> Result<Vec<WorkerThread>> {
    let mut stmt = db.prepare(
        "SELECT id, scope, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at
         FROM worker_threads ORDER BY updated_at DESC, role ASC",
    )?;
    let rows = stmt
        .query_map([], row_to_worker_thread)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn update_worker_thread_row(db: &Connection, thread: &WorkerThread) -> Result<()> {
    db.execute(
        r#"UPDATE worker_threads
        SET scope = ?, runtime = ?, agent = ?, model = ?, provider = ?, worktree_path = ?, native_session_id = ?, last_session_id = ?, updated_at = ?
        WHERE id = ?"#,
        params![
            thread.scope.as_str(),
            thread.runtime.as_str(),
            thread.agent.as_str(),
            thread.model.as_str(),
            thread.provider.as_deref(),
            thread
                .worktree_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            thread.native_session_id.as_deref(),
            thread.last_session_id.map(|id| id.to_string()),
            thread.updated_at.to_rfc3339(),
            thread.id.to_string(),
        ],
    )?;
    Ok(())
}

fn row_to_worker_thread(row: &rusqlite::Row) -> rusqlite::Result<WorkerThread> {
    let id: String = row.get(0)?;
    let scope: String = row.get(1)?;
    let role: String = row.get(2)?;
    let runtime: String = row.get(3)?;
    let agent: String = row.get(4)?;
    let model: String = row.get(5)?;
    let provider: Option<String> = row.get(6)?;
    let worktree_path: Option<String> = row.get(7)?;
    let native_session_id: Option<String> = row.get(8)?;
    let last_session_id: Option<String> = row.get(9)?;
    let created_at: String = row.get(10)?;
    let updated_at: String = row.get(11)?;

    Ok(WorkerThread {
        id: Uuid::parse_str(&id).map_err(|_e| rusqlite::Error::InvalidQuery)?,
        scope,
        role,
        runtime,
        agent,
        model,
        provider,
        worktree_path: worktree_path.map(PathBuf::from),
        native_session_id,
        last_session_id: last_session_id.and_then(|id| Uuid::parse_str(&id).ok()),
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map_err(|_e| rusqlite::Error::InvalidQuery)?
            .with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map_err(|_e| rusqlite::Error::InvalidQuery)?
            .with_timezone(&Utc),
    })
}

fn current_worker_thread_scope() -> String {
    let env_scope = std::env::var("TIFFANY_WORKER_SCOPE").ok();
    worker_thread_scope_from_env_or_cwd(env_scope.as_deref(), std::env::current_dir().ok())
}

fn worker_thread_scope_from_env_or_cwd(env_scope: Option<&str>, cwd: Option<PathBuf>) -> String {
    if let Some(scope) = env_scope
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(normalize_worker_thread_scope)
    {
        return scope;
    }

    cwd.map(|cwd| {
        cwd.canonicalize()
            .unwrap_or(cwd)
            .to_string_lossy()
            .into_owned()
    })
    .map(|cwd| format!("cwd:{cwd}"))
    .unwrap_or_else(|| "global".to_string())
}

fn normalize_worker_thread_scope(scope: &str) -> String {
    let scope = scope.trim();
    if scope.is_empty() {
        "global".to_string()
    } else {
        scope.to_string()
    }
}

fn worker_thread_scope_family(scope: &str) -> String {
    let scope = normalize_worker_thread_scope(scope);
    scope
        .rsplit_once(":session:")
        .map(|(family, _session)| family.to_string())
        .unwrap_or(scope)
}

#[async_trait::async_trait]
impl SessionReader for SessionStore {
    async fn get_many(&self, ids: &[Uuid]) -> Result<Vec<Session>> {
        SessionStore::get_many(self, ids)
    }
    async fn list(&self, limit: u32) -> Result<Vec<Session>> {
        SessionStore::list(self, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn test_store() -> (tempfile::TempDir, SessionStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        (tmp, store)
    }

    #[test]
    fn worker_thread_scope_prefers_env_override() {
        let cwd = PathBuf::from("/tmp/tiffany-project");

        assert_eq!(
            worker_thread_scope_from_env_or_cwd(Some("  tui:session:/tmp/project  "), Some(cwd)),
            "tui:session:/tmp/project"
        );
        assert_eq!(
            worker_thread_scope_from_env_or_cwd(Some("  "), None),
            "global"
        );
    }

    #[test]
    fn resolve_selector_accepts_last_dot_full_id_and_prefix() {
        let (_tmp, store) = test_store();
        let mut older = Session::new(Uuid::new_v4(), "older", Role::Worker);
        older.id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        older.started_at = Utc::now() - Duration::seconds(10);
        let mut newer = Session::new(Uuid::new_v4(), "newer", Role::Worker);
        newer.id = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        newer.started_at = Utc::now();
        store.finalize(&older).unwrap();
        store.finalize(&newer).unwrap();

        assert_eq!(store.resolve_selector("last").unwrap().id, newer.id);
        assert_eq!(store.resolve_selector(".").unwrap().id, newer.id);
        assert_eq!(
            store.resolve_selector(&newer.id.to_string()).unwrap().id,
            newer.id
        );
        assert_eq!(store.resolve_selector("22222222").unwrap().id, newer.id);
    }

    #[test]
    fn resolve_selector_reports_ambiguous_prefixes() {
        let (_tmp, store) = test_store();
        let mut first = Session::new(Uuid::new_v4(), "first", Role::Worker);
        first.id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1").unwrap();
        let mut second = Session::new(Uuid::new_v4(), "second", Role::Worker);
        second.id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2").unwrap();
        store.finalize(&first).unwrap();
        store.finalize(&second).unwrap();

        let err = store.resolve_selector("aaaaaaaa").unwrap_err().to_string();
        assert!(err.contains("Ambiguous session prefix"));
    }

    #[test]
    fn worker_threads_are_reused_by_role_and_keep_last_session() {
        let (_tmp, store) = test_store();

        let first = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();
        let second = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();

        assert_eq!(first.id, second.id);
        let session_id = Uuid::new_v4();
        store
            .update_worker_thread_after_session(
                first.id,
                Some("native-1"),
                session_id,
                Some(std::path::Path::new("/tmp/tiffany-worker")),
            )
            .unwrap();
        let updated = store.worker_thread_by_role("worker-cc").unwrap().unwrap();

        assert_eq!(updated.id, first.id);
        assert_eq!(updated.native_session_id.as_deref(), Some("native-1"));
        assert_eq!(updated.last_session_id, Some(session_id));
        assert_eq!(
            updated.worktree_path.as_deref(),
            Some(std::path::Path::new("/tmp/tiffany-worker"))
        );
    }

    #[test]
    fn tui_jobs_persist_status_prompt_and_recent_listing() {
        let (_tmp, store) = test_store();

        let queued = store
            .create_tui_job(
                "first queued prompt",
                "queued",
                Some("direct-answer"),
                Some("worker-cc"),
            )
            .unwrap();
        let running = store
            .create_tui_job(
                "running prompt",
                "running",
                Some("single-worker"),
                Some("worker-codex"),
            )
            .unwrap();

        store
            .update_tui_job_prompt(queued.id, "edited queued prompt")
            .unwrap();
        store
            .set_tui_job_status(queued.id, "removed", None, Some("removed from queue"))
            .unwrap();
        let task_id = Uuid::new_v4();
        let worker_thread_id = Uuid::new_v4();
        store
            .attach_tui_job_worker(
                running.id,
                Some(task_id),
                Some("worker-codex"),
                Some(worker_thread_id),
                Some("native-1"),
            )
            .unwrap();
        let mut session = Session::new(task_id, "worker-codex", Role::Worker);
        session.worker_thread_id = Some(worker_thread_id);
        session.native_session_id = Some("native-1".to_string());
        store.finalize(&session).unwrap();
        store
            .attach_tui_job_completed_task(running.id, task_id, Some("worker-codex"))
            .unwrap();
        store
            .set_tui_job_status(running.id, "done", Some("done text"), None)
            .unwrap();

        let queued = store.get_tui_job(queued.id).unwrap().unwrap();
        let running = store.get_tui_job(running.id).unwrap().unwrap();
        let recent = store.list_tui_jobs(10).unwrap();

        assert_eq!(queued.prompt, "edited queued prompt");
        assert_eq!(queued.status, "removed");
        assert_eq!(queued.error.as_deref(), Some("removed from queue"));
        assert!(queued.ended_at.is_some());
        assert_eq!(running.status, "done");
        assert_eq!(running.task_id, Some(task_id));
        assert_eq!(running.session_id, Some(session.id));
        assert_eq!(running.worker_thread_id, Some(worker_thread_id));
        assert_eq!(running.native_session_id.as_deref(), Some("native-1"));
        assert_eq!(running.result.as_deref(), Some("done text"));
        assert_eq!(store.list_active_tui_jobs().unwrap(), Vec::new());
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn worker_threads_are_scoped_by_conversation_context() {
        let (_tmp, store) = test_store();

        let first = store
            .get_or_create_worker_thread_scoped(
                "cwd:/tmp/project-a",
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();
        let same_scope = store
            .get_or_create_worker_thread_scoped(
                "cwd:/tmp/project-a",
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();
        let other_scope = store
            .get_or_create_worker_thread_scoped(
                "cwd:/tmp/project-b",
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();

        assert_eq!(first.id, same_scope.id);
        assert_ne!(first.id, other_scope.id);
        assert_eq!(first.scope, "cwd:/tmp/project-a");
        assert_eq!(other_scope.scope, "cwd:/tmp/project-b");

        store
            .update_worker_thread_after_session(first.id, Some("native-a"), Uuid::new_v4(), None)
            .unwrap();
        store
            .update_worker_thread_after_session(
                other_scope.id,
                Some("native-b"),
                Uuid::new_v4(),
                None,
            )
            .unwrap();

        assert_eq!(
            store
                .worker_thread_by_role_scoped("cwd:/tmp/project-a", "worker-cc")
                .unwrap()
                .unwrap()
                .native_session_id
                .as_deref(),
            Some("native-a")
        );
        assert_eq!(
            store
                .worker_thread_by_role_scoped("cwd:/tmp/project-b", "worker-cc")
                .unwrap()
                .unwrap()
                .native_session_id
                .as_deref(),
            Some("native-b")
        );

        let scoped = store
            .list_worker_threads_scoped("cwd:/tmp/project-a")
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, first.id);
    }

    #[test]
    fn worker_threads_can_be_found_across_same_tui_scope_family() {
        let (_tmp, store) = test_store();

        let first_session = store
            .get_or_create_worker_thread_scoped(
                "tui:/tmp/project-a:session:first",
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();
        store
            .update_worker_thread_after_session(
                first_session.id,
                Some("native-first"),
                Uuid::new_v4(),
                None,
            )
            .unwrap();
        let second_session = store
            .get_or_create_worker_thread_scoped(
                "tui:/tmp/project-a:session:second",
                "worker-codex",
                "codex",
                "codex",
                "gpt-4o",
                Some("openai"),
            )
            .unwrap();
        let other_project = store
            .get_or_create_worker_thread_scoped(
                "tui:/tmp/project-b:session:first",
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();

        let family = store
            .worker_threads_in_scope_family("tui:/tmp/project-a:session:current")
            .unwrap();
        let ids = family.iter().map(|thread| thread.id).collect::<Vec<_>>();

        assert!(ids.contains(&first_session.id));
        assert!(ids.contains(&second_session.id));
        assert!(!ids.contains(&other_project.id));
        assert_eq!(
            worker_thread_scope_family("tui:/tmp/project-a:session:current"),
            "tui:/tmp/project-a"
        );
        assert_eq!(
            worker_thread_scope_family("cwd:/tmp/project-a"),
            "cwd:/tmp/project-a"
        );
    }

    #[test]
    fn worker_thread_native_session_can_be_cleared() {
        let (_tmp, store) = test_store();
        let thread = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();
        store
            .update_worker_thread_after_session(
                thread.id,
                Some("stale-native"),
                Uuid::new_v4(),
                None,
            )
            .unwrap();

        store.clear_worker_thread_native_session(thread.id).unwrap();

        let updated = store.worker_thread_by_role("worker-cc").unwrap().unwrap();
        assert_eq!(updated.id, thread.id);
        assert!(updated.native_session_id.is_none());
        assert!(updated.last_session_id.is_some());
    }

    #[test]
    fn worker_thread_clears_native_session_when_runtime_provider_agent_or_model_changes() {
        let (_tmp, store) = test_store();
        let thread = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();
        let first_session_id = Uuid::new_v4();
        store
            .update_worker_thread_after_session(thread.id, Some("native-1"), first_session_id, None)
            .unwrap();

        let model_changed = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "opus",
                Some("anthropic"),
            )
            .unwrap();
        assert_eq!(model_changed.id, thread.id);
        assert!(
            model_changed.native_session_id.is_none(),
            "model changes must not resume a native CLI session created under another model"
        );
        assert_eq!(model_changed.last_session_id, Some(first_session_id));

        let second_session_id = Uuid::new_v4();
        store
            .update_worker_thread_after_session(
                thread.id,
                Some("native-2"),
                second_session_id,
                None,
            )
            .unwrap();
        let provider_changed = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "opus",
                Some("minimax"),
            )
            .unwrap();
        assert_eq!(provider_changed.id, thread.id);
        assert!(provider_changed.native_session_id.is_none());
        assert_eq!(provider_changed.last_session_id, Some(second_session_id));

        let third_session_id = Uuid::new_v4();
        store
            .update_worker_thread_after_session(thread.id, Some("native-3"), third_session_id, None)
            .unwrap();
        let agent_changed = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude",
                "opus",
                Some("minimax"),
            )
            .unwrap();
        assert_eq!(agent_changed.id, thread.id);
        assert!(agent_changed.native_session_id.is_none());
        assert_eq!(agent_changed.last_session_id, Some(third_session_id));

        store
            .update_worker_thread_after_session(thread.id, Some("native-4"), Uuid::new_v4(), None)
            .unwrap();
        let runtime_changed = store
            .get_or_create_worker_thread("worker-cc", "codex", "codex", "gpt-5", Some("openai"))
            .unwrap();
        assert_eq!(runtime_changed.id, thread.id);
        assert!(runtime_changed.native_session_id.is_none());
    }

    #[test]
    fn worker_threads_can_be_listed_newest_first() {
        let (_tmp, store) = test_store();
        let first = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "sonnet",
                Some("anthropic"),
            )
            .unwrap();
        let second = store
            .get_or_create_worker_thread("worker-codex", "codex", "codex", "gpt-4o", Some("openai"))
            .unwrap();
        let session_id = Uuid::new_v4();
        store
            .update_worker_thread_after_session(
                first.id,
                Some("native-1"),
                session_id,
                Some(std::path::Path::new("/tmp/tiffany-worker")),
            )
            .unwrap();

        let threads = store.list_worker_threads().unwrap();

        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, first.id);
        assert_eq!(threads[0].role, "worker-cc");
        assert_eq!(threads[0].native_session_id.as_deref(), Some("native-1"));
        assert_eq!(threads[0].last_session_id, Some(session_id));
        assert!(threads.iter().any(|thread| thread.id == second.id));
    }

    #[test]
    fn sessions_persist_worker_thread_native_session_and_worktree() {
        let (_tmp, store) = test_store();
        let mut session = Session::new(Uuid::new_v4(), "claude-code", Role::Worker);
        let thread_id = Uuid::new_v4();
        session.worker_thread_id = Some(thread_id);
        session.native_session_id = Some("native-session".to_string());
        session.worktree_path = Some(std::path::PathBuf::from("/tmp/thread-worktree"));

        store.finalize(&session).unwrap();
        let loaded = store.get_many(&[session.id]).unwrap().remove(0);

        assert_eq!(loaded.worker_thread_id, Some(thread_id));
        assert_eq!(loaded.native_session_id.as_deref(), Some("native-session"));
        assert_eq!(
            loaded.worktree_path.as_deref(),
            Some(std::path::Path::new("/tmp/thread-worktree"))
        );
    }

    #[test]
    fn native_conversation_round_trips_typed_events() {
        let (_tmp, store) = test_store();
        let conversation = NativeConversation {
            id: "tiffany-native-test".to_string(),
            cwd: "/tmp/project".to_string(),
            created_at_unix: 10,
            updated_at_unix: 20,
            turns: vec![NativeTurn {
                turn_index: 0,
                user_prompt: "改 README".to_string(),
                result: "已修改".to_string(),
                captured_at_unix: 20,
                events: vec![NativeEvent {
                    event_index: 0,
                    role: "worker".to_string(),
                    status: "output".to_string(),
                    title: "worker diff · worker-cc · claude-code".to_string(),
                    kind: Some("diff".to_string()),
                    content: Some("diff --git a/README.md b/README.md".to_string()),
                    agent: Some("claude-code".to_string()),
                    worker_role: Some("worker-cc".to_string()),
                    model: Some("claude-sonnet-4-6".to_string()),
                    provider: Some("anthropic".to_string()),
                    task_id: Some("task-1".to_string()),
                    worker_thread_id: Some("thread-1".to_string()),
                    native_session_id: Some("native-1".to_string()),
                }],
            }],
        };

        let report = store
            .upsert_native_conversation(&conversation)
            .expect("upsert native");
        let loaded = store
            .native_conversation_by_cwd("/tmp/project")
            .expect("load native")
            .expect("native conversation");
        let listed = store
            .list_native_conversations()
            .expect("list native conversations");

        assert_eq!(
            report,
            NativeImportReport {
                conversations: 1,
                turns: 1,
                events: 1
            }
        );
        assert_eq!(loaded, conversation);
        assert_eq!(listed, vec![conversation]);
    }

    #[test]
    fn native_turn_append_preserves_existing_turns() {
        let (_tmp, store) = test_store();
        let first_events = vec![NativeEvent {
            event_index: 99,
            role: "worker".to_string(),
            status: "output".to_string(),
            title: "worker answer · worker-cc".to_string(),
            kind: Some("answer".to_string()),
            content: Some("first answer".to_string()),
            agent: Some("claude-code".to_string()),
            worker_role: Some("worker-cc".to_string()),
            model: Some("claude-sonnet".to_string()),
            provider: Some("anthropic".to_string()),
            task_id: Some("task-1".to_string()),
            worker_thread_id: Some("thread-1".to_string()),
            native_session_id: Some("native-1".to_string()),
        }];
        let second_events = vec![NativeEvent {
            event_index: 99,
            role: "worker".to_string(),
            status: "output".to_string(),
            title: "worker answer · worker-codex".to_string(),
            kind: Some("answer".to_string()),
            content: Some("second answer".to_string()),
            agent: Some("codex".to_string()),
            worker_role: Some("worker-codex".to_string()),
            model: Some("gpt-5".to_string()),
            provider: Some("openai".to_string()),
            task_id: Some("task-2".to_string()),
            worker_thread_id: Some("thread-2".to_string()),
            native_session_id: Some("native-2".to_string()),
        }];

        let first = store
            .append_native_turn(
                "tiffany-native-test",
                "/tmp/project",
                "first prompt",
                "first answer",
                10,
                &first_events,
            )
            .expect("append first turn");
        let second = store
            .append_native_turn(
                "ignored-new-id",
                "/tmp/project",
                "second prompt",
                "second answer",
                20,
                &second_events,
            )
            .expect("append second turn");
        let loaded = store
            .native_conversation_by_cwd("/tmp/project")
            .expect("load native")
            .expect("native conversation");

        assert_eq!(
            first,
            NativeImportReport {
                conversations: 1,
                turns: 1,
                events: 1
            }
        );
        assert_eq!(
            second,
            NativeImportReport {
                conversations: 1,
                turns: 1,
                events: 1
            }
        );
        assert_eq!(loaded.id, "tiffany-native-test");
        assert_eq!(loaded.created_at_unix, 10);
        assert_eq!(loaded.updated_at_unix, 20);
        assert_eq!(loaded.turns.len(), 2);
        assert_eq!(loaded.turns[0].turn_index, 0);
        assert_eq!(loaded.turns[0].user_prompt, "first prompt");
        assert_eq!(loaded.turns[0].events[0].event_index, 0);
        assert_eq!(loaded.turns[1].turn_index, 1);
        assert_eq!(loaded.turns[1].user_prompt, "second prompt");
        assert_eq!(loaded.turns[1].events[0].event_index, 0);
        assert_eq!(
            loaded.turns[1].events[0].worker_role.as_deref(),
            Some("worker-codex")
        );
    }
}
