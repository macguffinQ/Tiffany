use super::state::{ChatMsg, InputState};
use super::util::{
    copy_to_clipboard, format_duration_ms, humanize_jsonish, is_low_value_execution_output,
    normalize_execution_output_summary, summarize_execution_output, truncate_chars,
};
use crate::agent_events;
use crate::core::session_store::SessionStore;
use crate::pipeline::orchestrator::RunProgress;
use std::io::Write;
use std::path::PathBuf;

const TRACE_COMPACT_LIMIT: usize = 8;
const TRACE_EXPANDED_LIMIT: usize = 24;
const PROCESS_SUMMARY_LIMIT: usize = 12;
const PROCESS_FAILURE_CONTEXT_LIMIT: usize = 8;
const PROCESS_FINAL_OUTPUT_MAX_CHARS: usize = 240_000;

pub(super) fn record_run_event(input: &mut InputState, event: &str) {
    if should_skip_recorded_run_event(input, event) {
        return;
    }

    let line = format!("{}  {}", current_time_hms(), event);
    input.run_events.push(line.clone());
    if let Some(path) = &input.process_capture_path {
        let _ = append_process_capture_line(path, &line);
    }
    if input.run_events.len() > 500 {
        let overflow = input.run_events.len() - 500;
        input.run_events.drain(0..overflow);
    }
}

pub(super) fn record_run_progress(input: &mut InputState, event: &RunProgress) {
    if recorded_output_was_seen(input, event) {
        return;
    }
    let Some(event_text) = format_run_event_for_recording(event) else {
        return;
    };
    remember_recorded_output(input, event);
    record_run_event(input, &event_text);
}

fn recorded_output_was_seen(input: &InputState, event: &RunProgress) -> bool {
    let Some(key) = recorded_output_key_for_event(event) else {
        return false;
    };
    input
        .run_recorded_output_keys
        .iter()
        .any(|seen| seen == &key)
}

fn remember_recorded_output(input: &mut InputState, event: &RunProgress) {
    let Some(key) = recorded_output_key_for_event(event) else {
        return;
    };
    if input
        .run_recorded_output_keys
        .iter()
        .any(|seen| seen == &key)
    {
        return;
    }
    input.run_recorded_output_keys.push(key);
    if input.run_recorded_output_keys.len() > 160 {
        input.run_recorded_output_keys.remove(0);
    }
}

fn recorded_output_key_for_event(event: &RunProgress) -> Option<String> {
    match event {
        RunProgress::WorkerOutput {
            task_id,
            agent,
            role,
            content,
        } => Some(format!(
            "worker:{}:{}:{}:{}",
            &task_id.to_string()[..8],
            role,
            agent,
            normalized_recorded_output(content, 200)?
        )),
        RunProgress::RoleOutput { role, content } => {
            if agent_events::is_redundant_role_output(role, content, 180) {
                return None;
            }
            Some(format!(
                "role:{}:{}",
                role,
                normalized_recorded_output(content, 180)?
            ))
        }
        _ => None,
    }
}

fn normalized_recorded_output(content: &str, max: usize) -> Option<String> {
    if agent_events::final_output_candidate(content, PROCESS_FINAL_OUTPUT_MAX_CHARS).is_some() {
        return None;
    }
    let display = summarize_execution_output(content, max)?;
    Some(truncate_chars(
        &normalize_execution_output_summary(&display),
        220,
    ))
}

fn should_skip_recorded_run_event(input: &InputState, event: &str) -> bool {
    if run_event_is_low_value_output(event) {
        return true;
    }

    input
        .run_events
        .last()
        .and_then(|line| line.split_once("  ").map(|(_, body)| body))
        == Some(event)
}

fn run_event_is_low_value_output(event: &str) -> bool {
    let Some((head, body)) = event.split_once(": ") else {
        return false;
    };

    let is_output = head.ends_with(" output") || head == "Worker output";
    is_output && is_low_value_execution_output(body)
}

pub(super) fn start_live_trace(input: &mut InputState) {
    input.trace_message_index = None;
    if !input.trace_live_enabled {
        return;
    }
    append_live_trace_message(input);
    refresh_live_trace(input, false);
}

