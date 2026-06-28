//! Worker adapters: one per agent runtime.

use crate::core::types::Event;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub mod claude_code;
pub mod codex_cli;
pub mod direct_api;
pub mod gemini_cli;

#[derive(Clone, Default)]
pub(crate) struct StderrCapture {
    lines: Arc<Mutex<Vec<String>>>,
}

impl StderrCapture {
    pub(crate) fn push(&self, line: String) {
        let Ok(mut lines) = self.lines.lock() else {
            return;
        };
        if lines.len() >= 40 {
            lines.remove(0);
        }
        lines.push(line);
    }

    pub(crate) fn error_suffix(&self) -> String {
        let Ok(lines) = self.lines.lock() else {
            return String::new();
        };
        let mut recent = lines
            .iter()
            .rev()
            .filter_map(|line| {
                let line = line.trim();
                (!line.is_empty()).then_some(line.to_string())
            })
            .take(8)
            .collect::<Vec<_>>();
        recent.reverse();
        if recent.is_empty() {
            return String::new();
        }
        format!("; stderr: {}", truncate_stderr(&recent.join(" | "), 800))
    }
}

fn truncate_stderr(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return value.to_string();
        };
        out.push(ch);
    }
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

pub(crate) fn stream_cli_session_events(
    path: PathBuf,
    session_id: Uuid,
    task_id: Uuid,
    classify: fn(&str) -> String,
) -> BoxStream<'static, anyhow::Result<Event>> {
    #[derive(Debug)]
    struct State {
        path: PathBuf,
        pos: u64,
        pending: VecDeque<Event>,
    }

    fn heartbeat(session_id: Uuid, task_id: Uuid) -> Event {
        Event {
            session_id,
            task_id,
            ts: Utc::now(),
            kind: "heartbeat".into(),
            payload: serde_json::json!({}),
        }
    }

    Box::pin(stream::unfold(
        State {
            path,
            pos: 0,
            pending: VecDeque::new(),
        },
        move |mut state| async move {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};

            if let Some(event) = state.pending.pop_front() {
                return Some((Ok(event), state));
            }

            let mut file = match tokio::fs::File::open(&state.path).await {
                Ok(file) => file,
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    return Some((Ok(heartbeat(session_id, task_id)), state));
                }
            };
            if file
                .seek(std::io::SeekFrom::Start(state.pos))
                .await
                .is_err()
            {
                return None;
            }

            let mut buf = String::new();
            if file.read_to_string(&mut buf).await.is_err() {
                return None;
            }
            if buf.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                return Some((Ok(heartbeat(session_id, task_id)), state));
            }

            state.pos += buf.len() as u64;
            for line in buf.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                let line = line.to_string();
                state.pending.push_back(Event {
                    session_id,
                    task_id,
                    ts: Utc::now(),
                    kind: classify(&line),
                    payload: serde_json::from_str(&line).unwrap_or(serde_json::Value::String(line)),
                });
            }

            let event = state
                .pending
                .pop_front()
                .unwrap_or_else(|| heartbeat(session_id, task_id));
            Some((Ok(event), state))
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn classify_for_test(line: &str) -> String {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "plain".to_string())
    }

    #[tokio::test]
    async fn cli_session_stream_preserves_every_line_from_one_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.jsonl");
        std::fs::write(
            &path,
            "{\"kind\":\"answer\",\"text\":\"one\"}\n{\"kind\":\"tool_call\",\"text\":\"two\"}\nplain output\n",
        )
        .unwrap();

        let session_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut stream = stream_cli_session_events(path, session_id, task_id, classify_for_test);

        let first = stream.next().await.unwrap().unwrap();
        let second = stream.next().await.unwrap().unwrap();
        let third = stream.next().await.unwrap().unwrap();

        assert_eq!(first.kind, "answer");
        assert_eq!(second.kind, "tool_call");
        assert_eq!(third.kind, "plain");
        assert_eq!(
            third.payload,
            serde_json::Value::String("plain output".to_string())
        );
    }
}
