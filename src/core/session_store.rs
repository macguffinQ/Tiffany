//! Persistent session log: JSONL files + SQLite index.

use crate::core::types::{Event, Role, Session};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rusqlite::{params, Connection};
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
                role TEXT NOT NULL UNIQUE,
                runtime TEXT NOT NULL,
                agent TEXT NOT NULL,
                model TEXT NOT NULL,
                provider TEXT,
                worktree_path TEXT,
                native_session_id TEXT,
                last_session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            "#,
        )?;
        migrate_sessions_table(&db)?;
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
        let db = self.inner.db.lock().unwrap();
        let existing = select_worker_thread_by_role(&db, role)?;
        let now = Utc::now();
        if let Some(mut thread) = existing {
            let provider_changed = thread.provider.as_deref() != provider;
            let runtime_changed = thread.runtime != runtime;
            if runtime_changed || provider_changed || thread.agent != agent || thread.model != model
            {
                thread.runtime = runtime.to_string();
                thread.agent = agent.to_string();
                thread.model = model.to_string();
                thread.provider = provider.map(str::to_string);
                thread.updated_at = now;
                if runtime_changed || provider_changed {
                    thread.native_session_id = None;
                }
                update_worker_thread_row(&db, &thread)?;
            }
            return Ok(thread);
        }

        let thread = WorkerThread {
            id: Uuid::new_v4(),
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
            (id, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at)
            VALUES (?,?,?,?,?,?,?,?,?,?,?)"#,
            params![
                thread.id.to_string(),
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
        let db = self.inner.db.lock().unwrap();
        select_worker_thread_by_role(&db, role)
    }

    pub fn worker_thread_by_id(&self, thread_id: Uuid) -> Result<Option<WorkerThread>> {
        let db = self.inner.db.lock().unwrap();
        select_worker_thread_by_id(&db, thread_id)
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

fn select_worker_thread_by_role(db: &Connection, role: &str) -> Result<Option<WorkerThread>> {
    let mut stmt = db.prepare(
        "SELECT id, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at
         FROM worker_threads WHERE role = ?",
    )?;
    let mut rows = stmt.query(params![role])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row_to_worker_thread(row)?));
    }
    Ok(None)
}

fn select_worker_thread_by_id(db: &Connection, id: Uuid) -> Result<Option<WorkerThread>> {
    let mut stmt = db.prepare(
        "SELECT id, role, runtime, agent, model, provider, worktree_path, native_session_id, last_session_id, created_at, updated_at
         FROM worker_threads WHERE id = ?",
    )?;
    let mut rows = stmt.query(params![id.to_string()])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row_to_worker_thread(row)?));
    }
    Ok(None)
}

fn update_worker_thread_row(db: &Connection, thread: &WorkerThread) -> Result<()> {
    db.execute(
        r#"UPDATE worker_threads
        SET runtime = ?, agent = ?, model = ?, provider = ?, worktree_path = ?, native_session_id = ?, last_session_id = ?, updated_at = ?
        WHERE id = ?"#,
        params![
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
    let role: String = row.get(1)?;
    let runtime: String = row.get(2)?;
    let agent: String = row.get(3)?;
    let model: String = row.get(4)?;
    let provider: Option<String> = row.get(5)?;
    let worktree_path: Option<String> = row.get(6)?;
    let native_session_id: Option<String> = row.get(7)?;
    let last_session_id: Option<String> = row.get(8)?;
    let created_at: String = row.get(9)?;
    let updated_at: String = row.get(10)?;

    Ok(WorkerThread {
        id: Uuid::parse_str(&id).map_err(|_e| rusqlite::Error::InvalidQuery)?,
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
}
