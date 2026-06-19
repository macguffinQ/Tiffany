//! Human-readable rendering for persisted session logs.

use crate::agent_events;
use crate::core::types::{Event, Session};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::path::PathBuf;

const EVENT_SUMMARY_MAX_CHARS: usize = 4_000;
const RAW_LINE_MAX_CHARS: usize = 1_000;

pub struct SessionLogRenderOptions {
    pub raw: bool,
    pub tail: Option<usize>,
}

pub struct SessionFlowRenderOptions {
    pub tail_per_session: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionListActionStyle {
    Slash,
    Cli,
}

pub struct SessionListRenderOptions {
    pub action_style: SessionListActionStyle,
}

pub fn format_session_log(
    session: &Session,
    log_path: &Path,
    options: SessionLogRenderOptions,
) -> Result<String> {
    let content = std::fs::read_to_string(log_path)
        .with_context(|| format!("reading session log {}", log_path.display()))?;
    let lines = content.lines().collect::<Vec<_>>();
    let visible_lines = tail_slice(&lines, options.tail);

    let mut out = format_session_header(session, log_path);
    if options.raw {
        out.push_str(&format!(
            "\n\nRaw events ({} of {} line(s)):",
            visible_lines.len(),
            lines.len()
        ));
        for line in visible_lines {
            out.push('\n');
            out.push_str(line);
        }
        return Ok(out);
    }

    let range_label = if options.tail.is_some() {
        format!(
            "last {} of {} raw line(s)",
            visible_lines.len(),
            lines.len()
        )
    } else {
        format!("{} raw line(s)", lines.len())
    };
    out.push_str(&format!(
        "\n\nReadable events ({range_label}; low-value duplicates hidden):"
    ));
    if visible_lines.is_empty() {
        out.push_str("\n  (empty)");
        return Ok(out);
    }
    let mut seen_output_keys = HashSet::new();
    let mut emitted = 0usize;
    for line in visible_lines {
        if should_skip_human_event_line(line, &mut seen_output_keys) {
            continue;
        }
        out.push('\n');
        out.push_str(&indent_block(&format_session_event_line(line), "  "));
        emitted += 1;
    }
    if emitted == 0 {
        out.push_str("\n  (only low-value duplicate events in this range; use --raw to inspect)");
    }
    Ok(out)
}

pub fn format_session_header(session: &Session, log_path: &Path) -> String {
    let state = if session.ended_at.is_some() {
        "done"
    } else {
        "running"
    };
    let model = if session.model.trim().is_empty() {
        "(none)"
    } else {
        session.model.as_str()
    };
    let ended = session
        .ended_at
        .map(|time| time.to_rfc3339())
        .unwrap_or_else(|| "(running)".to_string());
    let files = if session.files_touched.is_empty() {
        "none".to_string()
    } else {
        truncate_chars(&session.files_touched.join(", "), 240)
    };
    let parents = if session.parent_session_ids.is_empty() {
        "none".to_string()
    } else {
        session
            .parent_session_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "Session {}\n  task: {}\n  parents: {}\n  state: {}\n  agent: {}\n  role: {}\n  model: {}\n  started: {}\n  ended: {}\n  tokens: in={} out={} total={}\n  cost: ${:.4}\n  files: {}\n  log: {}",
        session.id,
        session.task_id,
        parents,
        state,
        session.agent,
        session.role.as_str(),
        model,
        session.started_at.to_rfc3339(),
        ended,
        session.token_in,
        session.token_out,
        session.token_in + session.token_out,
        session.cost_usd,
        files,
        log_path.display()
    )
}

pub fn format_session_list(display_sessions: &[Session], all_sessions: &[Session]) -> String {
    format_session_list_with_options(
        display_sessions,
        all_sessions,
        SessionListRenderOptions {
            action_style: SessionListActionStyle::Slash,
        },
    )
}

pub fn format_session_list_with_options(
    display_sessions: &[Session],
    all_sessions: &[Session],
    options: SessionListRenderOptions,
) -> String {
    if all_sessions.is_empty() {
        return "No sessions yet.".to_string();
    }

    let mut child_counts = HashMap::new();
    for session in all_sessions {
        for parent_id in &session.parent_session_ids {
            *child_counts.entry(*parent_id).or_insert(0usize) += 1;
        }
    }

    let mut out = if display_sessions.len() == all_sessions.len() {
        format!("Recent sessions ({})", display_sessions.len())
    } else {
        format!(
            "Recent sessions (showing {} of {})",
            display_sessions.len(),
            all_sessions.len()
        )
    };
    out.push_str("\n  id       state     role          agent            started       relation       actions");

    if display_sessions.is_empty() {
        out.push_str("\n  (none shown; increase the limit to see sessions)");
    }

    for session in display_sessions {
        let children = child_counts.get(&session.id).copied().unwrap_or(0);
        out.push('\n');
        out.push_str("  ");
        out.push_str(&format_session_list_row(
            session,
            children,
            options.action_style,
        ));
    }

    out.push_str(match options.action_style {
        SessionListActionStyle::Slash => {
            "\n\nOpen: /run-flow <id> 120 for the run waterfall; /tree <id> for links; /log <id> 120 for readable events."
        }
        SessionListActionStyle::Cli => {
            "\n\nOpen: orchestrator sessions show <id> --flow --tail 120; add --tree for links or --raw for JSONL."
        }
    });
    out
}

pub fn format_session_tree(root: &Session, all_sessions: &[Session], log_dir: &Path) -> String {
    let mut children = all_sessions
        .iter()
        .filter(|session| session.parent_session_ids.contains(&root.id))
        .collect::<Vec<_>>();
    children.sort_by(|a, b| a.started_at.cmp(&b.started_at));

    let mut parents = all_sessions
        .iter()
        .filter(|session| root.parent_session_ids.contains(&session.id))
        .collect::<Vec<_>>();
    parents.sort_by(|a, b| a.started_at.cmp(&b.started_at));

    let mut out = format!(
        "Session tree\n{}\n  task: {}\n  state: {}\n  log: {}",
        format_tree_session_line(root),
        root.task_id,
        session_state(root),
        session_log_path(log_dir, root).display()
    );

    if !parents.is_empty() {
        out.push_str("\n\nParents:");
        for parent in parents {
            out.push_str("\n  ");
            out.push_str(&format_tree_session_line(parent));
            out.push_str(&format!("  /session tree {}", short_session_id(parent)));
        }
    }

    out.push_str(&format!("\n\nChildren ({}):", children.len()));
    if children.is_empty() {
        out.push_str("\n  (none)");
    } else {
        for (idx, child) in children.iter().enumerate() {
            let branch = if idx + 1 == children.len() {
                "└─"
            } else {
                "├─"
            };
            out.push_str(&format!(
                "\n  {} {}  /log {} 120",
                branch,
                format_tree_session_line(child),
                short_session_id(child)
            ));
        }
    }

    out
}

pub fn format_session_flow(
    selected: &Session,
    all_sessions: &[Session],
    log_dir: &Path,
    options: SessionFlowRenderOptions,
) -> String {
    let root = flow_root_session(selected, all_sessions);
    let sessions = collect_session_subtree(root, all_sessions);
    let mut out = format!(
        "Session flow\n{}\n  selected: {}\n  sessions: {}\n  events: {}",
        format_tree_session_line(root),
        if selected.id == root.id {
            "root".to_string()
        } else {
            format!(
                "{} via root {}",
                short_session_id(selected),
                short_session_id(root)
            )
        },
        sessions.len(),
        match options.tail_per_session {
            Some(limit) => format!("last {limit} raw line(s) per session; duplicates hidden"),
            None => "all readable events; duplicates hidden".to_string(),
        }
    );

    for (idx, session) in sessions.iter().enumerate() {
        let marker = if idx == 0 { "●" } else { "↳" };
        out.push_str(&format!(
            "\n\n{} {}  /tree {}  /log {} 120\n  log: {}",
            marker,
            format_tree_session_line(session),
            short_session_id(session),
            short_session_id(session),
            session_log_path(log_dir, session).display()
        ));
        out.push_str(&format_flow_session_events(
            session,
            log_dir,
            options.tail_per_session,
        ));
    }

    out
}

pub fn format_session_event(event: &Event) -> String {
    if event.kind == "heartbeat" {
        return format!("{} heartbeat", event.ts.format("%H:%M:%S"));
    }
    if let Some(line) = format_imported_claude_event(event) {
        return line;
    }

    let ts = event.ts.format("%H:%M:%S").to_string();
    let summary = agent_events::summarize_event_payload(&event.payload, EVENT_SUMMARY_MAX_CHARS);
    if summary.trim().is_empty() {
        format!("{} {}", ts, event.kind)
    } else {
        format_multiline_log_event(&ts, &event.kind, &summary)
    }
}

pub fn format_session_event_line(raw: &str) -> String {
    match serde_json::from_str::<Event>(raw) {
        Ok(event) => format_session_event(&event),
        Err(_) => truncate_chars(raw, RAW_LINE_MAX_CHARS),
    }
}

fn flow_root_session<'a>(selected: &'a Session, all_sessions: &'a [Session]) -> &'a Session {
    let by_id = all_sessions
        .iter()
        .map(|session| (session.id, session))
        .collect::<HashMap<_, _>>();
    let mut current = by_id.get(&selected.id).copied().unwrap_or(selected);
    let mut visited = HashSet::new();

    loop {
        if !visited.insert(current.id) {
            break;
        }
        let Some(parent) = current
            .parent_session_ids
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .min_by(|a, b| a.started_at.cmp(&b.started_at))
        else {
            break;
        };
        if visited.contains(&parent.id) {
            break;
        }
        current = parent;
    }

    current
}

fn collect_session_subtree<'a>(root: &'a Session, all_sessions: &'a [Session]) -> Vec<&'a Session> {
    let mut children_by_parent = HashMap::<uuid::Uuid, Vec<&Session>>::new();
    for session in all_sessions {
        for parent_id in &session.parent_session_ids {
            children_by_parent
                .entry(*parent_id)
                .or_default()
                .push(session);
        }
    }
    for children in children_by_parent.values_mut() {
        children.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    }

    let mut out = Vec::new();
    let mut visited = HashSet::new();
    collect_session_subtree_into(root, &children_by_parent, &mut visited, &mut out);
    out
}

fn collect_session_subtree_into<'a>(
    session: &'a Session,
    children_by_parent: &HashMap<uuid::Uuid, Vec<&'a Session>>,
    visited: &mut HashSet<uuid::Uuid>,
    out: &mut Vec<&'a Session>,
) {
    if !visited.insert(session.id) {
        return;
    }
    out.push(session);
    if let Some(children) = children_by_parent.get(&session.id) {
        for child in children {
            collect_session_subtree_into(child, children_by_parent, visited, out);
        }
    }
}

