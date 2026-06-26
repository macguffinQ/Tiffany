//! Shared run lifecycle and progress state for the terminal chat UI.

use super::commands::push_system;
use super::context::build_contextual_prompt;
use super::process::{
    create_process_capture_file, format_failure_context, record_run_event, record_run_progress,
    refresh_live_trace, start_live_trace,
};
use super::queue_state::{
    can_start_queued_batch, drain_queued_prompts, queue_followup, run_is_active,
};
use super::run_progress_view::{run_status_view, RunStatusView};
use super::state::{ChatMsg, InputState};
use super::util::{humanize_jsonish, is_low_value_execution_output, truncate_chars};
use crate::agent_events;
use crate::core::session_store::SessionStore;
use crate::core::types::Task;
use crate::pipeline::orchestrator::{Orchestrator, RunProgress};
use crate::task_policy::classify_task_route;
use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use unicode_width::UnicodeWidthChar;

const FINAL_RESULT_WRAP_WIDTH: usize = 100;
const FINAL_OUTPUT_MAX_CHARS: usize = 240_000;
const RUN_CHAT_OUTPUT_MAX_CHARS: usize = FINAL_OUTPUT_MAX_CHARS;

pub(super) struct RunController {
    orch: Arc<Orchestrator>,
    store: Arc<SessionStore>,
}

impl RunController {
    pub(super) fn new(orch: Arc<Orchestrator>, store: Arc<SessionStore>) -> Self {
        Self { orch, store }
    }

    pub(super) fn start_user_prompt(&self, prompt: String, input: &mut InputState) {
        self.start_prompt(prompt, input, "complete");
    }

    pub(super) fn start_next_queued(&self, input: &mut InputState) {
        if !can_start_queued_batch(input) {
            return;
        }

        let prompt = drain_queued_prompts(input);
        self.start_prompt(prompt, input, "queued_batch");
    }

    fn start_prompt(&self, prompt: String, input: &mut InputState, user_status: &str) {
        if run_is_active(input) {
            queue_followup(input, prompt);
            return;
        }

        input.last_prompt = Some(prompt.clone());
        let contextual_prompt = build_contextual_prompt(input, &prompt);
        input.last_context_messages = contextual_prompt.message_count;
        input.last_context_chars = contextual_prompt.context_chars;
        let task_prompt = contextual_prompt.prompt;
        apply_route_preview(input, &task_prompt);

        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: prompt.clone(),
            ts: std::time::SystemTime::now(),
            status: user_status.into(),
        });

        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: initial_run_status(input, &prompt),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<RunProgress>();
        input.run_rx = Some(rx);
        input.current_stage = initial_stage_label(input);
        input.current_stage_detail.clear();
        input.last_event_at = Some(Instant::now());
        input.run_events.clear();
        input.run_final_output = None;
        input.run_last_worker_output = None;
        input.run_review_issue_count = 0;
        input.run_review_unavailable_count = 0;
        input.run_worker_failure_count = 0;
        input.run_visible_output_keys.clear();
        input.run_visible_status_keys.clear();
        input.run_recorded_output_keys.clear();
        input.run_chat_output_keys.clear();
        input.process_capture_path = match create_process_capture_file(&self.store, &prompt) {
            Ok(path) => Some(path),
            Err(e) => {
                push_system(
                    input,
                    format!("Process capture file could not be created: {}", e),
                );
                None
            }
        };
        record_run_event(
            input,
            &format!("Started run: {}", truncate_chars(&prompt, 180)),
        );
        if let Some(preview) = route_preview_event(input) {
            record_run_event(input, &preview);
        }
        if input.last_context_messages > 0 {
            record_run_event(
                input,
                &format!(
                    "Injected context: {} message(s), {} chars",
                    input.last_context_messages, input.last_context_chars
                ),
            );
        }
        if let Some(agent) = input.agent_hint.clone() {
            record_run_event(input, &format!("Agent hint: {}", agent));
        }
        if let Some(agent) = input.cc_agent_hint.clone() {
            record_run_event(input, &format!("Claude subagent: {}", agent));
        }
        start_live_trace(input);

        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        input.cancel_flag = Some(cancel_flag.clone());

        let orch = self.orch.clone();
        let agent_hint = input.agent_hint.clone();
        let cc_agent_hint = input.cc_agent_hint.clone();
        let handle = tokio::spawn(async move {
            let mut task = Task::new(task_prompt);
            task.agent_hint = agent_hint;
            task.cc_agent_hint = cc_agent_hint;
            let _ = run_with_cancel(orch, task, tx, cancel_flag).await;
        });
        input.run_handle = Some(handle);
    }
}

