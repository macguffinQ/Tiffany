//! Shared event parsing for Claude Code, Codex CLI, ACP, and TUI surfaces.

use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEventSummary {
    pub kind: String,
    pub text: String,
}

pub fn classify_json_line(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| event_kind_from_value(&value).map(str::to_string))
        .unwrap_or_else(|| "raw".to_string())
}

pub fn summarize_cli_stream_line(line: &str, max: usize) -> Option<AgentEventSummary> {
    let value = match serde_json::from_str::<Value>(line) {
        Ok(value) => value,
        Err(_) => {
            let text = sanitize_text(line, max);
            return (!text.trim().is_empty()).then(|| AgentEventSummary {
                kind: "raw".into(),
                text,
            });
        }
    };

    summarize_event_value(&value, max)
}

pub fn summarize_event_value(value: &Value, max: usize) -> Option<AgentEventSummary> {
    let kind = event_kind_from_value(value).unwrap_or("event").to_string();
    let text = extract_text_from_value(value)
        .or_else(|| {
            value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("line")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| summarize_json_value(value));
    let text = sanitize_text(&text, max);
    (!text.trim().is_empty()).then(|| AgentEventSummary { kind, text })
}

pub fn format_runtime_output(agent: &str, event_kind: &str, payload: &Value, max: usize) -> String {
    let summary = summarize_event_payload(payload, max);
    if summary.trim().is_empty() {
        format!("{agent} {event_kind}")
    } else {
        format!("{agent} {event_kind}: {summary}")
    }
}

pub fn summarize_event_payload(payload: &Value, max: usize) -> String {
    if payload.is_null() {
        return String::new();
    }
    if let Some(text) = payload.as_str() {
        return sanitize_text(text, max);
    }
    if let Some(text) = extract_text_from_value(payload) {
        return sanitize_text(&text, max);
    }
    sanitize_text(&summarize_json_value(payload), max)
}

pub fn extract_text_from_value(value: &Value) -> Option<String> {
    let parts = collect_text_parts(value);
    if !parts.is_empty() {
        return Some(parts.join("\n"));
    }

    let object = value.as_object()?;
    if let Some(name) = object
        .get("name")
        .or_else(|| object.get("tool_name"))
        .and_then(Value::as_str)
        .and_then(non_empty)
    {
        return Some(format!("tool {name}"));
    }
    object
        .get("type")
        .and_then(Value::as_str)
        .and_then(non_empty)
}

fn collect_text_parts(value: &Value) -> Vec<String> {
    if let Some(text) = value.as_str().and_then(non_empty) {
        return vec![text];
    }
    if let Some(array) = value.as_array() {
        return dedupe_text_parts(array.iter().flat_map(collect_text_parts).collect());
    }
    let Some(object) = value.as_object() else {
        return Vec::new();
    };

    let mut parts = Vec::new();
    for key in [
        "text", "summary", "delta", "line", "result", "message", "content", "error",
    ] {
        if let Some(value) = object.get(key) {
            parts.extend(collect_text_parts(value));
        }
    }
    dedupe_text_parts(parts)
}

fn dedupe_text_parts(parts: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for part in parts {
        if out.last().map(String::as_str) != Some(part.as_str()) {
            out.push(part);
        }
    }
    out
}

pub fn final_output_candidate(content: &str, max: usize) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    for marker in [
        " result: ",
        " final: ",
        " final_answer: ",
        " task_complete: ",
        " turn_complete: ",
    ] {
        if let Some(idx) = lower.find(marker) {
            let raw = &content[idx + marker.len()..];
            let result = humanize_jsonish(raw, max);
            if !result.trim().is_empty() {
                return Some(result);
            }
        }
    }

    let normalized = normalize_output_summary(&humanize_jsonish(content, max));
    if let Some(raw) = strip_final_heading(&normalized) {
        let result = humanize_jsonish(raw, max);
        if !result.trim().is_empty() {
            return Some(result);
        }
    }
    None
}