fn format_flow_session_events(
    session: &Session,
    log_dir: &Path,
    tail_per_session: Option<usize>,
) -> String {
    let path = session_log_path(log_dir, session);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => return format!("\n  events:\n    log unavailable: {err}"),
    };
    let lines = content.lines().collect::<Vec<_>>();
    let visible_lines = tail_slice(&lines, tail_per_session);
    let range = if tail_per_session.is_some() {
        format!(
            "last {} of {} raw line(s)",
            visible_lines.len(),
            lines.len()
        )
    } else {
        format!("{} raw line(s)", lines.len())
    };
    let mut out = format!("\n  events ({range}):");
    if visible_lines.is_empty() {
        out.push_str("\n    (empty)");
        return out;
    }

    let mut seen_output_keys = HashSet::new();
    let mut emitted = 0usize;
    for raw in visible_lines {
        if should_skip_human_event_line(raw, &mut seen_output_keys) {
            continue;
        }
        out.push('\n');
        out.push_str(&indent_block(&format_session_event_line(raw), "    "));
        emitted += 1;
    }
    if emitted == 0 {
        out.push_str("\n    (only low-value duplicate events in this range)");
    }

    out
}

fn format_session_list_row(
    session: &Session,
    child_count: usize,
    action_style: SessionListActionStyle,
) -> String {
    let short = short_session_id(session);
    format!(
        "{:<8} {:<9} {:<12} {:<16} {:<13} {:<14} {}",
        short,
        format!("{} {}", session_state_icon(session), session_state(session)),
        session.role.as_str(),
        truncate_chars(
            if session.agent.trim().is_empty() {
                "(agent)"
            } else {
                &session.agent
            },
            16
        ),
        session.started_at.format("%m-%d %H:%M"),
        format_session_relation(session, child_count),
        format_session_list_actions(&short, action_style),
    )
}

