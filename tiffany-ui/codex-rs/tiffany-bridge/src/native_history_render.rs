use crate::native_chat_store::{NativeChatConversation, NativeChatEvent, NativeChatTurn};

pub fn render_native_history_text_graph(
    conversation: &NativeChatConversation,
    limit: usize,
) -> String {
    let mut out = String::from("Conversation flow\n");
    out.push_str(&format!("cwd: {}\n", conversation.cwd));
    out.push_str(&format!("session: {}\n", conversation.id));
    let turns = limited_turns(conversation, limit);
    let first_turn_number = conversation.turns.len().saturating_sub(turns.len()) + 1;
    for (idx, turn) in turns.iter().enumerate() {
        let turn_number = first_turn_number + idx;
        out.push_str(&format!("\n{}. user -> answer\n", turn_number));
        out.push_str(&format!(
            "   user: {}\n",
            truncate_text(&one_line(&turn.user_prompt), 120)
        ));
        out.push_str(&format!(
            "   answer: {}\n",
            truncate_text(&one_line(&turn.result), 140)
        ));
        let summary = native_turn_event_summary(turn);
        if !summary.is_empty() {
            out.push_str(&format!("   events: {}\n", summary.join(" -> ")));
        }
    }
    out
}

pub fn render_native_history_mermaid(
    conversation: &NativeChatConversation,
    limit: usize,
    raw: bool,
) -> String {
    let turns = limited_turns(conversation, limit);
    let first_turn_number = conversation.turns.len().saturating_sub(turns.len()) + 1;
    let mut out = String::new();
    if !raw {
        out.push_str("```mermaid\n");
    }
    out.push_str("flowchart TD\n");
    out.push_str("  start([Tiffany conversation])\n");
    let mut previous = "start".to_string();
    for (idx, turn) in turns.iter().enumerate() {
        let turn_number = first_turn_number + idx;
        let user_id = format!("u{turn_number}");
        let answer_id = format!("a{turn_number}");
        out.push_str(&format!(
            "  {user_id}[\"{}\"]\n",
            escape_mermaid_label(&format!(
                "user: {}",
                truncate_text(&one_line(&turn.user_prompt), 72)
            ))
        ));
        out.push_str(&format!(
            "  {answer_id}[\"{}\"]\n",
            escape_mermaid_label(&format!(
                "answer: {}",
                truncate_text(&one_line(&turn.result), 72)
            ))
        ));
        out.push_str(&format!("  {previous} --> {user_id}\n"));
        out.push_str(&format!("  {user_id} --> {answer_id}\n"));
        let mut event_previous = answer_id.clone();
        for (event_idx, label) in native_turn_event_summary(turn)
            .into_iter()
            .take(6)
            .enumerate()
        {
            let event_id = format!("e{turn_number}_{event_idx}");
            out.push_str(&format!(
                "  {event_id}[\"{}\"]\n",
                escape_mermaid_label(&label)
            ));
            out.push_str(&format!("  {event_previous} --> {event_id}\n"));
            event_previous = event_id;
        }
        previous = event_previous;
    }
    if !raw {
        out.push_str("```\n");
    }
    out
}

pub fn native_turn_event_summary(turn: &NativeChatTurn) -> Vec<String> {
    let mut labels = Vec::new();
    for event in &turn.events {
        let kind = event.kind.as_deref().unwrap_or(event.status.as_str());
        let label = match kind {
            "answer" | "final" | "question" => kind.to_string(),
            "tool_call" => native_history_tool_event_label("tool", event),
            "tool_result" => "tool result".to_string(),
            "diff" => "diff".to_string(),
            "patch" => "patch".to_string(),
            "file_update" => "file update".to_string(),
            "approval" => "approval".to_string(),
            "stderr" => "stderr".to_string(),
            other => other.replace('_', " "),
        };
        if labels.last().map(String::as_str) != Some(label.as_str()) {
            labels.push(label);
        }
    }
    labels
}

pub fn native_history_tool_event_label(prefix: &str, event: &NativeChatEvent) -> String {
    event
        .content
        .as_deref()
        .and_then(nonempty_trimmed)
        .map(|content| {
            compact_tool_event_text(content)
                .unwrap_or_else(|| tiffany_event_format::humanize_jsonish(content, 120))
        })
        .map(|content| one_line(&content))
        .map(|content| truncate_text(&content, 60))
        .map(|content| format!("{prefix}: {content}"))
        .unwrap_or_else(|| prefix.to_string())
}

