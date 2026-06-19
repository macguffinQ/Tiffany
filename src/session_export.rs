//! Export persisted session logs as shareable text artifacts.

use crate::core::session_store::SessionStore;
use crate::core::types::Session;
use crate::session_display::{self, SessionLogRenderOptions};
use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExportFormat {
    Markdown,
    Html,
}

pub struct SessionExport {
    pub session: Session,
    pub format: SessionExportFormat,
    pub body: String,
    pub path: PathBuf,
}

pub fn render_session_markdown(store: &SessionStore, session: &Session) -> Result<String> {
    let log_path = store.log_path(session.id);
    let body = if log_path.exists() {
        session_display::format_session_log(
            session,
            &log_path,
            SessionLogRenderOptions {
                raw: false,
                tail: None,
            },
        )?
    } else {
        format!(
            "{}\n\nEvents:\n  log file not found: {}",
            session_display::format_session_header(session, &log_path),
            log_path.display()
        )
    };

    Ok(format!(
        "# Tiffany session {}\n\n```text\n{}\n```\n",
        short_session_id(session),
        body
    ))
}

pub fn render_session_html(store: &SessionStore, session: &Session) -> Result<String> {
    let markdown = render_session_markdown(store, session)?;
    Ok(format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>Tiffany session {}</title>\n<style>\n:root {{ color-scheme: light dark; }}\nbody {{ margin: 0; padding: 24px; font-family: -apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif; background: #f6fffd; color: #10201f; }}\nmain {{ max-width: 960px; margin: 0 auto; }}\nh1 {{ margin: 0 0 16px; font-size: 22px; }}\npre {{ margin: 0; padding: 18px; overflow: auto; border-left: 4px solid #0abab5; background: rgba(10, 186, 181, 0.08); white-space: pre-wrap; word-break: break-word; }}\n@media (prefers-color-scheme: dark) {{ body {{ background: #071312; color: #dffcf9; }} pre {{ background: rgba(10, 186, 181, 0.14); }} }}\n</style>\n</head>\n<body>\n<main>\n<h1>Tiffany session {}</h1>\n<pre>{}</pre>\n</main>\n</body>\n</html>\n",
        short_session_id(session),
        short_session_id(session),
        escape_html(&markdown)
    ))
}

pub fn export_session_to_file(
    store: &SessionStore,
    session: &Session,
    format: SessionExportFormat,
) -> Result<SessionExport> {
    let body = match format {
        SessionExportFormat::Markdown => render_session_markdown(store, session)?,
        SessionExportFormat::Html => render_session_html(store, session)?,
    };
    let path = session_export_path(store, session, format);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating export directory {}", parent.display()))?;
    }
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
    Ok(SessionExport {
        session: session.clone(),
        format,
        body,
        path,
    })
}

pub fn session_export_path(
    store: &SessionStore,
    session: &Session,
    format: SessionExportFormat,
) -> PathBuf {
    let ext = match format {
        SessionExportFormat::Markdown => "md",
        SessionExportFormat::Html => "html",
    };
    let ts = session.started_at.format("%Y%m%d-%H%M%S");
    store.log_dir().join("exports").join(format!(
        "session-{}-{}.{}",
        short_session_id(session),
        ts,
        ext
    ))
}

pub fn short_session_id(session: &Session) -> String {
    session.id.to_string().chars().take(8).collect()
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Event, Role};

    #[test]
    fn exports_session_as_markdown_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let mut session = Session::new(uuid::Uuid::new_v4(), "worker-cc", Role::Worker);
        session.ended_at = Some(chrono::Utc::now());
        store.finalize(&session).unwrap();
        store
            .append(&Event {
                session_id: session.id,
                task_id: session.task_id,
                ts: chrono::Utc::now(),
                kind: "assistant".into(),
                payload: serde_json::json!({"text": "done <ok>"}),
            })
            .unwrap();

        let export =
            export_session_to_file(&store, &session, SessionExportFormat::Markdown).unwrap();

        assert_eq!(export.format, SessionExportFormat::Markdown);
        assert_eq!(export.session.id, session.id);
        assert!(export.path.ends_with(format!(
            "session-{}-{}.md",
            short_session_id(&session),
            session.started_at.format("%Y%m%d-%H%M%S")
        )));
        assert!(export.body.contains("# Tiffany session"));
        assert!(export.body.contains("worker-cc"));
        assert!(export.body.contains("done <ok>"));
        assert_eq!(std::fs::read_to_string(export.path).unwrap(), export.body);
    }

    #[test]
    fn exports_session_as_html_with_escaped_body() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let mut session = Session::new(uuid::Uuid::new_v4(), "worker-codex", Role::Worker);
        session.ended_at = Some(chrono::Utc::now());
        store.finalize(&session).unwrap();
        store
            .append(&Event {
                session_id: session.id,
                task_id: session.task_id,
                ts: chrono::Utc::now(),
                kind: "assistant".into(),
                payload: serde_json::json!({"text": "use <script> & check"}),
            })
            .unwrap();

        let export = export_session_to_file(&store, &session, SessionExportFormat::Html).unwrap();

        assert_eq!(export.format, SessionExportFormat::Html);
        assert_eq!(export.session.id, session.id);
        assert!(export.path.extension().unwrap() == "html");
        assert!(export.body.contains("&lt;script&gt;"));
        assert!(export.body.contains("&amp; check"));
        assert!(!export.body.contains("use <script> & check"));
    }
}