pub(super) fn refresh_live_trace(input: &mut InputState, terminal: bool) {
    if !input.trace_live_enabled {
        return;
    }
    if input.run_events.is_empty() && !run_is_active(input) {
        return;
    }

    ensure_live_trace_message(input);
    let content = format_live_trace_content(input, terminal);
    let status = if terminal || !run_is_active(input) {
        "complete"
    } else {
        "thinking"
    };
    if let Some(msg) = live_trace_message_mut(input) {
        msg.content = content;
        msg.status = status.into();
        msg.ts = std::time::SystemTime::now();
    }
}

pub(super) fn complete_live_trace(input: &mut InputState) {
    if let Some(msg) = live_trace_message_mut(input) {
        msg.status = "complete".into();
    }
}

pub(super) fn live_trace_status(input: &InputState) -> String {
    format!(
        "Live trace:\n  mode: {}\n  block: {}\n  captured events: {}\n\nCommands:\n  /trace on\n  /trace off\n  /trace full\n  /trace compact\n\nUse /process [n] for the full captured process.",
        live_trace_mode(input),
        live_trace_block_state(input),
        input.run_events.len()
    )
}

pub(super) fn live_trace_summary(input: &InputState) -> String {
    format!(
        "{} ({}, {} event(s))",
        live_trace_mode(input),
        live_trace_block_state(input),
        input.run_events.len()
    )
}

pub(super) fn create_process_capture_file(
    store: &SessionStore,
    prompt: &str,
) -> std::io::Result<PathBuf> {
    let dir = store.log_dir().join("process");
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("process-{}.log", ts));
    let mut file = std::fs::File::create(&path)?;
    writeln!(file, "# orchestrator process capture")?;
    writeln!(file, "# prompt: {}", prompt.replace('\n', " "))?;
    writeln!(file)?;
    Ok(path)
}

fn append_process_capture_line(path: &PathBuf, line: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

fn current_time_hms() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            let secs_in_day = d.as_secs() % 86400;
            let h = (secs_in_day / 3600) % 24;
            let m = (secs_in_day / 60) % 60;
            let s = secs_in_day % 60;
            format!("{:02}:{:02}:{:02}", h, m, s)
        })
        .unwrap_or_else(|_| "??:??:??".into())
}

fn append_live_trace_message(input: &mut InputState) {
    let idx = input.transcript.len();
    input.transcript.push(ChatMsg {
        role: "trace".into(),
        content: String::new(),
        ts: std::time::SystemTime::now(),
        status: if run_is_active(input) {
            "thinking".into()
        } else {
            "complete".into()
        },
    });
    input.trace_message_index = Some(idx);
}

fn ensure_live_trace_message(input: &mut InputState) {
    if live_trace_message(input).is_some() {
        return;
    }

    let active = run_is_active(input);
    if let Some(idx) = input
        .transcript
        .iter()
        .rposition(|msg| msg.role == "trace" && (!active || msg.status == "thinking"))
    {
        input.trace_message_index = Some(idx);
        return;
    }

    append_live_trace_message(input);
}

fn live_trace_message(input: &InputState) -> Option<&ChatMsg> {
    input
        .trace_message_index
        .and_then(|idx| input.transcript.get(idx))
        .filter(|msg| msg.role == "trace")
}

fn live_trace_message_mut(input: &mut InputState) -> Option<&mut ChatMsg> {
    let idx = input.trace_message_index?;
    input
        .transcript
        .get_mut(idx)
        .filter(|msg| msg.role == "trace")
}