fn initial_run_status(input: &InputState, prompt: &str) -> String {
    let worker = input.agent_hint.as_deref().unwrap_or("auto");
    let flow = input.run_route_label.as_deref().unwrap_or("auto");
    let flow_reason = input.run_route_reason_label.as_deref().unwrap_or("pending");
    let flow_steps = input
        .run_flow_steps
        .as_deref()
        .unwrap_or("direct/single/full decided at run start");
    let context = if input.last_context_messages == 0 {
        "none".to_string()
    } else {
        format!(
            "{} message(s), {} chars",
            input.last_context_messages, input.last_context_chars
        )
    };
    format!(
        "Working\n  status: starting orchestrator\n  flow: {} · {}\n  steps: {}\n  worker: {}\n  claude agent: {}\n  context: {}\n  request: {}\n\nDetails: /process 200 · /o detail",
        flow,
        flow_reason,
        flow_steps,
        worker,
        input.cc_agent_hint.as_deref().unwrap_or("default"),
        context,
        truncate_chars(prompt, 120)
    )
}

fn apply_route_preview(input: &mut InputState, task_prompt: &str) {
    let route = classify_task_route(&Task::new(task_prompt));
    input.run_route = Some(route.label().to_string());
    input.run_route_label = Some(route.display_label().to_string());
    input.run_route_reason = Some(route.reason().to_string());
    input.run_route_reason_label = Some(route.short_reason_label().to_string());
    input.run_flow_steps = Some(route.flow_steps().to_string());
}

fn initial_stage_label(input: &InputState) -> String {
    input
        .run_route_label
        .as_deref()
        .map(|flow| format!("starting {flow} flow"))
        .unwrap_or_else(|| "starting...".into())
}

fn route_preview_event(input: &InputState) -> Option<String> {
    let route = input.run_route_label.as_deref()?;
    let reason = input.run_route_reason_label.as_deref().unwrap_or("pending");
    let steps = input.run_flow_steps.as_deref().unwrap_or("pending");
    Some(format!("route  preview · {route} · {reason} · {steps}"))
}

/// Run the orchestrator, but check the cancel flag before starting.
async fn run_with_cancel(
    orch: Arc<Orchestrator>,
    task: Task,
    tx: tokio::sync::mpsc::UnboundedSender<RunProgress>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use std::sync::atomic::Ordering;
    if cancel.load(Ordering::Relaxed) {
        let _ = tx.send(RunProgress::Failed("cancelled by user".into()));
        return Ok(());
    }
    let _ = orch.run_with_progress(task, tx).await;
    Ok(())
}

