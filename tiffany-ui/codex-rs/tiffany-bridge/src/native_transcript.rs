#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTranscriptEvent {
    pub kind: String,
    pub title: String,
    pub content: String,
}

pub fn parse_claude_native_transcript_events(body: &str) -> Vec<NativeTranscriptEvent> {
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .flat_map(|value| native_transcript_events_from_claude_value(&value))
        .collect()
}

pub fn parse_codex_native_transcript_events(body: &str) -> Vec<NativeTranscriptEvent> {
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .flat_map(|value| native_transcript_events_from_codex_value(&value))
        .collect()
}

pub fn parse_gemini_native_transcript_events(
    body: &str,
    skip_messages: usize,
) -> Vec<NativeTranscriptEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    value
        .get("messages")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .skip(skip_messages)
        .flat_map(native_transcript_events_from_gemini_message)
        .collect()
}

fn native_transcript_events_from_claude_value(
    value: &serde_json::Value,
) -> Vec<NativeTranscriptEvent> {
    let mut events = Vec::new();
    if value.get("type").and_then(|value| value.as_str()) == Some("result")
        && let Some(result) = native_result_content(value)
    {
        events.push(native_transcript_event("final", "native final", result));
    }

    let Some(message) = value.get("message") else {
        return events;
    };
    let role = message
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    if role == "assistant"
        && let Some(content) = native_message_text(message)
    {
        events.push(native_transcript_event("answer", "native answer", content));
    }
    if role == "user"
        && let Some(content) = native_message_text(message)
        && !content.contains("tool_result")
    {
        events.push(native_transcript_event("question", "native user", content));
    }
    if let Some(items) = message.get("content").and_then(|value| value.as_array()) {
        for item in items {
            match item.get("type").and_then(|value| value.as_str()) {
                Some("tool_use") => {
                    if let Some(content) = native_tool_call_text(item) {
                        let kind = native_tool_call_kind(item);
                        events.push(native_transcript_event(
                            kind,
                            native_tool_call_title(kind),
                            content,
                        ));
                    }
                }
                Some("tool_result") => {
                    if let Some(content) = native_tool_result_text(item) {
                        let kind = if item
                            .get("is_error")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false)
                        {
                            native_error_kind(&content)
                        } else {
                            "tool_result"
                        };
                        events.push(native_transcript_event(
                            kind,
                            native_result_title(kind, "native tool result"),
                            content,
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    events
}

fn native_transcript_events_from_gemini_message(
    message: &serde_json::Value,
) -> Vec<NativeTranscriptEvent> {
    let mut events = Vec::new();
    let message_type = message
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    match message_type {
        "user" => {
            if let Some(content) = message.get("content").and_then(json_string_value) {
                events.push(native_transcript_event("question", "native user", content));
            }
        }
        "gemini" | "assistant" | "model" => {
            if let Some(content) = message.get("content").and_then(json_string_value) {
                events.push(native_transcript_event("answer", "native answer", content));
            }
            if let Some(thoughts) = gemini_thoughts_text(message) {
                events.push(native_transcript_event(
                    "output",
                    "native reasoning",
                    thoughts,
                ));
            }
            for tool_call in message
                .get("toolCalls")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(content) = gemini_tool_call_text(tool_call) {
                    let kind = gemini_tool_call_kind(tool_call);
                    events.push(native_transcript_event(
                        kind,
                        native_tool_call_title(kind),
                        content,
                    ));
                }
                if let Some(content) = gemini_tool_result_text(tool_call) {
                    let kind = if gemini_tool_call_failed(tool_call) {
                        native_error_kind(&content)
                    } else {
                        "tool_result"
                    };
                    events.push(native_transcript_event(
                        kind,
                        native_result_title(kind, "native tool result"),
                        content,
                    ));
                }
            }
            for tool_result in message
                .get("toolResults")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(content) = gemini_tool_result_response_text(tool_result) {
                    let kind = if gemini_tool_result_has_error(tool_result) {
                        native_error_kind(&content)
                    } else {
                        "tool_result"
                    };
                    events.push(native_transcript_event(
                        kind,
                        native_result_title(kind, "native tool result"),
                        content,
                    ));
                }
            }
        }
        "error" => {
            if let Some(content) = message.get("content").and_then(json_string_value) {
                events.push(native_transcript_event("stderr", "native error", content));
            }
        }
        _ => {}
    }
    events
}

fn native_transcript_events_from_codex_value(
    value: &serde_json::Value,
) -> Vec<NativeTranscriptEvent> {
    let mut events = Vec::new();
    let item = value.get("item").unwrap_or(value);
    let item = item.get("event_msg").unwrap_or(item);
    if let Some(message) = item.get("agent_message").and_then(json_string_value) {
        events.push(native_transcript_event("answer", "native answer", message));
    }
    if let Some(message) = item.get("user_message").and_then(json_string_value) {
        events.push(native_transcript_event("question", "native user", message));
    }
    if let Some(message) = item.get("agent_reasoning").and_then(json_string_value) {
        events.push(native_transcript_event(
            "output",
            "native reasoning",
            message,
        ));
    }
    if let Some(command_line) = item
        .get("exec_command_begin")
        .and_then(codex_exec_command_text)
    {
        events.push(native_transcript_event(
            "tool_call",
            "native tool call",
            command_line,
        ));
    }
    if let Some(result) = item.get("exec_command_end").and_then(codex_exec_end_text) {
        let kind = codex_exec_end_kind(item.get("exec_command_end"), &result);
        events.push(native_transcript_event(
            kind,
            native_result_title(kind, "native tool result"),
            result,
        ));
    }
    if let Some(patch) = item.get("patch_apply_begin").and_then(codex_patch_text) {
        events.push(native_transcript_event("patch", "native patch", patch));
    }
    if let Some(patch) = item.get("patch_apply_end").and_then(codex_patch_text) {
        events.push(native_transcript_event("patch", "native patch", patch));
    }
    if let Some(diff) = item.get("turn_diff").and_then(codex_turn_diff_text) {
        events.push(native_transcript_event("diff", "native diff", diff));
    }
    events
}

fn native_transcript_event(
    kind: impl Into<String>,
    title: impl Into<String>,
    content: String,
) -> NativeTranscriptEvent {
    NativeTranscriptEvent {
        kind: kind.into(),
        title: title.into(),
        content,
    }
}

fn native_result_title(kind: &str, default_title: &'static str) -> &'static str {
    match kind {
        "approval" => "native approval",
        "stderr" => "native tool error",
        _ => default_title,
    }
}

fn native_tool_call_title(kind: &str) -> &'static str {
    match kind {
        "file_update" => "native file update",
        "patch" => "native patch",
        _ => "native tool call",
    }
}

fn native_tool_call_kind(item: &serde_json::Value) -> &'static str {
    let name = item
        .get("name")
        .and_then(json_string_value)
        .unwrap_or_default();
    let input = item.get("input");
    native_named_tool_call_kind(&name, input)
}

fn gemini_tool_call_kind(tool_call: &serde_json::Value) -> &'static str {
    let name = tool_call
        .get("displayName")
        .and_then(json_string_value)
        .or_else(|| tool_call.get("name").and_then(json_string_value))
        .unwrap_or_default();
    let input = tool_call.get("args").or_else(|| tool_call.get("input"));
    native_named_tool_call_kind(&name, input)
}

fn native_named_tool_call_kind(name: &str, input: Option<&serde_json::Value>) -> &'static str {
    let normalized = normalize_native_tool_name(name);
    if matches!(
        normalized.as_str(),
        "applypatch" | "apply_patch" | "patch" | "patchapply"
    ) || input.is_some_and(native_tool_input_has_patch)
    {
        return "patch";
    }
    if matches!(
        normalized.as_str(),
        "write"
            | "edit"
            | "multiedit"
            | "multi_edit"
            | "replace"
            | "replacefile"
            | "writefile"
            | "createfile"
            | "deletefile"
            | "movefile"
            | "renamefile"
            | "updatefile"
            | "savefile"
    ) {
        return "file_update";
    }
    "tool_call"
}

fn normalize_native_tool_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .flat_map(char::to_lowercase)
        .collect()
}