fn format_live_trace_content(input: &InputState, terminal: bool) -> String {
    let limit = if input.trace_expanded {
        TRACE_EXPANDED_LIMIT
    } else {
        TRACE_COMPACT_LIMIT
    };
    let (state_icon, state) = trace_state(input, terminal);

    if input.run_events.is_empty() {
        return format!(
            "status: {} {} | no events yet\n\nfull log: /process 80",
            state_icon, state
        );
    }

    let start = input.run_events.len().saturating_sub(limit);
    let shown = input.run_events.len() - start;
    let mode = if input.trace_expanded {
        "full"
    } else {
        "compact"
    };
    let current = input
        .run_events
        .last()
        .map(|line| humanize_trace_event(line));
    let mut out = format!(
        "status: {} {} | {} | {} event(s)",
        state_icon,
        state,
        mode,
        input.run_events.len()
    );
    if let Some(current) = &current {
        out.push_str(&format!(
            "\nnow: {} {} - {}",
            current.icon, current.kind, current.summary
        ));
    }
    out.push_str(&format!("\n\nrecent activity (last {}):", shown));
    for line in &input.run_events[start..] {
        let event = humanize_trace_event(line);
        out.push('\n');
        out.push_str(&format!(
            "  {} {}  {:<8} {}",
            event.icon, event.time, event.kind, event.summary
        ));
    }
    if !input.trace_expanded && input.run_events.len() > limit {
        out.push_str("\n\nmore: /trace full expands this view | /process 200 shows raw log");
    } else {
        out.push_str("\n\nfull log: /process 200");
    }
    if terminal {
        if let Some(path) = &input.process_capture_path {
            out.push_str(&format!("\nsaved: {}", path.display()));
        }
    }
    out
}

struct TraceEventView {
    icon: &'static str,
    time: String,
    kind: &'static str,
    summary: String,
}

fn humanize_trace_event(line: &str) -> TraceEventView {
    let (time, body) = split_trace_time(line);
    let body = body.trim();
    let (kind, summary) = summarize_trace_body(body);
    TraceEventView {
        icon: trace_kind_icon(kind),
        time: time.to_string(),
        kind,
        summary,
    }
}

fn split_trace_time(line: &str) -> (&str, &str) {
    line.split_once("  ").unwrap_or(("??:??:??", line))
}

fn summarize_trace_body(body: &str) -> (&'static str, String) {
    if let Some(prompt) = body.strip_prefix("Started run: ") {
        return (
            "start",
            format!("task accepted: {}", truncate_chars(prompt, 120)),
        );
    }
    if let Some(agent) = body.strip_prefix("Agent hint: ") {
        return ("route", format!("routing hint: {}", agent));
    }
    if body == "Planning: decomposing task" {
        return ("plan", "planning work".into());
    }
    if let Some(count) = body.strip_prefix("Planned: ") {
        return ("plan", format!("plan ready: {}", count));
    }
    if let Some(round) = body.strip_prefix("Critiquing: ") {
        return ("critic", format!("checking plan: {}", round));
    }
    if body == "Critique: approved" {
        return ("critic", "plan approved".into());
    }
    if let Some(issues) = body.strip_prefix("Critique: rejected with ") {
        return ("critic", format!("plan needs changes: {}", issues));
    }
    if let Some(attempt) = body.strip_prefix("Replanning: ") {
        return ("plan", format!("updating plan: {}", attempt));
    }
    if let Some(count) = body.strip_prefix("Executing: ") {
        return ("run", format!("running {}", count));
    }
    if let Some(worker) = body.strip_prefix("Worker started: ") {
        return ("worker", format!("started {}", worker));
    }
    if let Some(output) = body.strip_prefix("Worker output: ") {
        return ("worker", summarize_worker_output(output));
    }
    if let Some(done) = body.strip_prefix("Worker done: ") {
        return ("worker", format!("completed {}", done));
    }
    if let Some(failed) = body.strip_prefix("Worker failed: ") {
        return ("worker", format!("failed {}", failed));
    }
    if let Some(task) = body.strip_prefix("Reviewing: ") {
        return ("review", format!("reviewing {}", task));
    }
    if let Some(task) = body.strip_prefix("Review approved: ") {
        return ("review", format!("approved {}", task));
    }
    if let Some(task) = body.strip_prefix("Review rejected: ") {
        return ("review", format!("needs fixes: {}", task));
    }
    if let Some(done) = body.strip_prefix("Done: ") {
        return ("done", format!("completed {}", done));
    }
    if let Some(error) = body.strip_prefix("Failed: ") {
        return ("error", truncate_chars(error, 140));
    }
    if body == "Cancelled by user" {
        return ("cancel", "cancelled by user".into());
    }
    if let Some((role, output)) = body.split_once(" output: ") {
        if matches!(role, "planner" | "critic" | "reviewer") {
            return (role_label(role), humanize_jsonish(output, 140));
        }
    }
    ("event", humanize_jsonish(body, 140))
}

