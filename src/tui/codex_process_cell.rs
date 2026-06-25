// Codex-style process cell rendering for tiffany-loop lightweight terminal runner.
//
// The upstream Codex implementation keeps command execution display in
// `codex-rs/tui/src/exec_cell/`. This module keeps the same ownership boundary
// for orchestrator progress: execution events become stable history lines here,
// while the terminal loop only inserts those lines into scrollback.

use super::run_progress_view::{progress_history_view, ProgressHistoryView, ProgressTone};
use super::state::InputState;
use super::util::{normalize_execution_output_summary, truncate_chars};
use crate::agent_events;
use crate::pipeline::orchestrator::RunProgress;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const TIFFANY: &str = "\x1b[38;2;129;216;208m";
const GREEN: &str = TIFFANY;
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const CYAN: &str = TIFFANY;
const PROGRESS_OUTPUT_WIDTH: usize = 104;
const PROGRESS_OUTPUT_LINE_LIMIT: usize = 36;
const PROGRESS_OUTPUT_FOLDED_LIMIT: usize = 220;
const PROGRESS_OUTPUT_EXPANDED_LIMIT: usize = 4_000;
const FINAL_OUTPUT_MAX_CHARS: usize = 240_000;

pub(super) fn progress_history_lines(event: &RunProgress, input: &mut InputState) -> Vec<String> {
    let Some((icon, color, line)) = progress_line(event, input.run_review_issue_count, input)
    else {
        return Vec::new();
    };
    if progress_was_already_visible(input, event, &line) {
        return Vec::new();
    }
    remember_visible_progress(input, event, &line);
    render_progress_line(icon, color, &line)
}

pub(super) fn progress_line(
    event: &RunProgress,
    review_issue_count: usize,
    input: &InputState,
) -> Option<(&'static str, &'static str, String)> {
    match event {
        RunProgress::WorkerOutput {
            task_id,
            agent,
            role,
            content,
        } => visible_worker_output_line(task_id, role, agent, content, input),
        RunProgress::RoleOutput { role, content } => visible_role_output_line(role, content, input),
        RunProgress::Done { task_count } => {
            let _ = task_count;
            let _ = review_issue_count;
            None
        }
        _ => progress_history_view_for_input(event, input)
            .map(|view| (view.icon, progress_tone_color(view.tone), view.line)),
    }
}

fn progress_history_view_for_input(
    event: &RunProgress,
    input: &InputState,
) -> Option<ProgressHistoryView> {
    if let RunProgress::Planned { sub_task_count } = event {
        match input.run_route.as_deref() {
            Some("single-worker") => {
                return Some(ProgressHistoryView {
                    icon: "✓",
                    tone: ProgressTone::Success,
                    line: format!("worker  ready · {sub_task_count} run(s) · single-worker"),
                });
            }
            Some("direct-answer") => {
                return Some(ProgressHistoryView {
                    icon: "✓",
                    tone: ProgressTone::Success,
                    line: "worker  direct answer ready".into(),
                });
            }
            _ => {}
        }
    }

    progress_history_view(event)
}

fn progress_tone_color(tone: ProgressTone) -> &'static str {
    match tone {
        ProgressTone::Success => GREEN,
        ProgressTone::Running => YELLOW,
        ProgressTone::Error => RED,
    }
}

pub(super) fn remember_visible_progress(input: &mut InputState, event: &RunProgress, line: &str) {
    let key = visible_progress_key_for_event(event, line);
    match event {
        RunProgress::WorkerOutput { .. } => {
            input.run_visible_output_keys.push(key);
            if input.run_visible_output_keys.len() > 120 {
                input.run_visible_output_keys.remove(0);
            }
        }
        _ => {
            input.run_visible_status_keys.push(key);
            if input.run_visible_status_keys.len() > 120 {
                input.run_visible_status_keys.remove(0);
            }
        }
    }
}

fn render_progress_line(icon: &str, color: &str, line: &str) -> Vec<String> {
    let mut lines = line.lines();
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if color == DIM {
        out.push(format!("{DIM}{icon} {first}{RESET}"));
        for line in lines {
            out.push(format!("{DIM}  │ {line}{RESET}"));
        }
    } else {
        out.push(format!("{color}{icon}{RESET} {first}"));
        for line in lines {
            out.push(format!("  {DIM}│{RESET} {line}"));
        }
    }
    out
}