fn native_tool_input_has_patch(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.get("patch").is_some()
                || map.get("diff").is_some()
                || map.get("edits").is_some_and(native_tool_input_has_patch)
        }
        serde_json::Value::Array(items) => items.iter().any(native_tool_input_has_patch),
        serde_json::Value::String(value) => {
            let lower = value.trim_start().to_ascii_lowercase();
            lower.starts_with("*** begin patch") || lower.starts_with("diff --git")
        }
        _ => false,
    }
}

fn native_error_kind(content: &str) -> &'static str {
    if native_content_needs_approval(content) {
        "approval"
    } else {
        "stderr"
    }
}

fn codex_exec_end_kind(value: Option<&serde_json::Value>, result: &str) -> &'static str {
    let failed = value
        .and_then(|value| {
            value
                .get("exit_code")
                .or_else(|| value.get("exitCode"))
                .and_then(|value| value.as_i64())
        })
        .is_some_and(|exit_code| exit_code != 0);
    if native_content_needs_approval(result) {
        "approval"
    } else if failed {
        "stderr"
    } else {
        "tool_result"
    }
}

fn native_content_needs_approval(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    if lower.trim().is_empty() || native_content_is_service_error(&lower) {
        return false;
    }
    let explicit_request = [
        "request_permissions",
        "request permissions",
        "permission request",
        "permissions request",
        "request approval",
        "approval request",
        "requires approval",
        "approval required",
        "needs approval",
        "waiting for approval",
        "approve this command",
        "allow this command",
        "grant permission",
        "grant permissions",
        "requires additional permissions",
        "additional permissions",
        "command requires approval",
        "patch requires approval",
    ];
    if explicit_request.iter().any(|needle| lower.contains(needle)) {
        return true;
    }
    let denied_by_user = [
        "user did not allow",
        "did not allow tool call",
        "tool call was denied",
        "tool call denied",
        "command denied",
        "command rejected",
        "operation cancelled",
        "operation canceled",
        "operation was cancelled",
        "operation was canceled",
        "not allowed by user",
        "denied by user",
        "rejected by user",
        "cancelled by user",
        "canceled by user",
    ];
    if denied_by_user.iter().any(|needle| lower.contains(needle)) {
        return true;
    }
    lower.contains("permission denied")
        && (lower.contains("tool")
            || lower.contains("command")
            || lower.contains("write")
            || lower.contains("edit")
            || lower.contains("file")
            || lower.contains("shell"))
}