fn format_session_list_actions(short: &str, action_style: SessionListActionStyle) -> String {
    match action_style {
        SessionListActionStyle::Slash => format!("/run-flow {short} 120  /tree {short}"),
        SessionListActionStyle::Cli => {
            format!("orchestrator sessions show {short} --flow --tail 120")
        }
    }
}

fn format_session_relation(session: &Session, child_count: usize) -> String {
    match (session.parent_session_ids.len(), child_count) {
        (0, 0) => "solo".to_string(),
        (0, children) => format!("children={children}"),
        (1, 0) => format!("parent={}", short_uuid(&session.parent_session_ids[0])),
        (parents, 0) => format!("parents={parents}"),
        (1, children) => format!(
            "p={} c={children}",
            short_uuid(&session.parent_session_ids[0])
        ),
        (parents, children) => format!("p={parents} c={children}"),
    }
}

fn format_tree_session_line(session: &Session) -> String {
    format!(
        "{} {} {:<12} {:<16} {}",
        short_session_id(session),
        session_state_icon(session),
        session.role.as_str(),
        truncate_chars(
            if session.agent.trim().is_empty() {
                "(agent)"
            } else {
                &session.agent
            },
            16
        ),
        session.started_at.format("%Y-%m-%d %H:%M:%S")
    )
}