fn summarize_worker_output(output: &str) -> String {
    let output = output.trim();
    let Some((id, content)) = output.split_once(' ') else {
        return humanize_jsonish(output, 140);
    };
    if let Some((agent, content)) = content.split_once(": ") {
        return format!(
            "{} {}: {}",
            id,
            compact_worker_label(agent),
            summarize_worker_content(content.trim(), 130)
                .unwrap_or_else(|| humanize_jsonish(content.trim(), 130))
        );
    }
    format!("{}: {}", id, humanize_jsonish(content.trim(), 130))
}

fn compact_worker_label(label: &str) -> String {
    let mut parts = label.split_whitespace().collect::<Vec<_>>();
    if matches!(
        parts.last().copied(),
        Some("assistant" | "result" | "final" | "final_answer" | "task_complete" | "turn_complete")
    ) {
        parts.pop();
    }
    if parts.is_empty() {
        label.trim().to_string()
    } else {
        parts.join(" ")
    }
}

fn summarize_worker_content(content: &str, max: usize) -> Option<String> {
    let display = summarize_execution_output(content, max)?;
    let display = normalize_execution_output_summary(&display);
    (!display.trim().is_empty()).then_some(display)
}

fn role_label(role: &str) -> &'static str {
    match role {
        "planner" => "plan",
        "critic" => "critic",
        "reviewer" => "review",
        _ => "role",
    }
}

fn trace_state(input: &InputState, terminal: bool) -> (&'static str, &'static str) {
    if let Some(last) = input.run_events.last() {
        if last.contains("Failed:") {
            return ("✗", "failed");
        }
        if last.contains("Cancelled by user") {
            return ("○", "cancelled");
        }
    }

    if terminal {
        ("✓", "complete")
    } else if run_is_active(input) {
        ("●", "running")
    } else {
        ("○", "idle")
    }
}

fn trace_kind_icon(kind: &str) -> &'static str {
    match kind {
        "done" => "✓",
        "error" => "✗",
        "cancel" => "○",
        "worker" => "●",
        "run" => "▶",
        "review" => "◆",
        "critic" | "plan" => "◇",
        "route" => "→",
        "start" => "○",
        _ => "·",
    }
}

fn live_trace_mode(input: &InputState) -> &'static str {
    if !input.trace_live_enabled {
        "off"
    } else if input.trace_expanded {
        "on/full"
    } else {
        "on/compact"
    }
}

fn live_trace_block_state(input: &InputState) -> &'static str {
    if live_trace_message(input).is_some() {
        "visible"
    } else {
        "not visible"
    }
}

fn run_is_active(input: &InputState) -> bool {
    input.run_rx.is_some() || input.run_handle.is_some()
}

#[cfg(test)]
pub(super) fn format_run_event(event: &RunProgress) -> String {
    format_run_event_for_recording(event).unwrap_or_else(|| "low-value output suppressed".into())
}

