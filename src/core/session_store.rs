//! Persistent session log: JSONL files + SQLite index.

use crate::core::types::{Event, Role, Session};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
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
            "#,
        )?;
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
            (id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, token_in, token_out, cost_usd, files_touched)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"#,
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
                session.token_in as i64,
                session.token_out as i64,
                session.cost_usd,
                serde_json::to_string(&session.files_touched)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_many(&self, ids: &[Uuid]) -> Result<Vec<Session>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, token_in, token_out, cost_usd, files_touched
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
            "SELECT id, task_id, agent, role, model, started_at, ended_at, parent_session_ids, token_in, token_out, cost_usd, files_touched
             FROM sessions ORDER BY started_at DESC LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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
        let token_in: i64 = row.get(8)?;
        let token_out: i64 = row.get(9)?;
        let cost_usd: f64 = row.get(10)?;
        let files_str: String = row.get(11)?;

        let parent_ids: Vec<Uuid> = serde_json::from_str(&parent_str).unwrap_or_default();
        let files_touched: Vec<String> = serde_json::from_str(&files_str).unwrap_or_default();
        let role = match role.as_str() {
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
            token_in: token_in.max(0) as u64,
            token_out: token_out.max(0) as u64,
            cost_usd,
            files_touched,
        })
    }
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