fn limited_turns(conversation: &NativeChatConversation, limit: usize) -> Vec<&NativeChatTurn> {
    conversation
        .turns
        .iter()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn compact_tool_event_text(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let json_start = trimmed.find('{')?;
    let prefix = trimmed[..json_start].trim();
    let value = serde_json::from_str::<serde_json::Value>(&trimmed[json_start..]).ok()?;
    let detail = compact_tool_json_detail(&value)?;
    Some(if prefix.is_empty() {
        detail
    } else {
        format!("{prefix} {detail}")
    })
}

fn compact_tool_json_detail(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    for key in [
        "command",
        "cmd",
        "file_path",
        "path",
        "url",
        "query",
        "pattern",
    ] {
        if let Some(text) = object.get(key).and_then(compact_json_text_value) {
            return Some(text);
        }
    }
    for key in ["input", "parameters", "args", "arguments"] {
        if let Some(text) = object.get(key).and_then(compact_json_text_value) {
            return Some(text);
        }
        if let Some(detail) = object.get(key).and_then(compact_tool_json_detail) {
            return Some(detail);
        }
    }
    None
}

fn compact_json_text_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => trimmed_string(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        serde_json::Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(compact_json_text_value)
                .collect::<Vec<_>>()
                .join(" ");
            trimmed_string(text)
        }
        _ => None,
    }
}

fn escape_mermaid_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('[', "(")
        .replace(']', ")")
        .replace('{', "(")
        .replace('}', ")")
        .replace('\n', " ")
}

fn nonempty_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn trimmed_string(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn truncate_text(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation_with_events(events: Vec<NativeChatEvent>) -> NativeChatConversation {
        NativeChatConversation {
            id: "tiffany-native-test".into(),
            cwd: "/tmp/repo".into(),
            created_at_unix: 1,
            updated_at_unix: 2,
            turns: vec![NativeChatTurn {
                user_prompt: "实现登录".into(),
                result: "登录完成".into(),
                captured_at_unix: 2,
                events,
            }],
        }
    }

    fn event(kind: &str, content: &str) -> NativeChatEvent {
        NativeChatEvent {
            role: "worker".into(),
            status: "output".into(),
            title: format!("native {kind}"),
            kind: Some(kind.into()),
            content: Some(content.into()),
            agent: Some("claude-code".into()),
            worker_role: Some("worker-cc".into()),
            model: Some("claude-sonnet-4-6".into()),
            provider: Some("anthropic".into()),
            task_id: Some("task-1".into()),
            worker_thread_id: Some("thread-1".into()),
            native_session_id: Some("native-1".into()),
        }
    }

    #[test]
    fn native_history_text_graph_summarizes_flow_without_raw_json() {
        let conversation = conversation_with_events(vec![
            event("tool_call", r#"Bash: {"command":"cargo test -q"}"#),
            event("tool_result", "ok"),
            event("diff", "diff --git a/src/login.rs b/src/login.rs"),
        ]);

        let graph = render_native_history_text_graph(&conversation, 16);

        assert!(graph.contains("Conversation flow"));
        assert!(graph.contains("user: 实现登录"));
        assert!(graph.contains("answer: 登录完成"));
        assert!(graph.contains("events: tool: Bash: cargo test -q -> tool result -> diff"));
        assert!(!graph.contains("\"command\""));
    }

    #[test]
    fn native_history_mermaid_escapes_labels_and_can_emit_raw() {
        let mut conversation = conversation_with_events(vec![event(
            "tool_call",
            r#"Read: {"file_path":"src/[login].rs"}"#,
        )]);
        conversation.turns[0].user_prompt = "看 [a] {b}".into();
        conversation.turns[0].result = "完成 \"ok\"".into();

        let fenced = render_native_history_mermaid(&conversation, 16, false);
        let raw = render_native_history_mermaid(&conversation, 16, true);

        assert!(fenced.starts_with("```mermaid\n"));
        assert!(fenced.ends_with("```\n"));
        assert!(fenced.contains("user: 看 (a) (b)"));
        assert!(fenced.contains("answer: 完成 \\\"ok\\\""));
        assert!(fenced.contains("tool: Read: src/(login).rs"));
        assert!(raw.starts_with("flowchart TD\n"));
        assert!(!raw.contains("```"));
    }
}
