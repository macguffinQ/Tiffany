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
use super::state::{ChatMsg, InputState};
use super::util::{
    format_duration_ms, humanize_jsonish, is_low_value_execution_output, truncate_chars,
};
use crate::agent_events;
use crate::core::session_store::SessionStore;
use crate::core::types::Task;
use crate::pipeline::orchestrator::{Orchestrator, RunProgress};
use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use unicode_width::UnicodeWidthChar;

const FINAL_RESULT_WRAP_WIDTH: usize = 100;
const FINAL_OUTPUT_MAX_CHARS: usize = 240_000;
const RUN_CHAT_OUTPUT_MAX_CHARS: usize = 8_000;

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
        input.current_stage = "starting...".into();
        input.current_stage_detail.clear();
        input.last_event_at = Some(Instant::now());
        input.run_events.clear();
        input.run_final_output = None;
        input.run_last_worker_output = None;
        input.run_review_issue_count = 0;
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
        start_live_trace(input);

        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        input.cancel_flag = Some(cancel_flag.clone());

        let orch = self.orch.clone();
        let agent_hint = input.agent_hint.clone();
        let handle = tokio::spawn(async move {
            let mut task = Task::new(task_prompt);
            task.agent_hint = agent_hint;
            let _ = run_with_cancel(orch, task, tx, cancel_flag).await;
        });
        input.run_handle = Some(handle);
    }
}