fn strip_final_heading(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    for heading in [
        "final result",
        "final answer",
        "final_result",
        "final_answer",
    ] {
        if trimmed.eq_ignore_ascii_case(heading) {
            return Some("");
        }
        if trimmed
            .get(..heading.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(heading))
        {
            let rest = &trimmed[heading.len()..];
            if let Some(rest) = rest.strip_prefix(':') {
                return Some(rest.trim_start());
            }
            if rest.starts_with('\n') || rest.starts_with("\r\n") {
                return Some(rest.trim_start());
            }
        }
    }
    None
}

pub fn normalized_output_key(content: &str, max: usize) -> Option<String> {
    let display = humanize_jsonish(content, max);
    if is_low_value_output(&display) {
        return None;
    }
    Some(sanitize_text(&normalize_output_summary(&display), max))
}

pub fn normalize_output_summary(display: &str) -> String {
    let trimmed = display.trim();
    let Some((prefix, body)) = trimmed.split_once(": ") else {
        return trimmed.to_string();
    };

    let prefix = prefix.to_ascii_lowercase();
    let mut parts = prefix.split_whitespace();
    let _agent = parts.next();
    match parts.next() {
        Some(
            "assistant" | "event" | "result" | "final" | "final_answer" | "task_complete"
            | "turn_complete",
        ) => body.trim().to_string(),
        _ => trimmed.to_string(),
    }
}

pub fn humanize_jsonish(content: &str, max: usize) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(summary) = summarize_angle_role_block(trimmed, max) {
        return sanitize_text(&summary, max);
    }
    if let Some(summary) = summarize_json_code_fence(trimmed) {
        return sanitize_text(&summary, max);
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return sanitize_text(&summarize_json_value(&value), max);
    }
    if let Some((prefix, raw)) = trimmed.split_once(": ") {
        let raw = raw.trim();
        if looks_like_json(raw) {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                let summary = summarize_json_value(&value);
                return sanitize_text(&format!("{}: {}", prefix.trim(), summary), max);
            }
        }
    }
    if let Some(summary) = summarize_jsonish_lines(trimmed) {
        return sanitize_text(&summary, max);
    }
    if let Some(summary) = summarize_embedded_json_with_context(trimmed) {
        return sanitize_text(&summary, max);
    }
    if let Some(summary) = summarize_loose_structured_json(trimmed) {
        return sanitize_text(&summary, max);
    }
    sanitize_text(trimmed, max)
}

pub fn is_low_value_output(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    let stderr_like = lower.contains(" stderr:") || lower.ends_with(" stderr");
    if stderr_like && !looks_like_actionable_stderr(&lower) {
        return true;
    }

    matches!(
        lower.as_str(),
        "system"
            | "thinking"
            | "claude system"
            | "claude-code system"
            | "claude assistant: thinking"
            | "claude-code assistant: thinking"
            | "claude finished"
            | "claude-code finished"
            | "codex system"
            | "codex assistant: thinking"
            | "codex finished"
    ) || lower.contains(" heartbeat")
        || lower.ends_with(" system: system")
        || lower.ends_with(" assistant: assistant")
        || lower.ends_with(" assistant: thinking")
        || lower.contains("starting model=")
}

