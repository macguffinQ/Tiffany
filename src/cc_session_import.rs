//! Read Claude Code's session JSONL files from `~/.claude/projects/<slug>/`
//! and:
//!  - Format them as context for new CC workers (auto-injected into prompts)
//!  - Import them into the orchestrator's session store (via CLI subcommand)

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::core::session_store::SessionStore;

#[derive(Debug, Clone)]
pub struct CCSession {
    pub id: String,
    pub path: PathBuf,
    pub events: Vec<CCEvent>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub summary: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct CCEvent {
    pub ts: DateTime<Utc>,
    pub kind: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub raw: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct CCImportReport {
    pub project: PathBuf,
    pub discovered: usize,
    pub imported: usize,
    pub backfilled: usize,
    pub skipped: usize,
    pub session_ids: Vec<uuid::Uuid>,
}

/// Find and load all CC session JSONL files for the given project path.
pub fn load_cc_sessions(cwd: &Path) -> Vec<CCSession> {
    let home = match home::home_dir() {
        Some(h) => h,
        None => return vec![],
    };

    // CC uses a slug derived from the project path. The exact algorithm
    // varies; we try the two most common forms.
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let slug_variants = vec![
        canonical.to_string_lossy().replace('/', "-"),
        canonical
            .to_string_lossy()
            .replace('/', "-")
            .trim_start_matches('-')
            .to_string(),
    ];

    let mut paths = vec![];
    let mut seen_paths = HashSet::new();
    for slug in &slug_variants {
        let dir = home.join(".claude").join("projects").join(slug);
        if !dir.exists() {
            continue;
        }
        collect_jsonl_files(&dir, &mut paths, &mut seen_paths);
    }

    paths.sort();
    let mut all = vec![];
    for p in paths {
        if let Ok(sess) = parse_cc_session_file(&p) {
            all.push(sess);
        }
    }

    // Sort newest first, dedupe by id (in case slug variants matched the same project).
    all.sort_by_key(|session| Reverse(session.started_at));
    let mut seen_ids = HashSet::new();
    all.retain(|s| seen_ids.insert(s.id.clone()));
    all
}

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_jsonl_files(&path, out, seen);
        } else if file_type.is_file() && path.extension().and_then(|s| s.to_str()) == Some("jsonl")
        {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if seen.insert(key) {
                out.push(path);
            }
        }
    }
}