fn format_run_event_for_recording(event: &RunProgress) -> Option<String> {
    match event {
        RunProgress::Planning => Some("Planning: decomposing task".into()),
        RunProgress::Planned { sub_task_count } => {
            Some(format!("Planned: {} sub-task(s)", sub_task_count))
        }
        RunProgress::Critiquing { round } => Some(format!("Critiquing: round {}", round)),
        RunProgress::CritiqueResult { approved, issues } => {
            if *approved {
                Some("Critique: approved".into())
            } else {
                Some(format!("Critique: rejected with {} issue(s)", issues))
            }
        }
        RunProgress::Replanning { attempt } => Some(format!("Replanning: attempt {}", attempt)),
        RunProgress::Executing { sub_task_count } => {
            Some(format!("Executing: {} sub-task(s)", sub_task_count))
        }
        RunProgress::WorkerStarted {
            task_id,
            agent,
            role,
            runtime,
            model,
            provider,
            ..
        } => Some(format!(
            "Worker started: {} via {} ({}, {}, {})",
            role,
            runtime,
            provider_model_label(provider.as_deref(), model),
            agent,
            &task_id.to_string()[..8]
        )),
        RunProgress::WorkerOutput {
            task_id,
            agent,
            role: _,
            content,
        } => {
            if agent_events::final_output_candidate(content, PROCESS_FINAL_OUTPUT_MAX_CHARS)
                .is_some()
            {
                return None;
            }
            let display = summarize_worker_content(content, 160)?;
            Some(format!(
                "Worker output: {} {}: {}",
                &task_id.to_string()[..8],
                agent,
                truncate_chars(&display, 160)
            ))
        }
        RunProgress::RoleOutput { role, content } => {
            if agent_events::is_redundant_role_output(role, content, 180) {
                return None;
            }
            let display = summarize_execution_output(content, 180)?;
            Some(format!("{} output: {}", role, display))
        }
        RunProgress::WorkerDone {
            task_id,
            agent,
            role,
            duration_ms,
            ok,
        } => Some(format!(
            "Worker {}: {} ({}, {}, {})",
            if *ok { "done" } else { "failed" },
            role,
            agent,
            &task_id.to_string()[..8],
            format_duration_ms(*duration_ms)
        )),
        RunProgress::Reviewing { task_id } => {
            Some(format!("Reviewing: {}", &task_id.to_string()[..8]))
        }
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            if *approved {
                Some(format!("Review approved: {}", &task_id.to_string()[..8]))
            } else {
                Some(format!(
                    "Review rejected: {} with {} issue(s)",
                    &task_id.to_string()[..8],
                    issues
                ))
            }
        }
        RunProgress::Done { task_count } => {
            Some(format!("Done: {} sub-task(s) completed", task_count))
        }
        RunProgress::Failed(msg) => Some(format!("Failed: {}", humanize_jsonish(msg, 180))),
    }
}

fn provider_model_label(provider: Option<&str>, model: &str) -> String {
    match provider.map(str::trim).filter(|value| !value.is_empty()) {
        Some(provider) => format!("{provider}/{model}"),
        None => model.to_string(),
    }
}

pub(super) fn format_process_capture(input: &InputState, limit: usize) -> String {
    if input.run_events.is_empty() {
        return "No captured run process yet.".into();
    }

    let events = filtered_process_events(input);
    if events.is_empty() {
        if let Some(filter) = process_filter(input) {
            return format!(
                "No captured run process matching {:?} ({} total event(s)).",
                filter,
                input.run_events.len()
            );
        }
        return "No captured run process yet.".into();
    }

    let start = events.len().saturating_sub(limit);
    let filter_suffix = process_filter(input)
        .map(|filter| format!(" matching {:?}", filter))
        .unwrap_or_default();
    let mut out = format!(
        "Captured run process{} (last {} of {} event(s), total {}):",
        filter_suffix,
        events.len() - start,
        events.len(),
        input.run_events.len()
    );
    for line in &events[start..] {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&humanize_process_event_line(line));
    }
    if let Some(path) = &input.process_capture_path {
        out.push_str(&format!("\n\ncapture file:\n  {}", path.display()));
    }
    out
}

pub(super) fn format_process_summary(input: &InputState) -> String {
    if input.run_events.is_empty() {
        return "No captured run process yet.".into();
    }

    let events = filtered_process_events(input);
    if events.is_empty() {
        if let Some(filter) = process_filter(input) {
            return format!(
                "No captured run process matching {:?} ({} total event(s)).",
                filter,
                input.run_events.len()
            );
        }
        return "No captured run process yet.".into();
    }

    let important = important_process_events(&events);
    let source = if important.is_empty() {
        events
    } else {
        important
    };
    let start = source.len().saturating_sub(PROCESS_SUMMARY_LIMIT);
    let filter_suffix = process_filter(input)
        .map(|filter| format!(" matching {:?}", filter))
        .unwrap_or_default();
    let mut out = format!(
        "Process summary{} ({} event(s), {} captured total):",
        filter_suffix,
        source.len(),
        input.run_events.len()
    );
    for line in &source[start..] {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&humanize_process_event_line(line));
    }
    out.push_str("\n\nMore detail: /process full, /process 200, /trace full");
    if let Some(path) = &input.process_capture_path {
        out.push_str(&format!("\nCapture file: {}", path.display()));
    }
    out
}