fn short_session_id(session: &Session) -> String {
    session.id.to_string().chars().take(8).collect()
}

fn short_uuid(id: &uuid::Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn session_state(session: &Session) -> &'static str {
    if session.ended_at.is_some() {
        "done"
    } else {
        "running"
    }
}

fn session_state_icon(session: &Session) -> &'static str {
    if session.ended_at.is_some() {
        "✓"
    } else {
        "●"
    }
}

fn session_log_path(log_dir: &Path, session: &Session) -> PathBuf {
    log_dir.join(format!("{}.jsonl", session.id))
}

fn format_imported_claude_event(event: &Event) -> Option<String> {
    if event.payload.get("source").and_then(|v| v.as_str()) != Some("claude-code") {
        return None;
    }

    let ts = event.ts.format("%H:%M:%S").to_string();
    if event.kind == "metadata" {
        let summary = event
            .payload
            .get("summary")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("imported Claude Code session");
        let cwd = event
            .payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        let body = match cwd {
            Some(cwd) => format!("{summary}\nproject: {cwd}"),
            None => summary.to_string(),
        };
        return Some(format_multiline_log_event(&ts, "claude-code", &body));
    }

    let content = event
        .payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let body = if content.trim().is_empty() {
        agent_events::humanize_jsonish(&event.payload.to_string(), EVENT_SUMMARY_MAX_CHARS)
    } else {
        agent_events::humanize_jsonish(content, EVENT_SUMMARY_MAX_CHARS)
    };
    if body.trim().is_empty() {
        return Some(format!("{} {}", ts, event.kind));
    }

    let label = match event.kind.as_str() {
        "tool_use" => "tool",
        "tool_result" => "tool result",
        "result" => "result",
        other => other,
    };
    Some(format_multiline_log_event(&ts, label, &body))
}

fn should_skip_human_event_line(raw: &str, seen_output_keys: &mut HashSet<String>) -> bool {
    let Ok(event) = serde_json::from_str::<Event>(raw) else {
        return false;
    };
    if is_low_value_human_event(&event) {
        return true;
    }

    if let Some(key) = human_event_dedupe_key(&event) {
        return !seen_output_keys.insert(key);
    }
    false
}

pub fn is_low_value_human_event(event: &Event) -> bool {
    if event.kind == "heartbeat" {
        return true;
    }
    let summary = session_event_summary_for_key(event);
    is_low_value_session_event(&event.kind, &summary)
}

pub fn human_event_dedupe_key(event: &Event) -> Option<String> {
    let summary = session_event_summary_for_key(event);
    let key_input = if summary.trim().is_empty() {
        event.kind.clone()
    } else {
        format!("agent {}: {}", event.kind, summary)
    };
    agent_events::normalized_output_key(&key_input, 240_000)
}

fn session_event_summary_for_key(event: &Event) -> String {
    if event.payload.get("source").and_then(|v| v.as_str()) == Some("claude-code") {
        if let Some(content) = event.payload.get("content").and_then(|v| v.as_str()) {
            return agent_events::humanize_jsonish(content, EVENT_SUMMARY_MAX_CHARS);
        }
    }
    agent_events::summarize_event_payload(&event.payload, EVENT_SUMMARY_MAX_CHARS)
}

fn is_low_value_session_event(kind: &str, summary: &str) -> bool {
    let lower = summary.trim().to_ascii_lowercase();
    if matches!(
        (kind, lower.as_str()),
        ("system", "system")
            | ("assistant", "assistant")
            | ("assistant", "thinking")
            | ("result", "")
    ) {
        return true;
    }

    let display = if summary.trim().is_empty() {
        kind.to_string()
    } else {
        format!("{kind} {summary}")
    };
    agent_events::is_low_value_output(&display)
}