fn progress_was_already_visible(input: &InputState, event: &RunProgress, line: &str) -> bool {
    let key = visible_progress_key_for_event(event, line);
    match event {
        RunProgress::WorkerOutput { .. } => input
            .run_visible_output_keys
            .iter()
            .any(|seen| seen == &key),
        _ => input
            .run_visible_status_keys
            .iter()
            .any(|seen| seen == &key),
    }
}

fn visible_progress_key_for_event(event: &RunProgress, line: &str) -> String {
    match event {
        RunProgress::WorkerOutput {
            task_id,
            agent,
            role,
            content,
        } => visible_output_key(
            &format!("worker:{}:{}:{}", short_task_id(task_id), role, agent),
            &visible_worker_output_dedupe_display(content)
                .unwrap_or_else(|| truncate_chars(line.trim(), 240)),
        ),
        RunProgress::RoleOutput { role, content } => visible_output_key(
            &format!("role:{role}"),
            &visible_role_output_display(role, content, PROGRESS_OUTPUT_EXPANDED_LIMIT)
                .unwrap_or_else(|| truncate_chars(line.trim(), 240)),
        ),
        _ => truncate_chars(line.trim(), 240),
    }
}

fn visible_worker_output_line(
    task_id: &uuid::Uuid,
    role: &str,
    agent: &str,
    content: &str,
    input: &InputState,
) -> Option<(&'static str, &'static str, String)> {
    let max = if input.history_folded {
        PROGRESS_OUTPUT_FOLDED_LIMIT
    } else {
        PROGRESS_OUTPUT_EXPANDED_LIMIT
    };
    let output = visible_worker_output_display(content, max)?;
    let scope = format!("worker:{}:{}:{}", short_task_id(task_id), role, agent);
    let dedupe_display =
        visible_worker_output_dedupe_display(content).unwrap_or_else(|| output.display.clone());
    if output_was_already_visible(input, &scope, &dedupe_display) {
        return None;
    }
    let source = worker_output_title(role, agent, output.kind);
    let (icon, color) = worker_output_style(output.kind);
    Some((
        icon,
        color,
        format_agent_output_block(
            &source,
            Some(&short_task_id(task_id)),
            &output.display,
            input.history_folded,
        ),
    ))
}

fn worker_output_style(kind: agent_events::VisibleAgentOutputKind) -> (&'static str, &'static str) {
    match kind {
        agent_events::VisibleAgentOutputKind::Question => ("?", TIFFANY),
        agent_events::VisibleAgentOutputKind::ToolCall => ("↳", CYAN),
        agent_events::VisibleAgentOutputKind::ToolResult => ("✓", DIM),
        agent_events::VisibleAgentOutputKind::Diff => ("±", TIFFANY),
        agent_events::VisibleAgentOutputKind::Patch => ("±", TIFFANY),
        agent_events::VisibleAgentOutputKind::FileUpdate => ("✓", TIFFANY),
        agent_events::VisibleAgentOutputKind::Stderr => ("✗", RED),
        agent_events::VisibleAgentOutputKind::Actionable => ("⚠", YELLOW),
        agent_events::VisibleAgentOutputKind::Final => ("✓", CYAN),
        agent_events::VisibleAgentOutputKind::Normal => ("↳", DIM),
    }
}

fn worker_output_title(
    role: &str,
    agent: &str,
    kind: agent_events::VisibleAgentOutputKind,
) -> String {
    let role = role.trim();
    let agent = agent.trim();
    let mut title = match (role.is_empty(), agent.is_empty(), role == agent) {
        (false, false, false) => format!("{role} · {agent}"),
        (false, _, _) => role.to_string(),
        (_, false, _) => agent.to_string(),
        _ => "worker".to_string(),
    };
    if !matches!(kind, agent_events::VisibleAgentOutputKind::Normal) {
        title.push_str(" · ");
        title.push_str(kind.label());
    }
    title
}

fn visible_role_output_line(
    role: &str,
    content: &str,
    input: &InputState,
) -> Option<(&'static str, &'static str, String)> {
    let max = if input.history_folded {
        PROGRESS_OUTPUT_FOLDED_LIMIT
    } else {
        PROGRESS_OUTPUT_EXPANDED_LIMIT
    };
    let display = visible_role_output_display(role, content, max)?;
    let dedupe_display = visible_role_output_display(role, content, PROGRESS_OUTPUT_EXPANDED_LIMIT)
        .unwrap_or_else(|| display.clone());
    if output_was_already_visible(input, &format!("role:{role}"), &dedupe_display) {
        return None;
    }
    Some((
        "·",
        DIM,
        format_agent_output_block(role_display_name(role), None, &display, true),
    ))
}

