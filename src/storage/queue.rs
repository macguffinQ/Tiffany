//! Task queue backed by SQLite. (Light wrapper — most state lives in
//! SessionStore; this is for explicit task scheduling if needed.)

use crate::core::types::Task;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct TaskQueue {
    inner: Arc<Mutex<Connection>>,
}

impl TaskQueue {
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = Connection::open(db_path)?;
        db.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                deps TEXT NOT NULL DEFAULT '[]',
                role TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                result TEXT
            );
            "#,
        )?;
        Ok(Self {
            inner: Arc::new(Mutex::new(db)),
        })
    }

    pub fn enqueue(&self, task: &Task) -> Result<()> {
        let db = self.inner.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO tasks (id, prompt, tags, deps, role, status, created_at, result)
             VALUES (?,?,?,?,?,?,?,?)",
            params![
                task.id.to_string(),
                task.prompt,
                serde_json::to_string(&task.tags)?,
                serde_json::to_string(&task.deps.iter().map(|u| u.to_string()).collect::<Vec<_>>())?,
                task.role.as_str(),
                format!("{:?}", task.status).to_lowercase(),
                task.created_at.to_rfc3339(),
                task.result.as_ref().map(|v| v.to_string()),
            ],
        )?;
        Ok(())
    }
}