/// Handle progress events from the running orchestrator.
pub(super) fn handle_run_event(event: RunProgress, input: &mut InputState) -> bool {
    input.last_event_at = Some(Instant::now());
    let terminal_event = matches!(&event, RunProgress::Done { .. } | RunProgress::Failed(_));
    record_run_progress(input, &event);
    if let Some(view) = run_status_view_for_input(&event, input) {
        apply_run_status_view(input, view);
    }

    match event {
        RunProgress::RouteSelected { route, reason }
        | RunProgress::RouteUpdated { route, reason } => {
            let parsed_route =
                crate::agent_events::OrchestrationRoute::from_label_or_reason(&route, &reason);
            input.run_route_label = parsed_route.map(|route| route.display_label().to_string());
            input.run_route_reason_label =
                parsed_route.map(|route| route.short_reason_label().to_string());
            input.run_flow_steps = parsed_route.map(|route| route.flow_steps().to_string());
            input.run_route = Some(route);
            input.run_route_reason = Some(reason);
        }
        RunProgress::Planning
        | RunProgress::Planned { .. }
        | RunProgress::WorkerReady { .. }
        | RunProgress::Critiquing { .. }
        | RunProgress::CritiqueResult { .. }
        | RunProgress::Replanning { .. }
        | RunProgress::ControlFallback { .. }
        | RunProgress::DirectAnswer
        | RunProgress::Executing { .. }
        | RunProgress::WorkerThreadReady { .. }
        | RunProgress::WorkerStarted { .. }
        | RunProgress::Reviewing { .. }
        | RunProgress::ReviewSkipped { .. } => {}
        RunProgress::WorkerOutput {
            event_kind,
            content,
            ..
        } => {
            remember_worker_output(input, &event_kind, &content);
            capture_worker_output_as_chat(input, &event_kind, &content);
        }
        RunProgress::RoleOutput { .. } => {}
        RunProgress::WorkerDone {
            task_id,
            agent,
            role,
            duration_ms,
            ok,
        } => {
            if !ok {
                input.run_worker_failure_count += 1;
            }
            let _ = (task_id, agent, role, duration_ms);
        }
        RunProgress::ReviewResult {
            approved, issues, ..
        } => {
            if !approved {
                input.run_review_issue_count += issues.max(1);
            }
        }
        RunProgress::ReviewUnavailable { .. } => {
            input.run_review_unavailable_count += 1;
        }
        RunProgress::Done { task_count } => {
            let review_issues = input.run_review_issue_count;
            let review_unavailable = input.run_review_unavailable_count;
            let worker_failures = input.run_worker_failure_count;
            let completion_detail = completion_detail(input, task_count, worker_failures);
            input.current_stage = if review_issues > 0 {
                "Review needs fixes".into()
            } else if review_unavailable > 0 && worker_failures == 0 {
                "Done; review unavailable".into()
            } else if worker_failures > 0 && task_count == 0 {
                "Worker failed".into()
            } else if worker_failures > 0 {
                "Done with worker failures".into()
            } else {
                "Done".into()
            };
            input.current_stage_detail = if review_issues > 0 {
                format!("{completion_detail}, {} review issue(s)", review_issues)
            } else if review_unavailable > 0 {
                format!(
                    "{completion_detail}, {} review unavailable",
                    review_unavailable
                )
            } else if worker_failures > 0 {
                format!("{completion_detail}, {worker_failures} worker failure(s)")
            } else {
                completion_detail
            };
            let final_output = final_output_for_run(input);
            input.last_result_output = final_output.clone();
            let no_completed_message = format_no_completed_tasks_message(input, worker_failures);
            if let Some(last) = input
                .transcript
                .iter_mut()
                .rev()
                .find(|msg| msg.role == "assistant")
            {
                last.status = if review_issues > 0 || review_unavailable > 0 || worker_failures > 0
                {
                    "warning".into()
                } else {
                    "complete".into()
                };
                if task_count == 0 && worker_failures > 0 {
                    last.status = "error".into();
                    last.content = no_completed_message.clone();
                } else if task_count == 0 {
                    last.content = no_completed_message.clone();
                } else if review_issues > 0 {
                    last.content = format_done_with_review_issues_message(final_output.as_deref());
                } else if review_unavailable > 0 {
                    last.content = format_done_with_review_unavailable_message(
                        final_output.as_deref(),
                        review_unavailable,
                    );
                } else {
                    last.content = format_done_message(final_output.as_deref());
                }
            }
            input.run_rx = None;
            input.run_handle = None;
            input.cancel_flag = None;
        }
        RunProgress::Failed(msg) => {
            input.current_stage = if msg == "cancelled by user" {
                "Cancelled".into()
            } else {
                "Failed".into()
            };
            input.current_stage_detail = msg.clone();
            let failure_context = if msg == "cancelled by user" {
                None
            } else {
                format_failure_context(input)
            };
            if let Some(last) = input
                .transcript
                .iter_mut()
                .rev()
                .find(|msg| msg.role == "assistant")
            {
                if msg == "cancelled by user" {
                    last.status = "complete".into();
                    last.content = "○ cancelled".into();
                } else {
                    last.status = "error".into();
                    last.content = format_failed_message(&msg, failure_context);
                }
            }
            input.run_rx = None;
            input.cancel_flag = None;
            input.run_handle = None;
        }
    }
    refresh_live_trace(input, terminal_event);
    terminal_event
}