fn initial_run_status(input: &InputState, prompt: &str) -> String {
    let worker = input.agent_hint.as_deref().unwrap_or("auto");
    let context = if input.last_context_messages == 0 {
        "none".to_string()
    } else {
        format!(
            "{} message(s), {} chars",
            input.last_context_messages, input.last_context_chars
        )
    };
    format!(
        "Working\n  status: starting orchestrator\n  worker: {}\n  context: {}\n  request: {}\n\nDetails: /process 200 · /o detail",
        worker,
        context,
        truncate_chars(prompt, 120)
    )
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

    match event {
        RunProgress::Planning => {
            input.current_stage = "Planning".into();
            input.current_stage_detail = "decomposing task".into();
            update_last_assistant(input, "▸ Planning — decomposing task…".into(), "thinking");
        }
        RunProgress::Planned { sub_task_count } => {
            input.current_stage = format!("Planned ({} sub-tasks)", sub_task_count);
            input.current_stage_detail = "moving to critique".into();
            update_last_assistant(
                input,
                format!(
                    "▸ Planned {} sub-task(s). Moving to critique…",
                    sub_task_count
                ),
                "thinking",
            );
        }
        RunProgress::Critiquing { round } => {
            input.current_stage = format!("Critiquing (round {})", round);
            input.current_stage_detail = "adversarial review".into();
            update_last_assistant(
                input,
                format!("▸ Critiquing (round {}) — adversarial review…", round),
                "thinking",
            );
        }
        RunProgress::CritiqueResult { approved, issues } => {
            let msg = if approved {
                "✓ critic  plan approved".to_string()
            } else {
                format!("● critic  plan needs changes · {} issue(s)", issues)
            };
            input.current_stage = if approved {
                "critic: plan approved".into()
            } else {
                format!("critic: needs changes · {} issue(s)", issues)
            };
            input.current_stage_detail.clear();
            update_last_assistant(input, msg, "thinking");
        }
        RunProgress::Replanning { attempt } => {
            input.current_stage = format!("Replanning (attempt {})", attempt);
            input.current_stage_detail = "incorporating feedback".into();
            update_last_assistant(
                input,
                format!(
                    "▸ Replanning (attempt {}) — incorporating feedback…",
                    attempt
                ),
                "thinking",
            );
        }
        RunProgress::Executing { sub_task_count } => {
            input.current_stage = format!("Executing {} sub-task(s)", sub_task_count);
            input.current_stage_detail = "running workers".into();
            update_last_assistant(
                input,
                format!(
                    "▸ Executing {} sub-task(s) — running workers…",
                    sub_task_count
                ),
                "thinking",
            );
        }
        RunProgress::WorkerStarted {
            task_id,
            agent,
            role,
            runtime,
            model,
            provider,
            ..
        } => {
            let id8 = &task_id.to_string()[..8];
            input.current_stage = format!("worker: {} started · {}", role, id8);
            input.current_stage_detail = format!(
                "{} · {} · {}",
                runtime,
                provider_model_label(provider.as_deref(), &model),
                agent
            );
            update_last_assistant(
                input,
                format!(
                    "● worker  {} started · {} · {} · {}",
                    role,
                    runtime,
                    provider_model_label(provider.as_deref(), &model),
                    id8
                ),
                "thinking",
            );
        }
        RunProgress::WorkerOutput { content, .. } => {
            remember_worker_output(input, &content);
            capture_worker_output_as_chat(input, &content);
        }
        RunProgress::RoleOutput { .. } => {}
        RunProgress::WorkerDone {
            task_id,
            agent,
            role,
            duration_ms,
            ok,
        } => {
            let id8 = &task_id.to_string()[..8];
            if !ok {
                input.run_worker_failure_count += 1;
            }
            input.current_stage = if ok {
                format!(
                    "worker: {} done · {} · {}",
                    role,
                    id8,
                    format_duration_ms(duration_ms)
                )
            } else {
                format!(
                    "worker: {} failed · {} · {}",
                    role,
                    id8,
                    format_duration_ms(duration_ms)
                )
            };
            input.current_stage_detail = agent.clone();
        }
        RunProgress::Reviewing { task_id } => {
            let id8 = &task_id.to_string()[..8];
            input.current_stage = format!("review: checking worker output · {}", id8);
            input.current_stage_detail = "checking output".into();
            update_last_assistant(
                input,
                format!("● review  checking worker output · {}", id8),
                "thinking",
            );
        }
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            let id8 = &task_id.to_string()[..8];
            if !approved {
                input.run_review_issue_count += issues.max(1);
            }
            input.current_stage = if approved {
                format!("review: passed · {}", id8)
            } else {
                format!("review: needs fixes · {} · {} issue(s)", id8, issues)
            };
        }
        RunProgress::Done { task_count } => {
            let review_issues = input.run_review_issue_count;
            let worker_failures = input.run_worker_failure_count;
            input.current_stage = if review_issues > 0 {
                "Review needs fixes".into()
            } else if worker_failures > 0 && task_count == 0 {
                "Worker failed".into()
            } else if worker_failures > 0 {
                "Done with worker failures".into()
            } else {
                "Done".into()
            };
            input.current_stage_detail = if review_issues > 0 {
                format!(
                    "{} sub-task(s) completed, {} review issue(s)",
                    task_count, review_issues
                )
            } else if worker_failures > 0 && task_count == 0 {
                format!(
                    "0 sub-task(s) completed, {} worker failure(s)",
                    worker_failures
                )
            } else if worker_failures > 0 {
                format!(
                    "{} sub-task(s) completed, {} worker failure(s)",
                    task_count, worker_failures
                )
            } else {
                format!("{} sub-task(s) completed", task_count)
            };
            let final_output = final_output_for_run(input);
            input.last_result_output = final_output.clone();
            if let Some(last) = input
                .transcript
                .iter_mut()
                .rev()
                .find(|msg| msg.role == "assistant")
            {
                last.status = if review_issues > 0 || worker_failures > 0 {
                    "warning".into()
                } else {
                    "complete".into()
                };
                if task_count == 0 && worker_failures > 0 {
                    last.status = "error".into();
                    last.content = format_no_completed_tasks_message(worker_failures);
                } else if task_count == 0 {
                    last.content = "no sub-tasks were generated\n\nDetails: /process 200".into();
                } else if review_issues > 0 {
                    last.content = format_done_with_review_issues_message(final_output.as_deref());
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

fn provider_model_label(provider: Option<&str>, model: &str) -> String {
    match provider.map(str::trim).filter(|value| !value.is_empty()) {
        Some(provider) => format!("{provider}/{model}"),
        None => model.to_string(),
    }
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

fn remember_worker_output(input: &mut InputState, content: &str) {
    let display = humanize_jsonish(content, FINAL_OUTPUT_MAX_CHARS);
    if display.trim().is_empty() || is_low_value_worker_output(&display) {
        return;
    }

    if let Some(final_output) = worker_final_output_candidate(content) {
        remember_better_text(&mut input.run_final_output, final_output);
    }
    let normalized = normalize_chat_worker_output(&display);
    if !is_low_value_worker_output(&normalized) {
        remember_better_text(&mut input.run_last_worker_output, normalized);
    }
}

fn capture_worker_output_as_chat(input: &mut InputState, content: &str) {
    if worker_final_output_candidate(content).is_some() {
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
    if looks_like_final_answer(&normalized) {
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
        content: truncate_chars(&normalized, RUN_CHAT_OUTPUT_MAX_CHARS),
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

fn looks_like_final_answer(text: &str) -> bool {
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

fn worker_final_output_candidate(content: &str) -> Option<String> {
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

fn format_no_completed_tasks_message(worker_failures: usize) -> String {
    format!(
        "✗ no sub-tasks completed — {} worker failure(s)\n\nDetails: /process 200",
        worker_failures
    )
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
            "claude-code result: first paragraph\nsecond paragraph",
        );
        remember_worker_output(&mut input, "claude-code result: short");

        let result = final_output_for_run(&input).expect("final output");
        assert_eq!(result, "first paragraph\nsecond paragraph");
    }

    #[test]
    fn final_result_prefers_longer_worker_output_over_short_result_event() {
        let mut input = InputState::default();

        remember_worker_output(
            &mut input,
            "claude-code assistant: full answer paragraph one\nfull answer paragraph two",
        );
        remember_worker_output(&mut input, "claude-code result: done");

        let result = final_output_for_run(&input).expect("final output");
        assert_eq!(
            result,
            "full answer paragraph one\nfull answer paragraph two"
        );
    }

    #[test]
    fn worker_process_output_is_captured_as_tool_message() {
        let mut input = InputState::default();

        capture_worker_output_as_chat(&mut input, "claude-code tool_use: Bash(cargo test)");
        capture_worker_output_as_chat(&mut input, "claude-code tool_use: Bash(cargo test)");

        assert_eq!(input.transcript.len(), 1);
        assert_eq!(input.transcript[0].role, "tool");
        assert_eq!(input.transcript[0].content, "Bash(cargo test)");
    }

    #[test]
    fn final_like_worker_output_stays_for_assistant_result() {
        let mut input = InputState::default();

        capture_worker_output_as_chat(
            &mut input,
            "claude-code assistant: paragraph one\nparagraph two",
        );

        assert!(input.transcript.is_empty());
    }

    #[test]
    fn worker_failure_done_message_is_not_reported_as_empty_plan() {
        let mut input = InputState::default();
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
        assert!(msg.content.contains("worker failure"));
        assert!(!msg.content.contains("no sub-tasks were generated"));
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