fn native_content_is_service_error(lower: &str) -> bool {
    [
        "api error",
        "api key",
        "authentication",
        "unauthorized",
        "401",
        "403",
        "404",
        "429",
        "model not found",
        "模型不存在",
        "rate limit",
        "quota",
        "network error",
        "connection",
        "premature close",
        "timed out",
        "timeout",
        "dns",
        "tls",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn native_result_content(value: &serde_json::Value) -> Option<String> {
    ["result", "content", "summary", "message"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn native_message_text(message: &serde_json::Value) -> Option<String> {
    match message.get("content")? {
        serde_json::Value::String(value) => trimmed_string(value.clone()),
        serde_json::Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|value| value.as_str()) == Some("text") {
                        item.get("text")
                            .and_then(|value| value.as_str())
                            .map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            trimmed_string(text)
        }
        _ => None,
    }
}

fn native_tool_call_text(item: &serde_json::Value) -> Option<String> {
    let name = item
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("tool");
    let input = item
        .get("input")
        .map(native_tool_input_summary)
        .unwrap_or_default();
    let input = truncate_text(&one_line(&input), 2_000);
    if input.is_empty() {
        Some(format!("tool {name}"))
    } else {
        Some(format!("tool {name}: {input}"))
    }
}

fn native_tool_input_summary(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let fields = [
                "file_path",
                "path",
                "command",
                "cmd",
                "pattern",
                "query",
                "url",
                "instruction",
            ];
            let known = fields
                .into_iter()
                .filter_map(|key| {
                    map.get(key)
                        .and_then(json_string_value)
                        .map(|value| format!("{key}={}", one_line(&value)))
                })
                .take(4)
                .collect::<Vec<_>>();
            if !known.is_empty() {
                return known.join(", ");
            }
            map.iter()
                .filter_map(|(key, value)| {
                    json_string_value(value).map(|value| format!("{key}={}", one_line(&value)))
                })
                .take(4)
                .collect::<Vec<_>>()
                .join(", ")
        }
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(json_string_value)
            .take(4)
            .collect::<Vec<_>>()
            .join(", "),
        _ => json_string_value(value).unwrap_or_default(),
    }
}

fn native_tool_result_text(item: &serde_json::Value) -> Option<String> {
    let content = match item.get("content")? {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(|value| value.as_str()) == Some("text") {
                    item.get("text")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                } else {
                    item.as_str().map(str::to_string)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        value => value.to_string(),
    };
    trimmed_string(content)
}

fn json_string_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => trimmed_string(value.clone()),
        serde_json::Value::Object(_) => [
            "message", "text", "content", "output", "summary", "result", "diff", "patch",
        ]
        .into_iter()
        .find_map(|key| value.get(key).and_then(json_string_value)),
        serde_json::Value::Array(items) => {
            let content = items
                .iter()
                .filter_map(json_string_value)
                .collect::<Vec<_>>()
                .join("\n");
            trimmed_string(content)
        }
        _ => None,
    }
}