fn apply_run_status_view(input: &mut InputState, view: RunStatusView) {
    input.current_stage = view.stage;
    input.current_stage_detail = view.detail;
    if let Some(update) = view.assistant_update {
        update_last_assistant(input, update, "thinking");
    }
}

fn run_status_view_for_input(event: &RunProgress, input: &InputState) -> Option<RunStatusView> {
    if let RunProgress::Planned { sub_task_count } = event {
        match input.run_route.as_deref() {
            Some("single-worker") => {
                return Some(RunStatusView {
                    stage: format!("Worker ready ({sub_task_count} run(s))"),
                    detail: "single-worker flow".into(),
                    assistant_update: Some(format!(
                        "▸ Worker ready · {sub_task_count} run(s). Starting worker…"
                    )),
                });
            }
            Some("direct-answer") => {
                return Some(RunStatusView {
                    stage: "Direct answer ready".into(),
                    detail: "worker flow".into(),
                    assistant_update: Some("▸ Direct answer ready. Starting worker…".into()),
                });
            }
            _ => {}
        }
    }

    run_status_view(event)
}

fn update_last_assistant(input: &mut InputState, content: String, status: &str) {
    if let Some(last) = input
        .transcript
        .iter_mut()
        .rev()
        .find(|msg| msg.role == "assistant")
    {
        last.content = content;
        last.status = status.into();
    }
}

fn remember_worker_output(input: &mut InputState, event_kind: &str, content: &str) {
    let display = humanize_jsonish(content, FINAL_OUTPUT_MAX_CHARS);
    if display.trim().is_empty() || is_low_value_worker_output(&display) {
        return;
    }

    if let Some(final_output) = worker_final_output_candidate(event_kind, content) {
        remember_better_text(&mut input.run_final_output, final_output);
    }
    let normalized = normalize_chat_worker_output(&display);
    if !is_low_value_worker_output(&normalized) {
        remember_better_text(&mut input.run_last_worker_output, normalized);
    }
}

fn capture_worker_output_as_chat(input: &mut InputState, event_kind: &str, content: &str) {
    if worker_final_output_candidate(event_kind, content).is_some() {
        return;
    }

    let display = humanize_jsonish(content, RUN_CHAT_OUTPUT_MAX_CHARS);
    if display.trim().is_empty() || is_low_value_worker_output(&display) {
        return;
    }

    let normalized = normalize_chat_worker_output(&display);
    if normalized.trim().is_empty() || is_low_value_worker_output(&normalized) {
        return;
    }
    if looks_like_final_answer(event_kind, &normalized) {
        return;
    }

    let key = truncate_chars(&normalized, 300);
    if input.run_chat_output_keys.iter().any(|seen| seen == &key) {
        return;
    }
    input.run_chat_output_keys.push(key);
    if input.run_chat_output_keys.len() > 160 {
        input.run_chat_output_keys.remove(0);
    }

    let msg = ChatMsg {
        role: "tool".into(),
        content: normalized,
        ts: std::time::SystemTime::now(),
        status: "complete".into(),
    };

    if let Some(idx) = input
        .transcript
        .iter()
        .rposition(|msg| msg.role == "assistant" && msg.status == "thinking")
    {
        input.transcript.insert(idx, msg);
    } else {
        input.transcript.push(msg);
    }
}

fn looks_like_final_answer(event_kind: &str, text: &str) -> bool {
    if agent_events::visible_agent_output_kind_for_event_kind(event_kind)
        .is_some_and(|kind| kind.is_process_event())
    {
        return false;
    }
    let text = text.trim();
    text.lines().filter(|line| !line.trim().is_empty()).count() >= 2 || text.chars().count() > 180
}

fn normalize_chat_worker_output(display: &str) -> String {
    let normalized = agent_events::normalize_output_summary(display);
    let trimmed = normalized.trim();
    let Some((prefix, body)) = trimmed.split_once(':') else {
        return trimmed.to_string();
    };
    let kind = prefix.split_whitespace().last().unwrap_or_default();
    if matches!(kind, "tool" | "tool_use" | "tool_result" | "exec") {
        body.trim_start().to_string()
    } else {
        trimmed.to_string()
    }
}