pub(super) fn format_failure_context(input: &InputState) -> Option<String> {
    if input.run_events.is_empty() {
        return None;
    }
    let events: Vec<&String> = input.run_events.iter().collect();
    let important = important_process_events(&events);
    let source = if important.is_empty() {
        events
    } else {
        important
    };
    if source.is_empty() {
        return None;
    }

    let start = source.len().saturating_sub(PROCESS_FAILURE_CONTEXT_LIMIT);
    let mut out = String::from("Recent process:");
    for line in &source[start..] {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&humanize_process_event_line(line));
    }
    out.push_str("\n\nDetails: /process full or /trace full");
    if let Some(path) = &input.process_capture_path {
        out.push_str(&format!("\nCapture file: {}", path.display()));
    }
    Some(out)
}

fn important_process_events<'a>(events: &[&'a String]) -> Vec<&'a String> {
    events
        .iter()
        .copied()
        .filter(|line| process_event_is_important(line))
        .collect()
}

fn process_event_is_important(line: &str) -> bool {
    let body = line
        .split_once("  ")
        .map(|(_, body)| body)
        .unwrap_or(line)
        .trim();

    body.starts_with("Started run:")
        || body.starts_with("Agent hint:")
        || body.starts_with("Planning:")
        || body.starts_with("Planned:")
        || body.starts_with("Critique:")
        || body.starts_with("Replanning:")
        || body.starts_with("Executing:")
        || body.starts_with("Worker started:")
        || body.starts_with("Worker failed:")
        || body.starts_with("Worker done:")
        || body.starts_with("Review rejected:")
        || body.starts_with("Review approved:")
        || body.starts_with("Done:")
        || body.starts_with("Failed:")
        || body == "Cancelled by user"
}

fn format_process_for_copy(input: &InputState) -> String {
    let mut out = String::new();
    out.push_str("# orchestrator process capture\n\n");
    if let Some(filter) = process_filter(input) {
        out.push_str(&format!("# filter: {:?}\n\n", filter));
    }
    for line in filtered_process_events(input) {
        out.push_str(&humanize_process_event_line(line));
        out.push('\n');
    }
    out
}

fn humanize_process_event_line(line: &str) -> String {
    let Some((time, body)) = line.split_once("  ") else {
        return humanize_jsonish(line, 260);
    };
    let body = body.trim();
    if let Some(output) = body.strip_prefix("Worker output: ") {
        return format!(
            "{}  Worker output: {}",
            time,
            summarize_worker_output(output)
        );
    }
    if let Some((head, raw)) = body.split_once(": ") {
        return format!("{}  {}: {}", time, head, humanize_jsonish(raw, 220));
    }
    format!("{}  {}", time, humanize_jsonish(body, 260))
}

fn save_process_capture_to_file(input: &InputState) -> std::io::Result<PathBuf> {
    let home = home::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home dir"))?;
    let dir = home.join(".orchestrator").join("process");
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("process-export-{}.log", ts));
    std::fs::write(&path, format_process_for_copy(input))?;
    Ok(path)
}

