//! Human-readable rendering for persisted session logs.

use crate::agent_events;
use crate::core::types::{Event, Session};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;

const EVENT_SUMMARY_MAX_CHARS: usize = 4_000;
const RAW_LINE_MAX_CHARS: usize = 1_000;

pub struct SessionLogRenderOptions {
    pub raw: bool,
    pub tail: Option<usize>,
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
}