fn remember_better_text(slot: &mut Option<String>, candidate: String) {
    let candidate = candidate.trim().to_string();
    if candidate.is_empty() {
        return;
    }
    let should_replace = slot
        .as_ref()
        .map(|current| candidate.chars().count() > current.chars().count())
        .unwrap_or(true);
    if should_replace {
        *slot = Some(candidate);
    }
}

fn worker_final_output_candidate(event_kind: &str, content: &str) -> Option<String> {
    if agent_events::visible_agent_output_kind_for_event_kind(event_kind)
        .is_some_and(|kind| kind.is_process_event())
    {
        return None;
    }
    agent_events::final_output_candidate(content, FINAL_OUTPUT_MAX_CHARS)
}

fn final_output_for_run(input: &InputState) -> Option<String> {
    let mut best = None;
    for text in [&input.run_final_output, &input.run_last_worker_output]
        .into_iter()
        .flatten()
    {
        remember_better_text(&mut best, humanize_jsonish(text, FINAL_OUTPUT_MAX_CHARS));
    }
    best.filter(|text| !text.trim().is_empty())
}

fn format_done_message(final_output: Option<&str>) -> String {
    format_completion_notice("✓ done", final_output)
}

fn format_done_with_review_issues_message(final_output: Option<&str>) -> String {
    format_completion_notice("⚠ completed with review issues", final_output)
}

fn format_done_with_review_unavailable_message(final_output: Option<&str>, count: usize) -> String {
    let mut out = format_completion_notice("✓ done · review unavailable", final_output);
    let review = if count == 1 {
        "Reviewer control check was unavailable; worker output was kept."
    } else {
        "Reviewer control checks were unavailable; worker output was kept."
    };
    out.push('\n');
    out.push_str(review);
    out
}

fn completion_detail(input: &InputState, task_count: usize, worker_failures: usize) -> String {
    let route = input.run_route.as_deref();
    match route {
        Some("direct-answer") => {
            if task_count > 0 {
                "answer complete".into()
            } else {
                "no answer completed".into()
            }
        }
        Some("single-worker") => {
            if task_count > 0 {
                "worker complete".into()
            } else if worker_failures > 0 {
                "worker failed".into()
            } else {
                "no worker result completed".into()
            }
        }
        _ => {
            if task_count == 1 {
                "1 worker run completed".into()
            } else {
                format!("{task_count} worker runs completed")
            }
        }
    }
}

fn format_completion_notice(status: &str, final_output: Option<&str>) -> String {
    let mut out = status.to_string();
    if final_output
        .map(str::trim)
        .is_some_and(|text| !text.is_empty())
    {
        out.push_str("\n\nResult captured. Use /result to show it or /copy result to copy.");
    } else {
        out.push_str("\n\nNo final text was emitted by the worker.");
    }
    out.push_str("\nDetails: /process 200");
    out
}

fn format_no_completed_tasks_message(input: &InputState, worker_failures: usize) -> String {
    let detail = completion_detail(input, 0, worker_failures);
    if worker_failures > 0 {
        format!("✗ {detail} — {worker_failures} worker failure(s)\n\nDetails: /process 200")
    } else {
        format!("✗ {detail}\n\nDetails: /process 200")
    }
}

fn format_failed_message(message: &str, context: Option<String>) -> String {
    let mut out = format!("✗ error: {}", message);
    if let Some(context) = context {
        out.push_str("\n\n");
        out.push_str(&context);
    } else {
        out.push_str("\n\nDetails: /process full or /trace full");
    }
    out
}

