use tiffany_event_format as event_format;

#[derive(Clone, Copy, Debug)]
pub struct VisibleOutputEvent<'a> {
    pub role: &'a str,
    pub status: &'a str,
    pub agent: Option<&'a str>,
    pub worker_role: Option<&'a str>,
    pub task_id: Option<&'a str>,
    pub event_kind: Option<&'a str>,
    pub recovery: Option<&'a str>,
    pub content: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
pub struct PendingVisibleOutput<'a> {
    pub role: &'a str,
    pub agent: Option<&'a str>,
    pub worker_role: Option<&'a str>,
    pub task_id: Option<&'a str>,
    pub content: &'a str,
}

pub fn visible_output_content(
    event: &VisibleOutputEvent<'_>,
    worker_max: usize,
    control_max: usize,
) -> Option<String> {
    let content = event.content?.trim();
    if content.is_empty() {
        return None;
    }

    let display = if event.role == "worker" {
        event_format::visible_agent_output(
            content,
            visible_content_max(event, worker_max, control_max),
        )
        .map(|view| view.display)?
    } else {
        event_format::clean_visible_agent_output(
            content,
            visible_content_max(event, worker_max, control_max),
        )?
    };
    let display = strip_redundant_role_prefix(event.role, &display);
    let display = format_tiffany_summary_style(event.role, &display);
    if event_format::is_low_value_output(&display) {
        return None;
    }
    (!display.trim().is_empty()).then_some(display)
}