fn format_agent_output_block(
    agent: &str,
    task_id: Option<&str>,
    display: &str,
    folded: bool,
) -> String {
    let title = if let Some(task_id) = task_id {
        format!("{} · {}", agent, task_id)
    } else {
        agent.to_string()
    };
    if folded {
        return format!(
            "{} — {}",
            title,
            one_line_process_summary(display, PROGRESS_OUTPUT_FOLDED_LIMIT)
        );
    }

    let mut out = format!("{} output", title);
    let mut pushed = 0usize;
    for source_line in display
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        for line in super::codex_bottom_pane::wrap_text_lines(source_line, PROGRESS_OUTPUT_WIDTH) {
            out.push('\n');
            out.push_str(&line);
            pushed += 1;
            if pushed >= PROGRESS_OUTPUT_LINE_LIMIT {
                out.push('\n');
                out.push_str("… /process 200");
                return out;
            }
        }
    }
    if pushed == 0 {
        out.push_str("\noutput received");
    }
    out
}

struct VisibleWorkerOutputDisplay {
    kind: agent_events::VisibleAgentOutputKind,
    display: String,
}

fn visible_worker_output_display(content: &str, max: usize) -> Option<VisibleWorkerOutputDisplay> {
    if let Some(final_output) = agent_events::visible_agent_output(content, FINAL_OUTPUT_MAX_CHARS)
        .filter(|output| output.kind == agent_events::VisibleAgentOutputKind::Final)
    {
        return Some(VisibleWorkerOutputDisplay {
            kind: final_output.kind,
            display: format_captured_final_result(&final_output.display),
        });
    }

    let output = agent_events::visible_agent_output(content, max)?;
    if worker_output_is_final_like_display(&output.display) {
        return Some(VisibleWorkerOutputDisplay {
            kind: agent_events::VisibleAgentOutputKind::Final,
            display: format_captured_final_result(&output.display),
        });
    }
    Some(VisibleWorkerOutputDisplay {
        kind: output.kind,
        display: output.display,
    })
}

fn visible_worker_output_dedupe_display(content: &str) -> Option<String> {
    agent_events::visible_agent_output(content, PROGRESS_OUTPUT_EXPANDED_LIMIT)
        .map(|output| output.dedupe_key)
}

fn visible_role_output_display(role: &str, content: &str, max: usize) -> Option<String> {
    if agent_events::is_redundant_role_output(role, content, max) {
        return None;
    }
    let output = agent_events::visible_agent_output(content, max)?;
    if role_is_pipeline_controller(role)
        && output.kind != agent_events::VisibleAgentOutputKind::Actionable
    {
        return None;
    }
    Some(output.display)
}

fn format_captured_final_result(display: &str) -> String {
    let normalized = normalize_execution_output_summary(display);
    let normalized = normalized.trim();
    let char_count = normalized.chars().count();
    format!(
        "final response captured ({} chars); /result prints selectable plain text",
        char_count
    )
}

fn one_line_process_summary(display: &str, max: usize) -> String {
    let summary = display
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ");
    truncate_chars(&summary, max)
}

fn worker_output_is_final_like_display(display: &str) -> bool {
    let display = agent_events::normalize_output_summary(display);
    if display.lines().count() >= 2 {
        return true;
    }
    display.chars().count() > 180
}

fn role_display_name(role: &str) -> &'static str {
    match role {
        "planner" => "planner",
        "critic" => "critic",
        "reviewer" => "reviewer",
        _ => "role",
    }
}

fn role_is_pipeline_controller(role: &str) -> bool {
    matches!(role, "planner" | "critic" | "reviewer")
}

fn output_was_already_visible(input: &InputState, scope: &str, display: &str) -> bool {
    let key = visible_output_key(scope, display);
    input
        .run_visible_output_keys
        .iter()
        .any(|seen| seen == &key)
}

fn visible_output_key(scope: &str, display: &str) -> String {
    format!("{}:{}", scope, truncate_chars(display.trim(), 220))
}