fn looks_like_actionable_stderr(lower: &str) -> bool {
    [
        "error",
        "failed",
        "failure",
        "panic",
        "unauthorized",
        "authentication",
        "not authenticated",
        "forbidden",
        "permission",
        "denied",
        "rate limit",
        "api key",
        "login",
        "model not found",
        "unknown model",
        "invalid model",
        "not found",
        "模型不存在",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn is_redundant_role_output(role: &str, content: &str, max: usize) -> bool {
    if role == "worker" {
        return false;
    }
    let display = humanize_jsonish(content, max);
    let normalized = normalize_output_summary(&display);
    let lower = normalized.trim().to_ascii_lowercase();
    match role {
        "planner" => lower.starts_with("plan ready"),
        "critic" | "reviewer" => lower.starts_with("approved"),
        _ => false,
    }
}

pub fn sanitize_text(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max {
            out.push('…');
            return out;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn event_kind_from_value(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

fn summarize_json_value(value: &Value) -> String {
    if let Some(object) = value.as_object() {
        if let Some(summary) = summarize_error_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_plan_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_review_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_tool_object(object) {
            return summary;
        }
    }
    if let Some(text) = extract_text_from_value(value) {
        return text;
    }
    if let Some(array) = value.as_array() {
        return summarize_json_array(array);
    }
    if let Some(object) = value.as_object() {
        let keys = object
            .keys()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if !keys.is_empty() {
            return format!("structured output: {keys}");
        }
        return "structured output".into();
    }
    value.to_string()
}

fn summarize_error_object(object: &Map<String, Value>) -> Option<String> {
    let error = object.get("error")?;
    let message = if let Some(text) = error.as_str().and_then(non_empty) {
        text
    } else if let Some(error_object) = error.as_object() {
        error_object
            .get("message")
            .or_else(|| error_object.get("description"))
            .or_else(|| error_object.get("detail"))
            .and_then(value_to_text)
            .or_else(|| extract_text_from_value(error))
            .unwrap_or_else(|| "request failed".into())
    } else {
        value_to_text(error).unwrap_or_else(|| "request failed".into())
    };

    let code = error
        .as_object()
        .and_then(|error_object| {
            error_object
                .get("code")
                .or_else(|| error_object.get("status"))
                .or_else(|| error_object.get("type"))
        })
        .and_then(value_to_text);

    Some(match code {
        Some(code) if !message.contains(&code) => format!("error: {message} ({code})"),
        _ => format!("error: {message}"),
    })
}

fn summarize_plan_object(object: &Map<String, Value>) -> Option<String> {
    let sub_tasks = object.get("sub_tasks").and_then(Value::as_array)?;
    let mut out = format!("plan ready: {} sub-task(s)", sub_tasks.len());

    for (idx, task) in sub_tasks.iter().take(6).enumerate() {
        let prompt = task
            .get("prompt")
            .or_else(|| task.get("title"))
            .or_else(|| task.get("name"))
            .and_then(value_to_text)
            .unwrap_or_else(|| "task".into());
        out.push_str(&format!(
            "\n  {}. {}",
            idx + 1,
            sanitize_text(prompt.trim(), 140)
        ));

        if let Some(agent) = task
            .get("agent_hint")
            .or_else(|| task.get("agent"))
            .or_else(|| task.get("role"))
            .and_then(value_to_text)
        {
            out.push_str(&format!("\n     agent: {}", sanitize_text(&agent, 80)));
        }
    }

    if sub_tasks.len() > 6 {
        out.push_str(&format!("\n  … {} more", sub_tasks.len() - 6));
    }

    Some(out)
}

fn summarize_review_object(object: &Map<String, Value>) -> Option<String> {
    let approved = object.get("approved").and_then(Value::as_bool)?;
    let issues = value_array_texts(object.get("issues"));
    let suggestions = value_array_texts(object.get("suggestions"));
    let mut out = if approved {
        format!("approved: {} issue(s)", issues.len())
    } else {
        format!("needs changes: {} issue(s)", issues.len())
    };

    for issue in issues.iter().take(6) {
        out.push_str("\n  - ");
        out.push_str(&sanitize_text(issue.trim(), 180));
    }
    if issues.len() > 6 {
        out.push_str(&format!("\n  … {} more issue(s)", issues.len() - 6));
    }

    if !suggestions.is_empty() {
        out.push_str("\n  suggestions:");
        for suggestion in suggestions.iter().take(3) {
            out.push_str("\n  - ");
            out.push_str(&sanitize_text(suggestion.trim(), 180));
        }
        if suggestions.len() > 3 {
            out.push_str(&format!(
                "\n  … {} more suggestion(s)",
                suggestions.len() - 3
            ));
        }
    }

    Some(out)
}

fn summarize_tool_object(object: &Map<String, Value>) -> Option<String> {
    let name = object
        .get("tool_name")
        .or_else(|| object.get("name"))
        .or_else(|| object.get("tool"))
        .and_then(Value::as_str)
        .and_then(|name| non_empty(name).map(|text| sanitize_text(&text, 80)))?;
    let command = object
        .get("input")
        .and_then(|input| input.get("command").or_else(|| input.get("cmd")))
        .or_else(|| object.get("command"))
        .or_else(|| object.get("cmd"))
        .and_then(value_to_text);

    Some(match command {
        Some(command) => format!("tool {name}: {}", sanitize_text(&command, 160)),
        None => format!("tool {name}"),
    })
}

fn value_array_texts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|item| value_to_text(item).or_else(|| extract_text_from_value(item)))
                .map(|text| sanitize_text(text.trim(), 220))
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty(text),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn summarize_json_array(array: &[Value]) -> String {
    let parts = array
        .iter()
        .filter_map(extract_text_from_value)
        .take(6)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        format!("{} item(s)", array.len())
    } else {
        parts.join(" | ")
    }
}

fn summarize_json_code_fence(s: &str) -> Option<String> {
    let fence_start = s.find("```")?;
    let after_open = &s[fence_start + 3..];
    let content_start = after_open.find('\n').map(|idx| idx + 1).unwrap_or(0);
    let after_content_start = &after_open[content_start..];
    let fence_end = after_content_start.find("```")?;
    let raw = after_content_start[..fence_end].trim();
    if !looks_like_json(raw) {
        return None;
    }
    serde_json::from_str::<Value>(raw)
        .ok()
        .map(|value| summarize_json_value(&value))
}

fn summarize_angle_role_block(s: &str, max: usize) -> Option<String> {
    let (head, body) = s.split_once('\n')?;
    let role = head.trim().strip_suffix('>')?.trim();
    if !matches!(role, "planner" | "critic" | "reviewer" | "worker" | "tool") {
        return None;
    }
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let summary = humanize_jsonish(body, max);
    Some(format!("{role}: {summary}"))
}

fn summarize_jsonish_lines(s: &str) -> Option<String> {
    let lines = s
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() < 2 {
        return None;
    }

    let mut parsed = 0usize;
    let mut out = Vec::new();
    for line in lines.iter().take(24) {
        if let Some(summary) = summarize_jsonish_line(line) {
            parsed += 1;
            push_unique_summary_line(&mut out, summary);
        } else {
            push_unique_summary_line(&mut out, sanitize_text(line, 240));
        }
    }
    if lines.len() > 24 {
        out.push(format!("… {} more line(s)", lines.len() - 24));
    }

    (parsed > 0).then(|| out.join("\n"))
}

fn summarize_jsonish_line(line: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
        return Some(summarize_json_value(&value));
    }
    if let Some((prefix, raw)) = line.split_once(": ") {
        let raw = raw.trim();
        if looks_like_json(raw) {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                return Some(format!(
                    "{}: {}",
                    prefix.trim(),
                    summarize_json_value(&value)
                ));
            }
        }
    }
    summarize_embedded_json_with_context(line)
}

fn push_unique_summary_line(out: &mut Vec<String>, line: String) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if out.last().map(String::as_str) != Some(line) {
        out.push(line.to_string());
    }
}