fn codex_exec_command_text(value: &serde_json::Value) -> Option<String> {
    let command = value
        .get("command")
        .and_then(json_string_value)
        .or_else(|| value.get("cmd").and_then(json_string_value))
        .or_else(|| value.get("argv").and_then(json_string_value))
        .or_else(|| value.get("args").and_then(json_string_value))?;
    Some(format!("shell: {}", one_line(&command)))
}

fn codex_exec_end_text(value: &serde_json::Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(exit_code) = value
        .get("exit_code")
        .or_else(|| value.get("exitCode"))
        .and_then(|value| value.as_i64())
    {
        parts.push(format!("exit {exit_code}"));
    }
    if let Some(stdout) = value.get("stdout").and_then(json_string_value) {
        parts.push(stdout);
    }
    if let Some(stderr) = value.get("stderr").and_then(json_string_value) {
        parts.push(stderr);
    }
    if parts.is_empty() {
        json_string_value(value)
    } else {
        trimmed_string(parts.join("\n"))
    }
}

fn codex_patch_text(value: &serde_json::Value) -> Option<String> {
    value
        .get("patch")
        .and_then(json_string_value)
        .or_else(|| value.get("changes").and_then(json_string_value))
        .or_else(|| value.get("summary").and_then(json_string_value))
        .or_else(|| json_string_value(value))
}

fn codex_turn_diff_text(value: &serde_json::Value) -> Option<String> {
    value
        .get("diff")
        .and_then(json_string_value)
        .or_else(|| value.get("unified_diff").and_then(json_string_value))
        .or_else(|| value.get("changes").and_then(json_string_value))
        .or_else(|| json_string_value(value))
}

fn gemini_thoughts_text(message: &serde_json::Value) -> Option<String> {
    let thoughts = message.get("thoughts")?.as_array()?;
    let content = thoughts
        .iter()
        .filter_map(|thought| {
            let subject = thought.get("subject").and_then(json_string_value);
            let description = thought.get("description").and_then(json_string_value);
            match (subject, description) {
                (Some(subject), Some(description)) => Some(format!(
                    "{}: {}",
                    one_line(&subject),
                    one_line(&description)
                )),
                (Some(subject), None) => Some(one_line(&subject)),
                (None, Some(description)) => Some(one_line(&description)),
                (None, None) => None,
            }
        })
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");
    trimmed_string(content)
}

fn gemini_tool_call_text(tool_call: &serde_json::Value) -> Option<String> {
    let name = tool_call
        .get("displayName")
        .and_then(json_string_value)
        .or_else(|| tool_call.get("name").and_then(json_string_value))
        .unwrap_or_else(|| "tool".to_string());
    let description = tool_call
        .get("description")
        .and_then(json_string_value)
        .or_else(|| {
            tool_call
                .get("args")
                .map(gemini_tool_args_summary)
                .and_then(trimmed_string)
        })
        .unwrap_or_default();
    let description = truncate_text(&one_line(&description), 2_000);
    if description.is_empty() {
        Some(format!("tool {}", one_line(&name)))
    } else {
        Some(format!("tool {}: {}", one_line(&name), description))
    }
}

fn gemini_tool_result_text(tool_call: &serde_json::Value) -> Option<String> {
    if let Some(status) = tool_call.get("status").and_then(json_string_value) {
        let status = status.to_ascii_lowercase();
        if matches!(
            status.as_str(),
            "cancelled" | "canceled" | "error" | "failed"
        ) {
            let response = tool_call
                .get("result")
                .and_then(gemini_tool_result_response_text)
                .unwrap_or_else(|| status.clone());
            return Some(response);
        }
    }
    tool_call
        .get("result")
        .and_then(gemini_tool_result_response_text)
}