pub fn visible_output_suffix(event: &VisibleOutputEvent<'_>, control_max: usize) -> &'static str {
    if event.recovery.and_then(nonempty_trimmed).is_some() {
        return "session recovery";
    }
    let Some(raw) = event.content else {
        return "output";
    };
    if looks_like_native_session_recovery(raw) {
        return "session recovery";
    }
    match visible_output_kind_for_event(event, raw, control_max) {
        Some(event_format::VisibleAgentOutputKind::Final) => "final",
        Some(event_format::VisibleAgentOutputKind::Answer) => "answer",
        Some(event_format::VisibleAgentOutputKind::Question) => "question",
        Some(event_format::VisibleAgentOutputKind::Approval) => "approval",
        Some(event_format::VisibleAgentOutputKind::ToolCall) => "tool call",
        Some(event_format::VisibleAgentOutputKind::ToolResult) => "tool result",
        Some(event_format::VisibleAgentOutputKind::Diff) => "diff",
        Some(event_format::VisibleAgentOutputKind::Patch) => "patch",
        Some(event_format::VisibleAgentOutputKind::FileUpdate) => "file update",
        Some(event_format::VisibleAgentOutputKind::Stderr) => "stderr",
        Some(event_format::VisibleAgentOutputKind::Actionable) => "alert",
        Some(event_format::VisibleAgentOutputKind::Normal) | None => "output",
    }
}

pub fn visible_output_kind_for_event(
    event: &VisibleOutputEvent<'_>,
    raw: &str,
    control_max: usize,
) -> Option<event_format::VisibleAgentOutputKind> {
    let raw_kind = visible_output_kind_from_raw(raw, control_max);
    if matches!(
        raw_kind,
        Some(event_format::VisibleAgentOutputKind::Question)
            | Some(event_format::VisibleAgentOutputKind::Approval)
    ) {
        return raw_kind;
    }
    if raw_kind.is_some_and(is_artifact_output_kind) {
        return raw_kind;
    }
    event
        .event_kind
        .and_then(event_format::visible_agent_output_kind_for_event_kind)
        .or(raw_kind)
}

pub fn looks_like_native_session_recovery(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    (lower.contains("native session busy") || lower.contains("native session missing"))
        && (lower.contains("retrying once") || lower.contains("retry "))
        && (lower.contains("same native session") || lower.contains("cleared saved session"))
}

pub fn normalized_visible_output_key(content: &str, max: usize) -> String {
    event_format::normalized_output_key(content, max)
        .unwrap_or_else(|| content.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn visible_output_seen_key(
    event: &VisibleOutputEvent<'_>,
    content: &str,
    pending_tool_call: Option<&PendingVisibleOutput<'_>>,
    worker_max: usize,
    control_max: usize,
) -> String {
    let scope = visible_output_scope(event, control_max);
    let mut key = normalized_visible_output_key(content, worker_max);
    if is_tool_result_output(event, control_max)
        && let Some(tool_event) = pending_tool_call
        && same_worker_tool_sequence(tool_event, event)
    {
        key = format!(
            "after:{}=>{key}",
            normalized_visible_output_key(tool_event.content, worker_max)
        );
    }
    format!("{scope}:{key}")
}

pub fn visible_output_scope(event: &VisibleOutputEvent<'_>, control_max: usize) -> String {
    let mut scope = event.role.to_string();
    if let Some(agent) = event.agent.filter(|agent| !agent.is_empty()) {
        scope.push(':');
        scope.push_str(agent);
    }
    if let Some(task_id) = short_task_id(event.task_id) {
        scope.push(':');
        scope.push_str(task_id);
    }
    if let Some(kind) = output_dedupe_event_kind(event, control_max) {
        scope.push(':');
        scope.push_str(&kind);
    }
    if let Some(recovery) = event.recovery.and_then(nonempty_trimmed) {
        scope.push(':');
        scope.push_str(recovery);
    }
    scope
}

pub fn visible_output_kind_scope_label(
    kind: event_format::VisibleAgentOutputKind,
) -> Option<&'static str> {
    match kind {
        event_format::VisibleAgentOutputKind::Answer
        | event_format::VisibleAgentOutputKind::Normal
        | event_format::VisibleAgentOutputKind::Final => None,
        event_format::VisibleAgentOutputKind::Question => Some("question"),
        event_format::VisibleAgentOutputKind::Approval => Some("approval"),
        event_format::VisibleAgentOutputKind::ToolCall => Some("tool"),
        event_format::VisibleAgentOutputKind::ToolResult => Some("tool_result"),
        event_format::VisibleAgentOutputKind::Diff => Some("diff"),
        event_format::VisibleAgentOutputKind::Patch => Some("patch"),
        event_format::VisibleAgentOutputKind::FileUpdate => Some("file"),
        event_format::VisibleAgentOutputKind::Stderr => Some("stderr"),
        event_format::VisibleAgentOutputKind::Actionable => Some("alert"),
    }
}

pub fn format_tiffany_summary_style(role: &str, content: &str) -> String {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let first = restyle_summary_first_line(first);
    let rest = lines.collect::<Vec<_>>();
    if role == "planner" {
        return compact_plan_summary(&first, &rest);
    }
    if rest.is_empty() {
        first
    } else {
        format!("{}\n{}", first, rest.join("\n"))
    }
}

fn visible_content_max(
    event: &VisibleOutputEvent<'_>,
    worker_max: usize,
    control_max: usize,
) -> usize {
    if event.role == "worker" {
        worker_max
    } else {
        control_max
    }
}

fn visible_output_kind_from_raw(
    raw: &str,
    control_max: usize,
) -> Option<event_format::VisibleAgentOutputKind> {
    event_format::visible_agent_output(raw, control_max).map(|view| view.kind)
}

fn is_artifact_output_kind(kind: event_format::VisibleAgentOutputKind) -> bool {
    matches!(
        kind,
        event_format::VisibleAgentOutputKind::Diff
            | event_format::VisibleAgentOutputKind::Patch
            | event_format::VisibleAgentOutputKind::FileUpdate
    )
}

fn is_tool_result_output(event: &VisibleOutputEvent<'_>, control_max: usize) -> bool {
    event.role == "worker"
        && event.status == "output"
        && event
            .content
            .and_then(|raw| visible_output_kind_for_event(event, raw, control_max))
            == Some(event_format::VisibleAgentOutputKind::ToolResult)
}

fn same_worker_tool_sequence(
    left: &PendingVisibleOutput<'_>,
    right: &VisibleOutputEvent<'_>,
) -> bool {
    left.role == right.role
        && left.agent == right.agent
        && left.worker_role == right.worker_role
        && left.task_id == right.task_id
}

fn output_dedupe_event_kind(event: &VisibleOutputEvent<'_>, control_max: usize) -> Option<String> {
    if event.role == "worker" && event.recovery.and_then(nonempty_trimmed).is_some() {
        return Some("session_recovery".to_string());
    }
    let kind = event
        .event_kind
        .map(str::trim)
        .filter(|kind| !kind.is_empty())?;
    if event.role == "worker" {
        let output_kind = event
            .content
            .and_then(|raw| visible_output_kind_for_event(event, raw, control_max));
        if matches!(
            output_kind,
            Some(event_format::VisibleAgentOutputKind::Question)
                | Some(event_format::VisibleAgentOutputKind::Approval)
                | Some(event_format::VisibleAgentOutputKind::ToolCall)
                | Some(event_format::VisibleAgentOutputKind::ToolResult)
                | Some(event_format::VisibleAgentOutputKind::FileUpdate)
        ) || is_answer_like_event_kind(kind)
        {
            return output_kind
                .and_then(visible_output_kind_scope_label)
                .map(str::to_string);
        }
    }
    Some(kind.to_string())
}

fn is_answer_like_event_kind(kind: &str) -> bool {
    matches!(
        kind,
        "assistant" | "result" | "final" | "final_answer" | "turn_complete" | "message"
    )
}

fn strip_redundant_role_prefix(role: &str, content: &str) -> String {
    if !matches!(role, "planner" | "critic" | "reviewer") {
        return content.trim().to_string();
    }
    let trimmed = content.trim();
    let lower = trimmed.to_ascii_lowercase();
    let role = role.to_ascii_lowercase();
    for separator in [": ", " - ", " — "] {
        let prefix = format!("{role}{separator}");
        if lower.starts_with(&prefix) {
            return trimmed[prefix.len()..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

#[derive(Debug)]
struct CompactPlanTask {
    number: String,
    prompt: String,
    agent: Option<String>,
}

fn compact_plan_summary(first: &str, rest: &[&str]) -> String {
    if !first
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("plan ready")
    {
        return if rest.is_empty() {
            first.to_string()
        } else {
            format!("{}\n{}", first, rest.join("\n"))
        };
    }

    let mut tasks: Vec<CompactPlanTask> = Vec::new();
    let mut more_line: Option<String> = None;
    let mut passthrough: Vec<String> = Vec::new();
    for line in rest {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((number, prompt)) = parse_plan_task_line(trimmed) {
            tasks.push(CompactPlanTask {
                number,
                prompt,
                agent: None,
            });
            continue;
        }
        if let Some(agent) = trimmed.strip_prefix("agent: ") {
            if let Some(task) = tasks.last_mut() {
                task.agent = Some(agent.trim().to_string());
            } else {
                passthrough.push(trimmed.to_string());
            }
            continue;
        }
        if trimmed.starts_with("...") && trimmed.contains("more") {
            more_line = Some(trimmed.to_string());
            continue;
        }
        passthrough.push(trimmed.to_string());
    }

    if tasks.is_empty() {
        return if rest.is_empty() {
            first.to_string()
        } else {
            format!("{}\n{}", first, rest.join("\n"))
        };
    }

    let mut out = vec![first.to_string()];
    let visible_count = tasks.len().min(3);
    for task in tasks.iter().take(visible_count) {
        let mut line = format!("  {}. {}", task.number, truncate_text(&task.prompt, 96));
        if let Some(agent) = task.agent.as_deref().filter(|agent| !agent.is_empty()) {
            line.push_str(" · ");
            line.push_str(&truncate_text(agent, 48));
        }
        out.push(line);
    }

    let hidden_count = tasks.len().saturating_sub(visible_count);
    if hidden_count > 0 {
        out.push(format!("  ... {hidden_count} more"));
    } else if let Some(more_line) = more_line {
        out.push(format!("  {more_line}"));
    }
    out.extend(
        passthrough
            .into_iter()
            .take(2)
            .map(|line| format!("  {line}")),
    );
    out.join("\n")
}

fn parse_plan_task_line(line: &str) -> Option<(String, String)> {
    let (number, prompt) = line.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some((number.to_string(), prompt.trim().to_string()))
}

fn restyle_summary_first_line(line: &str) -> String {
    for prefix in ["plan ready", "approved", "needs changes"] {
        let needle = format!("{prefix}: ");
        if let Some(rest) = line.strip_prefix(&needle) {
            return format!("{prefix} - {rest}");
        }
    }
    line.to_string()
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

fn short_task_id(task_id: Option<&str>) -> Option<&str> {
    task_id.map(|id| id.get(..8).unwrap_or(id))
}

fn nonempty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_format::VisibleAgentOutputKind;

    const WORKER_MAX: usize = 240_000;
    const CONTROL_MAX: usize = 16_000;

    fn event<'a>(
        role: &'a str,
        status: &'a str,
        event_kind: Option<&'a str>,
        content: Option<&'a str>,
    ) -> VisibleOutputEvent<'a> {
        VisibleOutputEvent {
            role,
            status,
            agent: Some("claude-code"),
            worker_role: Some("worker-cc"),
            task_id: Some("12345678-90ab"),
            event_kind,
            recovery: None,
            content,
        }
    }

    #[test]
    fn humanizes_planner_json_and_strips_role_prefix() {
        let raw = r#"planner: {"sub_tasks":[{"prompt":"Write tests","agent":"worker-cc"}]}"#;
        let event = event("planner", "output", Some("plan"), Some(raw));

        let display = visible_output_content(&event, WORKER_MAX, CONTROL_MAX).expect("visible");

        assert!(!display.contains("\"sub_tasks\""));
        assert!(!display.starts_with("planner:"));
        assert!(display.contains("plan ready"));
    }

    #[test]
    fn scopes_assistant_and_result_together_but_tool_separately() {
        let assistant = event("worker", "output", Some("assistant"), Some("tests passed"));
        let result = event("worker", "output", Some("result"), Some("tests passed"));
        let tool = event(
            "worker",
            "output",
            Some("tool_use"),
            Some("tool Bash: cargo test"),
        );

        assert_eq!(
            visible_output_scope(&assistant, CONTROL_MAX),
            visible_output_scope(&result, CONTROL_MAX)
        );
        assert_ne!(
            visible_output_scope(&assistant, CONTROL_MAX),
            visible_output_scope(&tool, CONTROL_MAX)
        );
        assert_eq!(
            normalized_visible_output_key("tests passed", WORKER_MAX),
            "tests passed"
        );
    }

    #[test]
    fn tool_result_seen_key_depends_on_preceding_tool_call() {
        let call = PendingVisibleOutput {
            role: "worker",
            agent: Some("claude-code"),
            worker_role: Some("worker-cc"),
            task_id: Some("12345678-90ab"),
            content: "tool Bash: cargo test",
        };
        let result = event(
            "worker",
            "output",
            Some("tool_result"),
            Some("claude-code user: tool result: ok"),
        );

        let key = visible_output_seen_key(&result, "ok", Some(&call), WORKER_MAX, CONTROL_MAX);

        assert!(key.contains("worker:claude-code:12345678:tool_result:after:"));
        assert!(key.ends_with("=>ok"));
    }

    #[test]
    fn event_kind_overrides_normal_tool_result_wrapper() {
        let output = event(
            "worker",
            "output",
            Some("tool_result"),
            Some("claude-code user: tool result: ok"),
        );

        assert_eq!(
            visible_output_kind_for_event(&output, output.content.unwrap(), CONTROL_MAX),
            Some(VisibleAgentOutputKind::ToolResult)
        );
        assert_eq!(visible_output_suffix(&output, CONTROL_MAX), "tool result");
    }
}