fn parse_cc_session_file(path: &Path) -> Result<CCSession> {
    let raw = std::fs::read_to_string(path)?;
    let size_bytes = raw.len() as u64;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut events = vec![];
    let mut summary = None;
    let mut cwd = None;
    let mut git_branch = None;
    let mut model = None;
    let mut started_at = None;
    let mut ended_at = None;

    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(s) = parsed.get("summary").and_then(|v| v.as_str()) {
            summary = Some(s.to_string());
        }
        if let Some(c) = parsed.get("cwd").and_then(|v| v.as_str()) {
            cwd = Some(c.to_string());
        }
        if let Some(g) = parsed.get("gitBranch").and_then(|v| v.as_str()) {
            git_branch = Some(g.to_string());
        }
        if let Some(m) = parsed
            .get("message")
            .and_then(|v| v.get("model"))
            .and_then(|v| v.as_str())
        {
            if model.is_none() {
                model = Some(m.to_string());
            }
        }
        if let Some(dt) = parsed_timestamp(&parsed) {
            if started_at.is_none() {
                started_at = Some(dt);
            }
            ended_at = Some(dt);
        }

        if parsed.get("type").and_then(|v| v.as_str()) == Some("result") {
            if let Some(content) = extract_result_content(&parsed) {
                events.push(CCEvent {
                    ts: parsed_timestamp(&parsed).unwrap_or_else(Utc::now),
                    kind: "result".to_string(),
                    content,
                    tool_name: None,
                    raw: Some(parsed.clone()),
                });
            }
        }

        // Parse message body
        if let Some(msg) = parsed.get("message") {
            let role = msg
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let content = extract_content(msg.get("content"));
            let ts = parsed_timestamp(&parsed).unwrap_or_else(Utc::now);
            if !content.is_empty() {
                events.push(CCEvent {
                    ts,
                    kind: role.to_string(),
                    content,
                    tool_name: None,
                    raw: Some(parsed.clone()),
                });
            }

            // Tool use events
            if let Some(arr) = msg.get("content").and_then(|c| c.as_array()) {
                for c in arr {
                    match c.get("type").and_then(|v| v.as_str()) {
                        Some("tool_use") => {
                            let name = c
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let input = c.get("input").map(|v| v.to_string()).unwrap_or_default();
                            events.push(CCEvent {
                                ts,
                                kind: "tool_use".to_string(),
                                content: format!("{}({})", name, truncate(&input, 200)),
                                tool_name: Some(name),
                                raw: Some(c.clone()),
                            });
                        }
                        Some("tool_result") => {
                            let content = extract_tool_result_content(c);
                            if !content.is_empty() {
                                events.push(CCEvent {
                                    ts,
                                    kind: "tool_result".to_string(),
                                    content,
                                    tool_name: None,
                                    raw: Some(c.clone()),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(CCSession {
        id,
        path: path.to_path_buf(),
        events,
        started_at,
        ended_at,
        summary,
        cwd,
        git_branch,
        model,
        size_bytes,
    })
}

fn parsed_timestamp(parsed: &serde_json::Value) -> Option<DateTime<Utc>> {
    parsed
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

fn extract_content(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|c| {
                if let Some(t) = c.get("type").and_then(|v| v.as_str()) {
                    match t {
                        "text" => c.get("text").and_then(|v| v.as_str()).map(String::from),
                        _ => None,
                    }
                } else {
                    c.as_str().map(String::from)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn extract_tool_result_content(value: &serde_json::Value) -> String {
    let mut content = extract_content(value.get("content"));
    if content.is_empty() {
        content = value
            .get("content")
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default();
    }
    let content = content.trim();
    if content.is_empty() {
        return String::new();
    }

    let prefix = value
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .unwrap_or("tool_result");
    if value
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        format!("{prefix} error: {content}")
    } else {
        format!("{prefix}: {content}")
    }
}

fn extract_result_content(value: &serde_json::Value) -> Option<String> {
    let result = ["result", "content", "summary", "message"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let subtype = value
        .get("subtype")
        .and_then(|v| v.as_str())
        .unwrap_or("result");
    Some(format!("{subtype}: {result}"))
}

/// Format a list of CC sessions as an injectable context block.
/// Bounded by `max_chars`, `max_sessions`, `max_events_per_session`.
pub fn format_cc_sessions_for_context(
    sessions: &[CCSession],
    max_chars: usize,
    max_sessions: usize,
    max_events_per_session: usize,
) -> String {
    use std::fmt::Write as _;
    if sessions.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Prior Claude Code sessions ({} found in this project)\n",
        sessions.len()
    );

    let mut chars = out.len();
    for (i, s) in sessions.iter().take(max_sessions).enumerate() {
        if chars >= max_chars {
            break;
        }

        let mut header = format!("## Session {} ({})\n", i + 1, s.id);
        if let Some(sm) = &s.summary {
            header.push_str(&format!("Summary: {}\n", sm));
        }
        if let Some(t) = s.started_at {
            header.push_str(&format!("Started: {}\n", t.to_rfc3339()));
        }
        if let Some(m) = &s.model {
            header.push_str(&format!("Model: {}\n", m));
        }
        if let Some(b) = &s.git_branch {
            header.push_str(&format!("Branch: {}\n", b));
        }
        header.push('\n');

        chars += header.len();
        if chars >= max_chars {
            break;
        }
        out.push_str(&header);

        for e in s.events.iter().take(max_events_per_session) {
            let line = format!(
                "- [{}] {}: {}\n",
                e.ts.format("%H:%M:%S"),
                e.kind,
                truncate(&e.content, 200)
            );
            chars += line.len();
            if chars >= max_chars {
                out.push_str("  ... (truncated, max chars reached)\n");
                break;
            }
            out.push_str(&line);
        }
        if s.events.len() > max_events_per_session {
            out.push_str(&format!(
                "  ... ({} more events)\n",
                s.events.len() - max_events_per_session
            ));
        }
        out.push('\n');
    }

    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub fn cc_session_uuid(id: &str) -> uuid::Uuid {
    use uuid::Uuid;
    Uuid::parse_str(id).unwrap_or_else(|_| {
        let first = stable_hash64(0xcbf29ce484222325, id.as_bytes());
        let second = stable_hash64(0x84222325cbf29ce4, id.as_bytes());
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&first.to_be_bytes());
        bytes[8..].copy_from_slice(&second.to_be_bytes());
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    })
}

fn stable_hash64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    for b in b"claude-code-session:" {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn import_cc_sessions(store: &SessionStore, cwd: &Path) -> Result<CCImportReport> {
    let sessions = load_cc_sessions(cwd);
    let label = cwd.display().to_string();
    let mut report = CCImportReport {
        project: cwd.to_path_buf(),
        discovered: sessions.len(),
        ..CCImportReport::default()
    };

    for cc in &sessions {
        let session_id = cc_session_uuid(&cc.id);
        let existing = store.get_many(&[session_id])?.into_iter().next();
        let had_existing = existing.is_some();
        let log_path = store.log_path(session_id);
        let log_has_content = log_path
            .metadata()
            .map(|meta| meta.len() > 0)
            .unwrap_or(false);

        if had_existing && log_has_content {
            report.skipped += 1;
            continue;
        }

        let session = match existing {
            Some(session) => session,
            None => {
                let session = to_orchestrator_session(cc, &label);
                store.finalize(&session)?;
                report.imported += 1;
                session
            }
        };

        if !log_has_content {
            for event in to_orchestrator_events(cc, &session) {
                store.append(&event)?;
            }
            if had_existing {
                report.backfilled += 1;
            }
        }
        report.session_ids.push(session.id);
    }

    Ok(report)
}

/// Convert a CC session into the orchestrator's Session struct for import.
pub fn to_orchestrator_session(cc: &CCSession, project_label: &str) -> crate::core::types::Session {
    use crate::core::types::{Role, Session};
    use uuid::Uuid;
    let id = cc_session_uuid(&cc.id);
    // Use zero UUID for task_id (these are pre-existing sessions, no task)
    Session {
        id,
        task_id: Uuid::nil(),
        agent: format!("claude-code-imported ({})", project_label),
        role: Role::Worker,
        model: cc.model.clone().unwrap_or_default(),
        started_at: cc.started_at.unwrap_or_else(Utc::now),
        ended_at: cc.ended_at,
        parent_session_ids: vec![],
        worker_thread_id: None,
        native_session_id: Some(cc.id.clone()),
        worktree_path: None,
        token_in: 0,
        token_out: 0,
        cost_usd: 0.0,
        files_touched: vec![cc.path.to_string_lossy().to_string()],
    }
}

pub fn to_orchestrator_events(
    cc: &CCSession,
    session: &crate::core::types::Session,
) -> Vec<crate::core::types::Event> {
    use crate::core::types::Event;

    let mut events = Vec::with_capacity(cc.events.len() + 1);
    let base_ts = cc.started_at.unwrap_or_else(Utc::now);
    events.push(Event {
        session_id: session.id,
        task_id: session.task_id,
        ts: base_ts,
        kind: "metadata".to_string(),
        payload: serde_json::json!({
            "source": "claude-code",
            "imported": true,
            "cc_session_id": cc.id.clone(),
            "path": cc.path.display().to_string(),
            "summary": cc.summary.clone(),
            "cwd": cc.cwd.clone(),
            "git_branch": cc.git_branch.clone(),
            "model": cc.model.clone(),
            "size_bytes": cc.size_bytes,
        }),
    });

    for (index, event) in cc.events.iter().enumerate() {
        events.push(Event {
            session_id: session.id,
            task_id: session.task_id,
            ts: event.ts + Duration::microseconds((index + 1) as i64),
            kind: event.kind.clone(),
            payload: serde_json::json!({
                "source": "claude-code",
                "imported": true,
                "cc_session_id": cc.id.clone(),
                "kind": event.kind.clone(),
                "content": event.content.clone(),
                "tool_name": event.tool_name.clone(),
                "raw": event.raw.clone(),
            }),
        });
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write_cc_session(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(format!("{}.jsonl", name));
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parse_minimal_session() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"type":"summary","summary":"Test summary","cwd":"/tmp"}
{"type":"user","timestamp":"2026-06-07T10:00:00Z","message":{"role":"user","content":"Hello"}}
{"type":"assistant","timestamp":"2026-06-07T10:00:05Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi there"}]}}
"#;
        let path = write_cc_session(dir.path(), "test-session", content);
        let sess = parse_cc_session_file(&path).unwrap();
        assert_eq!(sess.summary.as_deref(), Some("Test summary"));
        assert_eq!(sess.cwd.as_deref(), Some("/tmp"));
        assert_eq!(sess.events.len(), 2);
        assert_eq!(sess.events[0].kind, "user");
        assert_eq!(sess.events[0].content, "Hello");
        assert_eq!(sess.events[1].content, "Hi there");
    }

    #[test]
    fn format_for_context_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let mut content = String::from(r#"{"type":"summary","summary":"X"}"#);
        for i in 0..50 {
            content.push_str(&format!(
                "\n{{\"type\":\"user\",\"timestamp\":\"2026-06-07T10:00:{:02}Z\",\"message\":{{\"role\":\"user\",\"content\":\"msg {}\"}}}}",
                i % 60, i
            ));
        }
        let path = write_cc_session(dir.path(), "big", &content);
        let sess = parse_cc_session_file(&path).unwrap();
        let sessions = vec![sess];
        let out = format_cc_sessions_for_context(&sessions, 1000, 5, 20);
        // Should be truncated to <= ~1000 chars
        assert!(out.len() <= 1100);
        assert!(out.contains("msg 0"));
    }

    #[test]
    fn empty_sessions_returns_empty() {
        let out = format_cc_sessions_for_context(&[], 1000, 5, 20);
        assert!(out.is_empty());
    }

    #[test]
    fn collect_jsonl_files_includes_project_root_and_nested_agents() {
        let dir = tempfile::tempdir().unwrap();
        let root_log = write_cc_session(dir.path(), "root-session", "{}");
        let nested = dir.path().join("subagents").join("workflow");
        fs::create_dir_all(&nested).unwrap();
        let nested_log = write_cc_session(&nested, "agent-session", "{}");
        fs::write(dir.path().join("ignore.txt"), "not jsonl").unwrap();

        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        collect_jsonl_files(dir.path(), &mut paths, &mut seen);
        paths.sort();

        assert_eq!(paths, vec![root_log, nested_log]);
    }

    #[test]
    fn truncate_handles_unicode() {
        assert_eq!(truncate("hello", 100), "hello");
        assert!(truncate("中文中文中文", 5).ends_with('…'));
    }

    #[test]
    fn imported_events_have_unique_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"type":"assistant","timestamp":"2026-06-07T10:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"I'll inspect it"},{"type":"tool_use","name":"Read","input":{"file_path":"README.md"}}]}}"#;
        let path = write_cc_session(dir.path(), "test-session", content);
        let cc = parse_cc_session_file(&path).unwrap();
        let session = to_orchestrator_session(&cc, "test");
        let events = to_orchestrator_events(&cc, &session);
        assert_eq!(events.len(), 3);
        assert!(events[1].ts < events[2].ts);
        assert_eq!(events[2].kind, "tool_use");
    }

    #[test]
    fn parse_captures_tool_results_and_result_lines() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"type":"assistant","timestamp":"2026-06-07T10:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo test"}}]}}
{"type":"user","timestamp":"2026-06-07T10:00:01Z","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"tests passed","is_error":false}]}}
{"type":"result","subtype":"success","timestamp":"2026-06-07T10:00:02Z","result":"done"}
"#;
        let path = write_cc_session(dir.path(), "test-session", content);
        let cc = parse_cc_session_file(&path).unwrap();
        let kinds = cc
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();

        assert_eq!(kinds, vec!["tool_use", "tool_result", "result"]);
        assert!(cc.events[1].content.contains("tests passed"));
        assert_eq!(cc.events[2].content, "success: done");
    }

    #[test]
    fn cc_session_uuid_is_stable_for_non_uuid_ids() {
        let first = cc_session_uuid("agent-af59be3fe8d92c711");
        let second = cc_session_uuid("agent-af59be3fe8d92c711");
        let other = cc_session_uuid("agent-different");

        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    #[test]
    fn imported_events_are_written_to_store_log() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("state.sqlite")).unwrap();
        let cc_dir = tempfile::tempdir().unwrap();
        let content = r#"{"type":"summary","summary":"Imported summary","cwd":"/tmp/project"}
{"type":"user","timestamp":"2026-06-07T10:00:00Z","message":{"role":"user","content":"Hello"}}
{"type":"assistant","timestamp":"2026-06-07T10:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}]}}
"#;
        let path = write_cc_session(cc_dir.path(), "test-session", content);
        let cc = parse_cc_session_file(&path).unwrap();
        let session = to_orchestrator_session(&cc, "test");
        store.finalize(&session).unwrap();
        for event in to_orchestrator_events(&cc, &session) {
            store.append(&event).unwrap();
        }

        let log = fs::read_to_string(store.log_path(session.id)).unwrap();
        assert!(log.contains("Imported summary"));
        assert!(log.contains("Hello"));
        assert!(log.contains("Hi"));
    }
}