fn summarize_embedded_json_with_context(s: &str) -> Option<String> {
    let start = s
        .char_indices()
        .find_map(|(idx, ch)| (ch == '{').then_some(idx))?;
    let mut iter = serde_json::Deserializer::from_str(&s[start..]).into_iter::<Value>();
    let value = iter.next()?.ok()?;
    let end = start + iter.byte_offset();
    let summary = summarize_json_value(&value);
    let prefix = s[..start].trim();
    let suffix = s[end..].trim();
    match (prefix.is_empty(), suffix.is_empty()) {
        (true, true) => Some(summary),
        (false, true) => Some(format!("{prefix} {summary}")),
        (true, false) => Some(format!("{summary} {suffix}")),
        (false, false) => Some(format!("{prefix} {summary} {suffix}")),
    }
}

fn summarize_loose_structured_json(s: &str) -> Option<String> {
    summarize_loose_review_json(s).or_else(|| summarize_loose_plan_json(s))
}

fn summarize_loose_review_json(s: &str) -> Option<String> {
    if !s.contains("approved") || !(s.contains("issues") || s.contains("suggestions")) {
        return None;
    }
    let approved = find_loose_bool_field(s, "approved")?;
    let issues = extract_loose_string_array(s, "issues");
    let suggestions = extract_loose_string_array(s, "suggestions");
    let mut out = if approved {
        format!("approved: {} issue(s)", issues.len())
    } else {
        format!("needs changes: {} issue(s)", issues.len())
    };

    for issue in issues.iter().take(6) {
        out.push_str("\n  - ");
        out.push_str(&sanitize_text(issue, 180));
    }
    if issues.len() > 6 {
        out.push_str(&format!("\n  … {} more issue(s)", issues.len() - 6));
    }
    if !suggestions.is_empty() {
        out.push_str("\n  suggestions:");
        for suggestion in suggestions.iter().take(3) {
            out.push_str("\n  - ");
            out.push_str(&sanitize_text(suggestion, 180));
        }
    }
    Some(out)
}