fn gemini_tool_result_response_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Array(items) => {
            let content = items
                .iter()
                .filter_map(gemini_tool_result_response_text)
                .collect::<Vec<_>>()
                .join("\n");
            trimmed_string(content)
        }
        serde_json::Value::Object(_) => value
            .get("functionResponse")
            .and_then(gemini_tool_result_response_text)
            .or_else(|| {
                value
                    .get("response")
                    .and_then(gemini_tool_result_response_text)
            })
            .or_else(|| value.get("error").and_then(json_string_value))
            .or_else(|| value.get("output").and_then(json_string_value))
            .or_else(|| value.get("result").and_then(json_string_value)),
        _ => json_string_value(value),
    }
}

fn gemini_tool_call_failed(tool_call: &serde_json::Value) -> bool {
    tool_call
        .get("status")
        .and_then(json_string_value)
        .map(|status| {
            matches!(
                status.to_ascii_lowercase().as_str(),
                "cancelled" | "canceled" | "error" | "failed"
            )
        })
        .unwrap_or(false)
        || tool_call
            .get("result")
            .is_some_and(gemini_tool_result_has_error)
}

fn gemini_tool_result_has_error(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.iter().any(gemini_tool_result_has_error),
        serde_json::Value::Object(_) => {
            value.get("error").is_some()
                || value
                    .get("functionResponse")
                    .is_some_and(gemini_tool_result_has_error)
                || value
                    .get("response")
                    .is_some_and(gemini_tool_result_has_error)
        }
        _ => false,
    }
}

fn gemini_tool_args_summary(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let fields = [
                "file_path",
                "path",
                "command",
                "cmd",
                "old_string",
                "new_string",
                "query",
                "pattern",
                "instruction",
            ];
            fields
                .into_iter()
                .filter_map(|key| {
                    map.get(key)
                        .and_then(json_string_value)
                        .map(|value| format!("{key}={}", one_line(&value)))
                })
                .take(4)
                .collect::<Vec<_>>()
                .join(", ")
        }
        _ => json_string_value(value).unwrap_or_default(),
    }
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

    #[test]
    fn parses_claude_jsonl_without_raw_json() {
        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"我会先检查文件。"},{"type":"tool_use","name":"Read","input":{"file_path":"README.md"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"README body"}]}}
{"type":"result","result":"完成。"}
"#;

        let events = parse_claude_native_transcript_events(jsonl);

        assert_eq!(events.len(), 4);
        assert_eq!(events[0].kind, "answer");
        assert_eq!(events[0].content, "我会先检查文件。");
        assert_eq!(events[1].kind, "tool_call");
        assert!(events[1].content.contains("tool Read"));
        assert!(events[1].content.contains("file_path=README.md"));
        assert!(!events[1].content.contains("\"type\""));
        assert_eq!(events[2].kind, "tool_result");
        assert_eq!(events[3].kind, "final");
        assert_eq!(events[3].content, "完成。");
    }

    #[test]
    fn parses_codex_jsonl_tools_and_diff() {
        let jsonl = r#"{"item":{"event_msg":{"user_message":"继续"}}}
{"item":{"event_msg":{"agent_message":"我会修改。"}}}
{"item":{"event_msg":{"exec_command_begin":{"command":"cargo test"}}}}
{"item":{"event_msg":{"exec_command_end":{"exit_code":1,"stdout":"running","stderr":"permission denied while writing file"}}}}
{"item":{"event_msg":{"patch_apply_begin":{"patch":"*** Begin Patch\n*** End Patch"}}}}
{"item":{"event_msg":{"turn_diff":{"diff":"diff --git a/a b/a"}}}}
"#;

        let events = parse_codex_native_transcript_events(jsonl);

        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            vec![
                "question",
                "answer",
                "tool_call",
                "approval",
                "patch",
                "diff"
            ]
        );
        assert!(events[2].content.contains("shell: cargo test"));
        assert!(events[3].content.contains("permission denied"));
    }

    #[test]
    fn parses_gemini_json_with_skip_and_tool_results() {
        let json = r#"{
          "messages": [
            {"type":"user","content":"old"},
            {"type":"model","content":"我会处理","thoughts":[{"subject":"plan","description":"inspect files"}],
             "toolCalls":[{"displayName":"Edit","args":{"file_path":"README.md","old_string":"a","new_string":"b"},"result":{"functionResponse":{"response":"ok"}}}]},
            {"type":"error","content":"network error"}
          ]
        }"#;

        let events = parse_gemini_native_transcript_events(json, 1);

        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["answer", "output", "file_update", "tool_result", "stderr"]
        );
        assert_eq!(events[0].content, "我会处理");
        assert!(events[2].content.contains("file_path=README.md"));
        assert_eq!(events[3].content, "ok");
    }
}