fn short_task_id(task_id: &uuid::Uuid) -> String {
    task_id.to_string()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_low_value_worker_output() {
        let task_id = uuid::Uuid::new_v4();

        assert!(progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude system".into(),
            },
            0,
            &InputState::default()
        )
        .is_none());
        assert!(progress_line(
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: "claude assistant: thinking".into(),
            },
            0,
            &InputState::default()
        )
        .is_none());

        assert!(progress_line(
            &RunProgress::Done { task_count: 2 },
            0,
            &InputState::default(),
        )
        .is_none());
    }

    #[test]
    fn shows_useful_execution_summaries_once() {
        let task_id = uuid::Uuid::nil();
        let mut input = InputState {
            history_folded: true,
            ..InputState::default()
        };
        let first = RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: "claude assistant: useful summary".into(),
        };

        let line = progress_line(&first, 0, &input).expect("visible worker output");
        assert_eq!(line.0, "↳");
        assert!(line.2.contains("worker-cc"));
        assert!(line.2.contains("claude-code"));
        assert!(line.2.contains("useful summary"));

        remember_visible_progress(&mut input, &first, &line.2);
        let duplicate = RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: "claude result: useful summary".into(),
        };
        assert!(progress_line(&duplicate, 0, &input).is_none());

        assert!(progress_line(
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: r#"{"sub_tasks":[{"prompt":"do one thing"}]}"#.into(),
            },
            0,
            &input,
        )
        .is_none());
        assert!(progress_line(
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: "plain planning explanation that belongs in /process".into(),
            },
            0,
            &input,
        )
        .is_none());

        let planner_error = progress_line(
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: "replan failed; continuing with previous plan".into(),
            },
            0,
            &input,
        )
        .expect("actionable planner status");
        assert!(planner_error.2.contains("replan failed"));

        let role_line = progress_line(
            &RunProgress::RoleOutput {
                role: "critic".into(),
                content: r#"{"approved":false,"issues":["missing test"]}"#.into(),
            },
            0,
            &input,
        )
        .expect("visible critic issue output");
        assert_eq!(role_line.0, "·");
        assert!(role_line.2.contains("critic"));
        assert!(role_line.2.contains("needs changes"));
        assert!(role_line.2.contains("missing test"));
        assert!(!role_line.2.contains('{'));

        assert!(progress_line(
            &RunProgress::RoleOutput {
                role: "reviewer".into(),
                content: r#"{"approved":true,"issues":[]}"#.into(),
            },
            0,
            &input,
        )
        .is_none());
    }

    #[test]
    fn marks_final_worker_output_without_duplication() {
        let task_id = uuid::Uuid::nil();

        let line = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content:
                    r#"claude-code result: {"result":"Implemented it in several paragraphs."}"#
                        .into(),
            },
            0,
            &InputState::default(),
        )
        .expect("final capture marker");
        assert_eq!(line.0, "✓");
        assert_eq!(line.1, CYAN);
        assert!(line.2.contains("final response captured"));
        assert!(line.2.contains("/result"));
        assert!(!line.2.contains("Implemented it in several paragraphs."));
        assert!(!line.2.contains('{'));
    }

    #[test]
    fn shows_codex_worker_actionable_stderr() {
        let task_id = uuid::Uuid::nil();
        let line = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "codex".into(),
                role: "worker-codex".into(),
                content: "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]".into(),
            },
            0,
            &InputState::default(),
        )
        .expect("codex stderr should be visible");

        assert_eq!(line.0, "✗");
        assert_eq!(line.1, RED);
        assert!(line.2.contains("codex"));
        assert!(line.2.contains("stderr"));
        assert!(line.2.contains("模型不存在"));
    }

    #[test]
    fn labels_worker_tool_calls_in_history() {
        let task_id = uuid::Uuid::nil();
        let line = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "worker-codex".into(),
                role: "worker-codex".into(),
                content: "codex local_shell_call: tool shell: cargo test --all".into(),
            },
            0,
            &InputState::default(),
        )
        .expect("tool call should be visible");

        assert_eq!(line.0, "↳");
        assert_eq!(line.1, CYAN);
        assert!(line.2.contains("worker-codex · tool call"));
        assert!(line.2.contains("tool shell: cargo test --all"));
    }

    #[test]
    fn labels_worker_tool_results_and_alerts_in_history() {
        let task_id = uuid::Uuid::nil();

        let result = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude-code tool_result: tool result: tests passed".into(),
            },
            0,
            &InputState::default(),
        )
        .expect("tool result should be visible");

        assert_eq!(result.0, "✓");
        assert_eq!(result.1, DIM);
        assert!(result.2.contains("worker-cc · claude-code · tool result"));
        assert!(result.2.contains("tests passed"));

        let alert = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude-code assistant: permission denied while writing file".into(),
            },
            0,
            &InputState::default(),
        )
        .expect("alert should be visible");

        assert_eq!(alert.0, "⚠");
        assert_eq!(alert.1, YELLOW);
        assert!(alert.2.contains("alert"));
        assert!(alert.2.contains("permission denied"));
    }

    #[test]
    fn labels_worker_questions_without_error_tone() {
        let task_id = uuid::Uuid::nil();

        let question = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude-code tool_use: tool AskUserQuestion".into(),
            },
            0,
            &InputState::default(),
        )
        .expect("question request should be visible");

        assert_eq!(question.0, "?");
        assert_eq!(question.1, TIFFANY);
        assert!(question.2.contains("worker-cc · claude-code · question"));
        assert!(question.2.contains("question requested"));

        let waiting = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude-code tool_result: tool error: Answer questions?".into(),
            },
            0,
            &InputState::default(),
        )
        .expect("question waiting state should be visible");

        assert_eq!(waiting.0, "?");
        assert_eq!(waiting.1, TIFFANY);
        assert!(waiting.2.contains("waiting for user input"));
        assert!(!waiting.2.contains("tool error"));
    }

    #[test]
    fn shared_visible_output_classifier_keeps_role_errors_only() {
        let input = InputState::default();

        assert!(progress_line(
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: r#"{"sub_tasks":[{"prompt":"say hello"}]}"#.into(),
            },
            0,
            &input,
        )
        .is_none());

        let critic = progress_line(
            &RunProgress::RoleOutput {
                role: "critic".into(),
                content: r#"critic>
{"approved":false,"issues":["模型不存在，需要修正 worker-codex 的 Model Name"]}"#
                    .into(),
            },
            0,
            &input,
        )
        .expect("critic issue should be visible");

        assert!(critic.2.contains("needs changes"));
        assert!(critic.2.contains("模型不存在"));
        assert!(!critic.2.contains("\"approved\""));
    }

    #[test]
    fn lifecycle_lines_match_tiffany_waterfall_style() {
        let task_id = uuid::Uuid::nil();
        let input = InputState::default();

        let started = progress_line(
            &RunProgress::WorkerStarted {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                runtime: "claude-code".into(),
                cc_agent: None,
                model: "MiniMax-M3".into(),
                provider: Some("minimax".into()),
                prompt: "do the task".into(),
            },
            0,
            &input,
        )
        .expect("worker start line");
        assert_eq!(
            started.2,
            "worker  worker-cc started · claude-code · minimax/MiniMax-M3 · 00000000"
        );

        let done = progress_line(
            &RunProgress::WorkerDone {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                duration_ms: 1_250,
                ok: true,
            },
            0,
            &input,
        )
        .expect("worker done line");
        assert_eq!(done.2, "worker  worker-cc done · 00000000 · 1.2s");

        let failed = progress_line(
            &RunProgress::WorkerDone {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                duration_ms: 42,
                ok: false,
            },
            0,
            &input,
        )
        .expect("worker failed line");
        assert_eq!(failed.2, "worker  worker-cc failed · 00000000 · 42ms");

        let reviewing =
            progress_line(&RunProgress::Reviewing { task_id }, 0, &input).expect("reviewing line");
        assert_eq!(reviewing.2, "review  checking worker output · 00000000");

        let passed = progress_line(
            &RunProgress::ReviewResult {
                task_id,
                approved: true,
                issues: 0,
            },
            0,
            &input,
        )
        .expect("review passed line");
        assert_eq!(passed.2, "review  passed · 00000000");

        let needs_fixes = progress_line(
            &RunProgress::ReviewResult {
                task_id,
                approved: false,
                issues: 3,
            },
            0,
            &input,
        )
        .expect("review needs fixes line");
        assert_eq!(needs_fixes.2, "review  needs fixes · 00000000 · 3 issue(s)");
    }

    #[test]
    fn planned_history_line_respects_effective_route() {
        let mut input = InputState {
            run_route: Some("single-worker".into()),
            ..InputState::default()
        };

        let single = progress_line(&RunProgress::Planned { sub_task_count: 1 }, 0, &input)
            .expect("single worker planned line");
        assert_eq!(single.2, "worker  ready · 1 run(s) · single-worker");
        assert!(!single.2.contains("plan ready"));

        input.run_route = Some("direct-answer".into());
        let direct = progress_line(&RunProgress::Planned { sub_task_count: 1 }, 0, &input)
            .expect("direct planned line");
        assert_eq!(direct.2, "worker  direct answer ready");

        input.run_route = Some("full-pipeline".into());
        let full = progress_line(&RunProgress::Planned { sub_task_count: 2 }, 0, &input)
            .expect("full pipeline planned line");
        assert_eq!(full.2, "plan ready — 2 worker run(s)");
    }

    #[test]
    fn renders_colored_history_lines() {
        let mut input = InputState::default();
        let lines = progress_history_lines(&RunProgress::Planning, &mut input);

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("planning"));
    }
}