fn format_multiline_log_event(ts: &str, label: &str, body: &str) -> String {
    let mut lines = body.trim_end().lines();
    let Some(first) = lines.next() else {
        return format!("{ts} {label}");
    };

    let mut out = format!("{ts} {label} {first}");
    for line in lines {
        out.push('\n');
        out.push_str("    ");
        out.push_str(line);
    }
    out
}

fn tail_slice<'a>(lines: &'a [&'a str], tail: Option<usize>) -> &'a [&'a str] {
    match tail {
        Some(0) => &[],
        Some(limit) => {
            let start = lines.len().saturating_sub(limit);
            &lines[start..]
        }
        None => lines,
    }
}

fn indent_block(text: &str, prefix: &str) -> String {
    if text.is_empty() {
        return prefix.to_string();
    }
    text.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Role, Session};
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn formats_json_event_without_raw_json() {
        let event = Event {
            session_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            ts: Utc::now(),
            kind: "assistant".into(),
            payload: serde_json::json!({
                "type": "assistant",
                "message": {
                    "content": [{ "type": "text", "text": "hello from worker" }]
                }
            }),
        };

        let line = format_session_event(&event);

        assert!(line.contains("assistant hello from worker"));
        assert!(!line.contains("\"message\""));
    }

    #[test]
    fn formats_review_json_as_readable_issues() {
        let raw = serde_json::to_string(&Event {
            session_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            ts: Utc::now(),
            kind: "reviewer".into(),
            payload: serde_json::json!({
                "approved": false,
                "issues": ["worker output missing"]
            }),
        })
        .unwrap();

        let line = format_session_event_line(&raw);

        assert!(line.contains("needs changes: 1 issue(s)"));
        assert!(line.contains("worker output missing"));
        assert!(!line.contains("\"approved\""));
    }

    #[test]
    fn renders_session_log_with_tail_and_raw_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.jsonl");
        let session = Session::new(Uuid::new_v4(), "claude-code", Role::Worker);
        let event_one = Event {
            session_id: session.id,
            task_id: session.task_id,
            ts: Utc::now(),
            kind: "assistant".into(),
            payload: serde_json::json!({"text": "first"}),
        };
        let event_two = Event {
            session_id: session.id,
            task_id: session.task_id,
            ts: Utc::now(),
            kind: "assistant".into(),
            payload: serde_json::json!({"text": "second"}),
        };
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&event_one).unwrap(),
                serde_json::to_string(&event_two).unwrap()
            ),
        )
        .unwrap();

        let human = format_session_log(
            &session,
            &path,
            SessionLogRenderOptions {
                raw: false,
                tail: Some(1),
            },
        )
        .unwrap();
        assert!(human.contains("Readable events (last 1 of 2 raw line(s)"));
        assert!(human.contains("second"));
        assert!(!human.contains("first"));

        let raw = format_session_log(
            &session,
            &path,
            SessionLogRenderOptions {
                raw: true,
                tail: Some(1),
            },
        )
        .unwrap();
        assert!(raw.contains("Raw events (1 of 2 line(s)):"));
        assert!(raw.contains("\"second\""));
    }

    #[test]
    fn human_session_log_skips_noise_and_duplicate_final_output() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.jsonl");
        let session = Session::new(Uuid::new_v4(), "claude-code", Role::Worker);
        let system = Event {
            session_id: session.id,
            task_id: session.task_id,
            ts: Utc::now(),
            kind: "system".into(),
            payload: serde_json::json!({"type": "system", "text": "system"}),
        };
        let assistant = Event {
            session_id: session.id,
            task_id: session.task_id,
            ts: Utc::now(),
            kind: "assistant".into(),
            payload: serde_json::json!({
                "type": "assistant",
                "message": {
                    "content": [{ "type": "text", "text": "final answer" }]
                }
            }),
        };
        let result = Event {
            session_id: session.id,
            task_id: session.task_id,
            ts: Utc::now(),
            kind: "result".into(),
            payload: serde_json::json!({"result": "final answer"}),
        };
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&system).unwrap(),
                serde_json::to_string(&assistant).unwrap(),
                serde_json::to_string(&result).unwrap()
            ),
        )
        .unwrap();

        let human = format_session_log(
            &session,
            &path,
            SessionLogRenderOptions {
                raw: false,
                tail: None,
            },
        )
        .unwrap();

        assert!(!human.contains("system system"));
        assert_eq!(human.matches("final answer").count(), 1);
    }

    #[test]
    fn formats_session_list_with_tree_and_log_shortcuts() {
        let mut root = Session::new(Uuid::new_v4(), "orchestrator", Role::Orchestrator);
        root.ended_at = Some(Utc::now());
        let mut worker = Session::new(Uuid::new_v4(), "claude-code", Role::Worker);
        worker.parent_session_ids.push(root.id);
        worker.ended_at = Some(Utc::now());

        let list = format_session_list(&[root.clone(), worker.clone()], &[root, worker]);

        assert!(list.contains("Recent sessions (2)"));
        assert!(list.contains("orchestrator"));
        assert!(list.contains("children=1"));
        assert!(list.contains("worker"));
        assert!(list.contains("parent="));
        assert!(list.contains("/tree"));
        assert!(list.contains("/log"));
        assert!(list.contains("/run-flow"));
    }

    #[test]
    fn formats_cli_session_list_with_shell_commands() {
        let mut session = Session::new(Uuid::new_v4(), "claude-code", Role::Worker);
        session.ended_at = Some(Utc::now());

        let list = format_session_list_with_options(
            &[session.clone()],
            &[session],
            SessionListRenderOptions {
                action_style: SessionListActionStyle::Cli,
            },
        );

        assert!(list.contains("orchestrator sessions show"));
        assert!(list.contains("--flow --tail 120"));
        assert!(list.contains("--tree"));
        assert!(list.contains("--raw"));
        assert!(!list.contains("/run-flow"));
    }

    #[test]
    fn formats_session_tree_with_children_and_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let mut root = Session::new(Uuid::new_v4(), "orchestrator", Role::Orchestrator);
        root.ended_at = Some(Utc::now());
        let mut worker = Session::new(Uuid::new_v4(), "claude-code", Role::Worker);
        worker.parent_session_ids.push(root.id);
        worker.ended_at = Some(Utc::now());

        let tree = format_session_tree(&root, &[root.clone(), worker.clone()], tmp.path());

        assert!(tree.contains("Session tree"));
        assert!(tree.contains("orchestrator"));
        assert!(tree.contains("Children (1):"));
        assert!(tree.contains("worker"));
        assert!(tree.contains("/log"));

        let worker_tree = format_session_tree(&worker, &[root, worker.clone()], tmp.path());
        assert!(worker_tree.contains("Parents:"));
        assert!(worker_tree.contains("/session tree"));
    }

    #[test]
    fn formats_session_flow_from_worker_back_to_root() {
        let tmp = tempfile::tempdir().unwrap();
        let mut root = Session::new(Uuid::new_v4(), "orchestrator", Role::Orchestrator);
        root.ended_at = Some(Utc::now());
        let mut worker = Session::new(Uuid::new_v4(), "claude-code", Role::Worker);
        worker.parent_session_ids.push(root.id);
        worker.ended_at = Some(Utc::now());

        let root_event = Event {
            session_id: root.id,
            task_id: root.task_id,
            ts: Utc::now(),
            kind: "planner".into(),
            payload: serde_json::json!({"message": "plan ready"}),
        };
        let worker_event = Event {
            session_id: worker.id,
            task_id: worker.task_id,
            ts: Utc::now(),
            kind: "assistant".into(),
            payload: serde_json::json!({
                "type": "assistant",
                "message": {
                    "content": [{ "type": "text", "text": "worker finished" }]
                }
            }),
        };
        std::fs::write(
            session_log_path(tmp.path(), &root),
            format!("{}\n", serde_json::to_string(&root_event).unwrap()),
        )
        .unwrap();
        std::fs::write(
            session_log_path(tmp.path(), &worker),
            format!("{}\n", serde_json::to_string(&worker_event).unwrap()),
        )
        .unwrap();

        let flow = format_session_flow(
            &worker,
            &[root.clone(), worker.clone()],
            tmp.path(),
            SessionFlowRenderOptions {
                tail_per_session: Some(20),
            },
        );

        assert!(flow.contains("Session flow"));
        assert!(flow.contains("selected:"));
        assert!(flow.contains("orchestrator"));
        assert!(flow.contains("claude-code"));
        assert!(flow.contains("plan ready"));
        assert!(flow.contains("worker finished"));
        assert!(!flow.contains("\"message\""));
    }
}