fn summarize_loose_plan_json(s: &str) -> Option<String> {
    if !s.contains("sub_tasks") {
        return None;
    }
    let prompts = extract_loose_repeated_string_field(s, "prompt");
    if prompts.is_empty() {
        return Some("plan ready: sub-task(s)".into());
    }
    let mut out = format!("plan ready: {} sub-task(s)", prompts.len());
    for (idx, prompt) in prompts.iter().take(6).enumerate() {
        out.push_str(&format!("\n  {}. {}", idx + 1, sanitize_text(prompt, 140)));
    }
    if prompts.len() > 6 {
        out.push_str(&format!("\n  … {} more", prompts.len() - 6));
    }
    Some(out)
}

fn find_loose_bool_field(s: &str, key: &str) -> Option<bool> {
    let idx = find_loose_key(s, key)?;
    let after_key = &s[idx + key.len() + 2..];
    let after_colon = after_key.split_once(':')?.1.trim_start();
    if after_colon.starts_with("true") {
        Some(true)
    } else if after_colon.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_loose_repeated_string_field(s: &str, key: &str) -> Vec<String> {
    let mut rest = s;
    let mut values = Vec::new();
    while let Some(idx) = find_loose_key(rest, key) {
        let after_key = &rest[idx + key.len() + 2..];
        let Some((_, after_colon)) = after_key.split_once(':') else {
            break;
        };
        let trimmed = after_colon.trim_start();
        if let Some((value, consumed)) = extract_loose_string(trimmed) {
            values.push(value);
            rest = &trimmed[consumed..];
        } else {
            rest = trimmed;
        }
    }
    values
}

fn extract_loose_string_array(s: &str, key: &str) -> Vec<String> {
    let Some(idx) = find_loose_key(s, key) else {
        return Vec::new();
    };
    let after_key = &s[idx + key.len() + 2..];
    let Some((_, after_colon)) = after_key.split_once(':') else {
        return Vec::new();
    };
    let Some(array_start) = after_colon.find('[') else {
        return Vec::new();
    };
    let mut rest = &after_colon[array_start + 1..];
    let mut values = Vec::new();
    loop {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(']') {
            break;
        }
        if let Some((value, consumed)) = extract_loose_string(trimmed) {
            values.push(value);
            rest = &trimmed[consumed..];
            continue;
        }
        if let Some(next_quote) = trimmed.find('"') {
            rest = &trimmed[next_quote..];
        } else {
            break;
        }
    }
    values
}

fn extract_loose_string(s: &str) -> Option<(String, usize)> {
    let start = s.find('"')?;
    let mut out = String::new();
    let mut escape = false;
    for (offset, ch) in s[start + 1..].char_indices() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' => escape = true,
            '"' => {
                let consumed = start + 1 + offset + ch.len_utf8();
                return Some((normalize_loose_string(&out), consumed));
            }
            _ => out.push(ch),
        }
    }
    (!out.trim().is_empty()).then(|| (normalize_loose_string(&out), s.len()))
}