pub(super) fn export_process_capture(input: &InputState, target: &str) -> String {
    if input.run_events.is_empty() {
        return "No captured run process yet.".into();
    }

    let event_count = filtered_process_events(input).len();
    if event_count == 0 {
        if let Some(filter) = process_filter(input) {
            return format!(
                "No captured run process matching {:?} ({} total event(s)).",
                filter,
                input.run_events.len()
            );
        }
    }

    match target {
        "clipboard" | "cb" => {
            let formatted = format_process_for_copy(input);
            match copy_to_clipboard(&formatted) {
                Ok(()) => format!(
                    "Copied {} process event(s) to clipboard ({} bytes).",
                    event_count,
                    formatted.len()
                ),
                Err(e) => format!("Process clipboard export failed: {}", e),
            }
        }
        "file" | "disk" | "log" => {
            if let (true, Some(path)) = (
                process_filter(input).is_none(),
                input.process_capture_path.as_ref(),
            ) {
                format!(
                    "Process capture is already saved to:\n  {}\n\nUse `/process clipboard` to copy it.",
                    path.display()
                )
            } else {
                match save_process_capture_to_file(input) {
                    Ok(path) => format!(
                        "Saved {} process event(s) to:\n  {}",
                        event_count,
                        path.display()
                    ),
                    Err(e) => format!("Process file export failed: {}", e),
                }
            }
        }
        other => format!(
            "Unknown process export target: {}\nUse `/process save` or `/process clipboard`.",
            other
        ),
    }
}

fn process_filter(input: &InputState) -> Option<&str> {
    input
        .process_filter
        .as_deref()
        .map(str::trim)
        .filter(|filter| !filter.is_empty())
}

fn filtered_process_events(input: &InputState) -> Vec<&String> {
    let Some(filter) = process_filter(input) else {
        return input.run_events.iter().collect();
    };
    let filter = filter.to_ascii_lowercase();
    input
        .run_events
        .iter()
        .filter(|line| line.to_ascii_lowercase().contains(&filter))
        .collect()
}