pub(super) fn format_final_result(final_output: Option<&str>) -> String {
    let raw_lines: Vec<String> = match final_output {
        Some(text) if !text.trim().is_empty() => text.trim().lines().map(str::to_string).collect(),
        _ => vec![
            "No final text was emitted by the worker.".into(),
            "Use /process 200 to inspect the captured run.".into(),
        ],
    };

    let wrapped_lines: Vec<String> = raw_lines
        .iter()
        .flat_map(|line| wrap_display_line(line, FINAL_RESULT_WRAP_WIDTH))
        .collect();

    let mut out = String::from("Final result:\n");
    for (idx, line) in wrapped_lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

fn wrap_display_line(line: &str, max_width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for ch in line.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width > 0 && current_width + char_width > max_width {
            rows.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += char_width;
    }

    if !current.is_empty() {
        rows.push(current);
    }

    rows
}

fn is_low_value_worker_output(text: &str) -> bool {
    is_low_value_execution_output(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_message_stays_out_of_transcript_until_execution() {
        let mut input = InputState::default();
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "running task".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "thinking...".into(),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        queue_followup(&mut input, "next task".into());

        assert_eq!(input.queued_prompts, vec!["next task"]);
        assert_eq!(
            input
                .transcript
                .iter()
                .filter(|msg| msg.role == "user")
                .count(),
            1
        );
        assert!(!input
            .transcript
            .iter()
            .any(|msg| msg.status == "queued" && msg.content == "next task"));
    }

    #[test]
    fn queued_messages_merge_into_one_prompt_when_started() {
        let merged = crate::tui::queue_state::merge_queued_prompts(vec![
            "first change".into(),
            "second change".into(),
            "  ".into(),
            "third change".into(),
        ]);

        assert_eq!(
            merged,
            "1. first change\n\n2. second change\n\n3. third change"
        );
    }

    #[test]
    fn paused_queue_is_not_drained_by_start_next_queued() {
        let mut input = InputState {
            queued_prompts: vec!["wait".into()],
            queue_paused: true,
            ..InputState::default()
        };

        assert!(!can_start_queued_batch(&input));

        assert_eq!(input.queued_prompts, vec!["wait"]);
        assert!(input.run_rx.is_none());
        assert!(input.run_handle.is_none());

        input.queue_paused = false;
        assert!(can_start_queued_batch(&input));
    }

    #[test]
    fn final_result_is_extracted_from_worker_result_output() {
        let result = worker_final_output_candidate(
            "result",
            "claude-code result: {\"result\":\"Implemented the requested TUI changes.\"}",
        )
        .expect("final result");

        assert_eq!(result, "Implemented the requested TUI changes.");

        let message = format_done_message(Some(&result));
        assert!(message.contains("✓ done"));
        assert!(message.contains("Result captured."));
        assert!(message.contains("/result"));
        assert!(!message.contains("Implemented the requested TUI changes."));
    }

    #[test]
    fn done_message_hides_final_result_but_keeps_result_state() {
        let mut input = InputState::default();
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "thinking...".into(),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        handle_run_event(
            RunProgress::WorkerOutput {
                task_id: uuid::Uuid::nil(),
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                event_kind: "result".into(),
                content: r#"claude-code result: {"result":"final answer body"}"#.into(),
            },
            &mut input,
        );
        handle_run_event(RunProgress::Done { task_count: 1 }, &mut input);

        let msg = input.transcript.last().expect("assistant message");
        assert_eq!(msg.status, "complete");
        assert!(msg.content.contains("Result captured."));
        assert!(!msg.content.contains("final answer body"));
        assert_eq!(
            input.last_result_output.as_deref(),
            Some("final answer body")
        );
    }

    #[test]
    fn final_result_keeps_longer_complete_worker_output() {
        let mut input = InputState::default();

        remember_worker_output(
            &mut input,
            "result",
            "claude-code result: first paragraph\nsecond paragraph",
        );
        remember_worker_output(&mut input, "result", "claude-code result: short");

        let result = final_output_for_run(&input).expect("final output");
        assert_eq!(result, "first paragraph\nsecond paragraph");
    }

    #[test]
    fn final_result_prefers_longer_worker_output_over_short_result_event() {
        let mut input = InputState::default();

        remember_worker_output(
            &mut input,
            "assistant",
            "claude-code assistant: full answer paragraph one\nfull answer paragraph two",
        );
        remember_worker_output(&mut input, "result", "claude-code result: done");

        let result = final_output_for_run(&input).expect("final output");
        assert_eq!(
            result,
            "full answer paragraph one\nfull answer paragraph two"
        );
    }

    #[test]
    fn worker_process_output_is_captured_as_tool_message() {
        let mut input = InputState::default();

        capture_worker_output_as_chat(
            &mut input,
            "tool_use",
            "claude-code tool_use: Bash(cargo test)",
        );
        capture_worker_output_as_chat(
            &mut input,
            "tool_use",
            "claude-code tool_use: Bash(cargo test)",
        );

        assert_eq!(input.transcript.len(), 1);
        assert_eq!(input.transcript[0].role, "tool");
        assert_eq!(input.transcript[0].content, "Bash(cargo test)");
    }

    #[test]
    fn long_tool_result_is_not_hidden_as_final_answer() {
        let mut input = InputState::default();
        let long_tool_result = format!(
            "claude-code tool_result: tool result: first line\n{}\nEND",
            "x".repeat(12_000)
        );

        capture_worker_output_as_chat(&mut input, "tool_result", &long_tool_result);

        assert_eq!(input.transcript.len(), 1);
        assert_eq!(input.transcript[0].role, "tool");
        assert!(input.transcript[0].content.contains("first line"));
        assert!(input.transcript[0].content.contains("END"));
    }

    #[test]
    fn route_event_updates_run_route_state() {
        let mut input = InputState::default();
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "starting".into(),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        handle_run_event(
            RunProgress::RouteSelected {
                route: "full-pipeline".into(),
                reason: "project or implementation work".into(),
            },
            &mut input,
        );

        assert_eq!(input.run_route.as_deref(), Some("full-pipeline"));
        assert_eq!(input.run_route_label.as_deref(), Some("full"));
        assert_eq!(
            input.run_route_reason.as_deref(),
            Some("project or implementation work")
        );
        assert_eq!(
            input.run_route_reason_label.as_deref(),
            Some("implementation")
        );
        assert_eq!(
            input.run_flow_steps.as_deref(),
            Some("planner -> critic -> worker -> reviewer -> answer")
        );
        assert_eq!(input.current_stage, "route: full");
    }

    #[test]
    fn initial_status_shows_route_preview_before_backend_event() {
        let mut input = InputState::default();

        apply_route_preview(
            &mut input,
            "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle",
        );
        let status = initial_run_status(
            &input,
            "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle",
        );

        assert_eq!(input.run_route.as_deref(), Some("single-worker"));
        assert_eq!(input.run_route_label.as_deref(), Some("single"));
        assert_eq!(
            input.run_route_reason_label.as_deref(),
            Some("atomic worker")
        );
        assert_eq!(input.run_flow_steps.as_deref(), Some("worker -> answer"));
        assert_eq!(initial_stage_label(&input), "starting single flow");
        assert_eq!(
            route_preview_event(&input).as_deref(),
            Some("route  preview · single · atomic worker · worker -> answer")
        );
        assert!(status.contains("flow: single · atomic worker"));
        assert!(status.contains("steps: worker -> answer"));
        assert!(!status.contains("planner -> critic"));
    }

    #[test]
    fn route_event_can_replace_route_preview() {
        let mut input = InputState::default();
        apply_route_preview(&mut input, "你好");
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: initial_run_status(&input, "你好"),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        handle_run_event(
            RunProgress::RouteSelected {
                route: "full-pipeline".into(),
                reason: "project or implementation work".into(),
            },
            &mut input,
        );

        assert_eq!(input.run_route.as_deref(), Some("full-pipeline"));
        assert_eq!(input.run_route_label.as_deref(), Some("full"));
        assert_eq!(
            input.run_flow_steps.as_deref(),
            Some("planner -> critic -> worker -> reviewer -> answer")
        );
        assert_eq!(input.current_stage, "route: full");
    }

    #[test]
    fn route_update_downgrades_active_flow_state() {
        let mut input = InputState::default();
        apply_route_preview(&mut input, "优化当前工程");
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: initial_run_status(&input, "优化当前工程"),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        handle_run_event(
            RunProgress::RouteUpdated {
                route: "single-worker".into(),
                reason: "planner unavailable; downgraded to single worker".into(),
            },
            &mut input,
        );

        assert_eq!(input.run_route.as_deref(), Some("single-worker"));
        assert_eq!(input.run_route_label.as_deref(), Some("single"));
        assert_eq!(
            input.run_route_reason_label.as_deref(),
            Some("atomic worker")
        );
        assert_eq!(input.run_flow_steps.as_deref(), Some("worker -> answer"));
        assert_eq!(input.current_stage, "route: single");
    }

    #[test]
    fn planned_status_respects_single_worker_route_downgrade() {
        let mut input = InputState::default();
        apply_route_preview(&mut input, "优化当前工程");
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: initial_run_status(&input, "优化当前工程"),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        handle_run_event(
            RunProgress::RouteUpdated {
                route: "single-worker".into(),
                reason: "planner fallback; downgraded to single worker".into(),
            },
            &mut input,
        );
        handle_run_event(RunProgress::Planned { sub_task_count: 1 }, &mut input);

        assert_eq!(input.current_stage, "Worker ready (1 run(s))");
        assert_eq!(input.current_stage_detail, "single-worker flow");
        let assistant = input
            .transcript
            .last()
            .expect("assistant status message after planned");
        assert!(assistant.content.contains("Worker ready · 1 run(s)"));
        assert!(!assistant.content.contains("Moving to critique"));
    }

    #[test]
    fn final_like_worker_output_stays_for_assistant_result() {
        let mut input = InputState::default();

        capture_worker_output_as_chat(
            &mut input,
            "assistant",
            "claude-code assistant: paragraph one\nparagraph two",
        );

        assert!(input.transcript.is_empty());
    }

    #[test]
    fn worker_failure_done_message_is_not_reported_as_empty_plan() {
        let mut input = InputState {
            run_route: Some("single-worker".into()),
            ..InputState::default()
        };
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "thinking...".into(),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        let task_id = uuid::Uuid::nil();
        handle_run_event(
            RunProgress::WorkerDone {
                task_id,
                agent: "test-worker".into(),
                role: "worker-cc".into(),
                duration_ms: 25,
                ok: false,
            },
            &mut input,
        );
        handle_run_event(RunProgress::Done { task_count: 0 }, &mut input);

        let msg = input.transcript.last().expect("assistant message");
        assert_eq!(msg.status, "error");
        assert!(msg.content.contains("worker failed"));
        assert!(msg.content.contains("worker failure"));
        assert!(!msg.content.contains("0 worker runs completed"));
        assert!(!msg.content.contains("sub-task"));
    }

    #[test]
    fn direct_answer_completion_uses_answer_language() {
        let mut input = InputState {
            run_route: Some("direct-answer".into()),
            ..InputState::default()
        };
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "thinking...".into(),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        handle_run_event(RunProgress::Done { task_count: 1 }, &mut input);

        assert_eq!(input.current_stage_detail, "answer complete");
    }

    #[test]
    fn review_unavailable_completion_keeps_worker_result() {
        let mut input = InputState {
            run_route: Some("full-pipeline".into()),
            ..InputState::default()
        };
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "thinking...".into(),
            ts: std::time::SystemTime::now(),
            status: "thinking".into(),
        });

        let task_id = uuid::Uuid::nil();
        handle_run_event(
            RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                event_kind: "result".into(),
                content: r#"claude-code result: {"result":"implemented orchestration flow"}"#
                    .into(),
            },
            &mut input,
        );
        handle_run_event(
            RunProgress::ReviewUnavailable {
                task_id,
                message: "no JSON found in CLI response".into(),
            },
            &mut input,
        );
        handle_run_event(RunProgress::Done { task_count: 1 }, &mut input);

        let msg = input.transcript.last().expect("assistant message");
        assert_eq!(input.current_stage, "Done; review unavailable");
        assert_eq!(
            input.current_stage_detail,
            "1 worker run completed, 1 review unavailable"
        );
        assert_eq!(input.run_review_issue_count, 0);
        assert_eq!(input.run_review_unavailable_count, 1);
        assert_eq!(msg.status, "warning");
        assert!(msg.content.contains("✓ done · review unavailable"));
        assert!(msg.content.contains("Result captured."));
        assert!(msg.content.contains("worker output was kept"));
        assert!(!msg.content.contains("completed with review issues"));
        assert!(!msg.content.contains("needs fixes"));
        assert_eq!(
            input.last_result_output.as_deref(),
            Some("implemented orchestration flow")
        );
    }

    #[test]
    fn failed_message_includes_process_context() {
        let message = format_failed_message(
            "worker crashed",
            Some("Recent process:\n  10:00:00  Failed: worker crashed".into()),
        );

        assert!(message.contains("✗ error: worker crashed"));
        assert!(message.contains("Recent process:"));
        assert!(message.contains("Failed: worker crashed"));
    }
}