fn normalize_loose_string(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_loose_key(s: &str, key: &str) -> Option<usize> {
    s.find(&format!("\"{key}\""))
}

fn looks_like_json(s: &str) -> bool {
    let s = s.trim_start();
    s.starts_with('{') || s.starts_with('[')
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_claude_stream_text_blocks() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "planning step one" }
                ]
            }
        })
        .to_string();

        let summary = summarize_cli_stream_line(&line, 200).unwrap();

        assert_eq!(summary.kind, "assistant");
        assert_eq!(summary.text, "planning step one");
    }

    #[test]
    fn extracts_all_text_blocks_from_nested_content() {
        let value = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "first paragraph" },
                    { "type": "text", "text": "second paragraph" }
                ]
            }
        });

        let text = extract_text_from_value(&value).expect("text");

        assert_eq!(text, "first paragraph\nsecond paragraph");
    }

    #[test]
    fn summarizes_codex_style_result_lines() {
        let line = serde_json::json!({
            "type": "turn_complete",
            "result": "Implemented the patch"
        })
        .to_string();

        let summary = summarize_cli_stream_line(&line, 200).unwrap();

        assert_eq!(summary.kind, "turn_complete");
        assert_eq!(summary.text, "Implemented the patch");
    }

    #[test]
    fn summarizes_structured_role_json_stream_lines() {
        let plan = summarize_cli_stream_line(
            r#"{"sub_tasks":[{"prompt":"fix display","agent_hint":"worker-cc"}]}"#,
            500,
        )
        .unwrap();
        assert_eq!(plan.kind, "event");
        assert!(plan.text.contains("plan ready: 1 sub-task(s)"));
        assert!(plan.text.contains("agent: worker-cc"));

        let review =
            summarize_cli_stream_line(r#"{"approved":false,"issues":["raw JSON leaked"]}"#, 500)
                .unwrap();
        assert_eq!(review.kind, "event");
        assert!(review.text.contains("needs changes: 1 issue(s)"));
        assert!(review.text.contains("raw JSON leaked"));
    }

    #[test]
    fn formats_runtime_payloads_without_raw_json() {
        let payload = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{ "type": "text", "text": "Working on it" }] }
        });

        let text = format_runtime_output("claude-code", "assistant", &payload, 200);

        assert_eq!(text, "claude-code assistant: Working on it");
        assert!(!text.contains('{'));
    }

    #[test]
    fn extracts_final_output_from_agent_prefixed_result() {
        let result = final_output_candidate(
            "codex turn_complete: {\"result\":\"Done and verified.\"}",
            200,
        )
        .unwrap();

        assert_eq!(result, "Done and verified.");
    }

    #[test]
    fn extracts_final_output_from_plain_final_heading() {
        assert_eq!(
            final_output_candidate("Final result\n你好！\n我可以帮你写代码。", 500).as_deref(),
            Some("你好！\n我可以帮你写代码。")
        );
        assert_eq!(
            final_output_candidate("claude-code assistant: Final answer:\n完成并验证。", 500)
                .as_deref(),
            Some("完成并验证。")
        );
    }

    #[test]
    fn humanizes_json_code_fence_without_raw_json() {
        let display = humanize_jsonish(
            "```json\n{\"approved\":false,\"issues\":[\"missing test\"]}\n```",
            200,
        );

        assert_eq!(display, "needs changes: 1 issue(s)\n  - missing test");
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_plan_json_as_waterfall_summary() {
        let display = humanize_jsonish(
            r#"{"sub_tasks":[{"prompt":"read the code and identify the broken TUI path","agent_hint":"worker-claude"},{"prompt":"patch JSON parsing"}]}"#,
            500,
        );

        assert!(display.contains("plan ready: 2 sub-task(s)"));
        assert!(display.contains("1. read the code"));
        assert!(display.contains("agent: worker-claude"));
        assert!(display.contains("2. patch JSON parsing"));
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_review_json_with_issues_and_suggestions() {
        let display = humanize_jsonish(
            r#"{"approved":false,"issues":["missing worker output","raw JSON leaked"],"suggestions":["show parsed summaries"]}"#,
            500,
        );

        assert!(display.contains("needs changes: 2 issue(s)"));
        assert!(display.contains("- missing worker output"));
        assert!(display.contains("- raw JSON leaked"));
        assert!(display.contains("suggestions:"));
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_error_json_with_message_and_code() {
        let display = humanize_jsonish(
            r#"{"error":{"message":"模型不存在，请检查模型代码。","code":1211}}"#,
            500,
        );

        assert_eq!(display, "error: 模型不存在，请检查模型代码。 (1211)");
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_jsonl_stream_as_lines() {
        let display = humanize_jsonish(
            "worker: {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"step one\"}]}}\n{\"approved\":true,\"issues\":[]}",
            500,
        );

        assert!(display.contains("worker: step one"));
        assert!(display.contains("approved: 0 issue(s)"));
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_angle_role_block_with_json_body() {
        let display = humanize_jsonish(
            "critic>\n  {\"approved\": false, \"issues\": [\"plan delegates despite direct-answer instruction\"]}",
            500,
        );

        assert_eq!(
            display,
            "critic: needs changes: 1 issue(s)\n  - plan delegates despite direct-answer instruction"
        );
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_loose_truncated_review_json() {
        let display = humanize_jsonish(
            "critic>\n  {\"approved\": false, \"issues\": [\"Self-contradictory: the prompt says 不要再拆解或委派 sub_agent,\n  直接在主对话中用中文回答\", \"Unnecessary delegation",
            800,
        );

        assert!(display.contains("critic: needs changes: 2 issue(s)"));
        assert!(display.contains("Self-contradictory"));
        assert!(display.contains("直接在主对话中用中文回答"));
        assert!(display.contains("Unnecessary delegation"));
        assert!(!display.contains('{'));
        assert!(!display.contains("\"issues\""));
    }

    #[test]
    fn normalizes_duplicate_assistant_and_result_outputs() {
        let assistant = normalized_output_key("claude assistant: useful summary", 200).unwrap();
        let result = normalized_output_key("claude result: useful summary", 200).unwrap();

        assert_eq!(assistant, result);
        assert_eq!(assistant, "useful summary");
    }

    #[test]
    fn filters_cli_noise_and_redundant_role_summaries() {
        assert!(is_low_value_output("claude-code system: system"));
        assert!(is_low_value_output("claude assistant: assistant"));
        assert!(is_low_value_output("claude-code assistant: thinking"));
        assert!(is_redundant_role_output(
            "planner",
            r#"{"sub_tasks":[{"prompt":"say hello"}]}"#,
            200
        ));
        assert!(!is_redundant_role_output(
            "reviewer",
            r#"{"approved":false,"issues":["missing output"]}"#,
            200
        ));
        assert!(is_redundant_role_output(
            "reviewer",
            r#"{"approved":true,"issues":[]}"#,
            200
        ));
        assert!(!is_redundant_role_output(
            "worker",
            "plan ready: not a planner status",
            200
        ));
    }

    #[test]
    fn keeps_actionable_stderr_visible() {
        assert!(!is_low_value_output(
            "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]"
        ));
        assert!(humanize_jsonish(
            "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]",
            200
        )
        .contains("模型不存在"));
        assert!(!is_low_value_output(
            "claude-code stderr: authentication failed: invalid api key"
        ));
        assert!(is_low_value_output(
            "codex stderr: debug transport message without actionable details"
        ));
    }

    #[test]
    fn preserves_plain_text_around_embedded_json() {
        let display = humanize_jsonish(
            "stderr: API Error: {\"error\":{\"message\":\"模型不存在\"}} retry stopped",
            200,
        );

        assert!(display.contains("stderr: API Error:"));
        assert!(display.contains("模型不存在"));
        assert!(display.contains("retry stopped"));
        assert!(!display.contains("{\"error\""));
    }
}