pub(super) fn process_filter_summary(input: &InputState) -> String {
    match process_filter(input) {
        Some(filter) => {
            let matching = filtered_process_events(input).len();
            format!(
                "{:?} ({} of {} event(s))",
                filter,
                matching,
                input.run_events.len()
            )
        }
        None => "none".into(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    #[test]
    fn process_capture_respects_filter() {
        let mut input = InputState::default();
        input.run_events = vec![
            "10:00:00  Planning: decomposing task".into(),
            "10:00:01  Worker started: agent-a".into(),
        ];
        input.process_filter = Some("worker".into());

        let formatted = format_process_capture(&input, 20);

        assert!(formatted.contains("Worker started"));
        assert!(!formatted.contains("Planning:"));
        assert!(formatted.contains("total 2"));
    }

    #[test]
    fn process_capture_reports_empty_filter_match() {
        let mut input = InputState::default();
        input.run_events = vec!["10:00:00  Planning: decomposing task".into()];
        input.process_filter = Some("worker".into());

        let formatted = format_process_capture(&input, 20);

        assert_eq!(
            formatted,
            "No captured run process matching \"worker\" (1 total event(s))."
        );
    }

    #[test]
    fn run_event_humanizes_json_outputs_before_capture() {
        let task_id = uuid::Uuid::nil();
        let line = format_run_event(&RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: r#"claude assistant: {"type":"assistant","message":{"content":[{"type":"text","text":"Working on it"}]}}"#.into(),
        });

        assert!(line.contains("claude-code"));
        assert!(line.contains("Working on it"));
        assert!(!line.contains('{'));

        let line = format_run_event(&RunProgress::RoleOutput {
            role: "diagnostic".into(),
            content: r#"```json
{"approved":false,"issues":["missing test"]}
```"#
                .into(),
        });

        assert!(line.contains("needs changes: 1 issue(s)"));
        assert!(line.contains("missing test"));
        assert!(!line.contains('{'));

        let line = format_run_event(&RunProgress::RoleOutput {
            role: "critic".into(),
            content: r#"{"approved":false,"issues":["missing test"]}"#.into(),
        });

        assert!(line.contains("critic output"));
        assert!(line.contains("needs changes: 1 issue(s)"));
        assert!(line.contains("missing test"));
        assert!(!line.contains('{'));
    }

    #[test]
    fn record_run_progress_skips_low_value_and_duplicate_outputs() {
        let task_id = uuid::Uuid::nil();
        let mut input = InputState::default();

        record_run_progress(
            &mut input,
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: "claude system".into(),
            },
        );
        assert!(input.run_events.is_empty());

        record_run_progress(
            &mut input,
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude assistant: useful summary".into(),
            },
        );
        record_run_progress(
            &mut input,
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude result: useful summary".into(),
            },
        );

        assert_eq!(input.run_events.len(), 1);
        assert!(input.run_events[0].contains("useful summary"));
    }

    #[test]
    fn process_capture_humanizes_existing_raw_json_lines() {
        let mut input = InputState::default();
        input.run_events = vec![
            r#"10:00:00  Worker output: abcdef12 claude assistant: {"type":"assistant","message":{"content":[{"type":"text","text":"Working on it"}]}}"#.into(),
        ];

        let formatted = format_process_capture(&input, 20);

        assert!(formatted.contains("abcdef12 claude: Working on it"));
        assert!(!formatted.contains('{'));
    }

    #[test]
    fn process_summary_keeps_key_events_and_hides_noisy_outputs() {
        let mut input = InputState::default();
        input.run_events = vec![
            "10:00:00  Planning: decomposing task".into(),
            "10:00:01  Worker output: abcdef12 incremental log line".into(),
            "10:00:02  Worker started: claude-code (abcdef12)".into(),
            "10:00:03  Worker done: abcdef12".into(),
            "10:00:04  Done: 1 sub-task(s) completed".into(),
        ];

        let formatted = format_process_summary(&input);

        assert!(formatted.contains("Process summary"));
        assert!(formatted.contains("Planning:"));
        assert!(formatted.contains("Worker started"));
        assert!(formatted.contains("Done:"));
        assert!(!formatted.contains("incremental log line"));
        assert!(formatted.contains("/process full"));
    }

    #[test]
    fn failure_context_includes_recent_key_events() {
        let mut input = InputState::default();
        input.run_events = vec![
            "10:00:00  Planning: decomposing task".into(),
            "10:00:01  Worker started: claude-code (abcdef12)".into(),
            "10:00:02  Worker output: abcdef12 verbose progress".into(),
            "10:00:03  Failed: command exited 1".into(),
        ];

        let formatted = format_failure_context(&input).expect("failure context");

        assert!(formatted.contains("Recent process:"));
        assert!(formatted.contains("Planning:"));
        assert!(formatted.contains("Worker started"));
        assert!(formatted.contains("Failed: command exited 1"));
        assert!(!formatted.contains("verbose progress"));
        assert!(formatted.contains("/process full"));
    }

    #[test]
    fn live_trace_refresh_creates_persistent_block() {
        let mut input = InputState::default();
        input.trace_live_enabled = true;
        input.run_events = vec![
            "10:00:00  Planning: decomposing task".into(),
            "10:00:01  Planned: 2 sub-task(s)".into(),
            "10:00:02  Executing: 2 sub-task(s)".into(),
            "10:00:03  Worker output: abcdef12 running cargo test".into(),
        ];

        refresh_live_trace(&mut input, false);

        let msg = input.transcript.last().expect("trace message");
        assert_eq!(msg.role, "trace");
        assert_eq!(msg.status, "complete");
        assert!(msg
            .content
            .contains("status: ○ idle | compact | 4 event(s)"));
        assert!(msg
            .content
            .contains("now: ● worker - abcdef12: running cargo test"));
        assert!(msg.content.contains("recent activity"));
        assert!(msg.content.contains("planning work"));
        assert!(msg.content.contains("full log: /process 200"));
    }

    #[test]
    fn live_trace_expanded_shows_longer_window() {
        let mut input = InputState::default();
        input.trace_live_enabled = true;
        input.trace_expanded = true;
        input.run_events = (0..12)
            .map(|i| format!("10:00:{:02}  event {}", i, i))
            .collect();

        refresh_live_trace(&mut input, false);

        let msg = input.transcript.last().expect("trace message");
        assert!(msg.content.contains("full"));
        assert!(msg.content.contains("event 0"));
        assert!(!msg.content.contains("/trace full expands"));
        assert!(msg.content.contains("full log: /process 200"));
    }
}
