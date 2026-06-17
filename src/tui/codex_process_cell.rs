// Codex-style process cell rendering for tiffany-loop lightweight terminal runner.
//
// The upstream Codex implementation keeps command execution display in
// `codex-rs/tui/src/exec_cell/`. This module keeps the same ownership boundary
// for orchestrator progress: execution events become stable history lines here,
// while the terminal loop only inserts those lines into scrollback.

use super::state::InputState;
use super::util::{normalize_execution_output_summary, summarize_execution_output, truncate_chars};
use crate::agent_events;
use crate::pipeline::orchestrator::RunProgress;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const TIFFANY: &str = "\x1b[38;2;129;216;208m";
const GREEN: &str = TIFFANY;
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
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
        RunProgress::Planning => Some(("●", YELLOW, "planning".into())),
        RunProgress::Planned { sub_task_count } => Some((
            "✓",
            GREEN,
            format!("plan ready — {} sub-task(s)", sub_task_count),
        )),
        RunProgress::Critiquing { round } => {
            Some(("●", YELLOW, format!("checking plan — round {}", round)))
        }
        RunProgress::CritiqueResult { approved, issues } => {
            if *approved {
                Some(("✓", GREEN, "plan approved".into()))
            } else {
                Some((
                    "●",
                    YELLOW,
                    format!("plan needs changes — {} issue(s)", issues),
                ))
            }
        }
        RunProgress::Replanning { attempt } => {
            Some(("●", YELLOW, format!("replanning — attempt {}", attempt)))
        }
        RunProgress::Executing { sub_task_count } => Some((
            "●",
            YELLOW,
            format!("running {} sub-task(s)", sub_task_count),
        )),
        RunProgress::WorkerStarted { task_id, agent } => Some((
            "●",
            YELLOW,
            format!("{} · {} started", agent, short_task_id(task_id)),
        )),
        RunProgress::WorkerOutput {
            task_id,
            agent,
            content,
        } => visible_worker_output_line(task_id, agent, content, input),
        RunProgress::RoleOutput { role, content } => visible_role_output_line(role, content, input),
        RunProgress::WorkerDone { task_id, agent, ok } => {
            if *ok {
                Some((
                    "✓",
                    GREEN,
                    format!("{} · {} done", agent, short_task_id(task_id)),
                ))
            } else {
                Some((
                    "✗",
                    RED,
                    format!("{} · {} failed", agent, short_task_id(task_id)),
                ))
            }
        }
        RunProgress::Reviewing { task_id } => Some((
            "●",
            YELLOW,
            format!("reviewing — {}", short_task_id(task_id)),
        )),
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            if *approved {
                Some((
                    "✓",
                    GREEN,
                    format!("review approved — {}", short_task_id(task_id)),
                ))
            } else {
                Some((
                    "●",
                    YELLOW,
                    format!(
                        "review needs fixes — {} issue(s), {}",
                        issues,
                        short_task_id(task_id)
                    ),
                ))
            }
        }
        RunProgress::Done { task_count } => {
            let _ = task_count;
            let _ = review_issue_count;
            None
        }
        RunProgress::Failed(msg) => Some(("✗", RED, format!("error — {}", msg))),
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
            content,
        } => visible_output_key(
            &format!("worker:{}:{}", short_task_id(task_id), agent),
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
    agent: &str,
    content: &str,
    input: &InputState,
) -> Option<(&'static str, &'static str, String)> {
    let max = if input.history_folded {
        PROGRESS_OUTPUT_FOLDED_LIMIT
    } else {
        PROGRESS_OUTPUT_EXPANDED_LIMIT
    };
    let display = visible_worker_output_display(content, max)?;
    let role = format!("worker:{}:{}", short_task_id(task_id), agent);
    let dedupe_display =
        visible_worker_output_dedupe_display(content).unwrap_or_else(|| display.clone());
    if output_was_already_visible(input, &role, &dedupe_display) {
        return None;
    }
    Some((
        "↳",
        DIM,
        format_agent_output_block(
            agent,
            Some(&short_task_id(task_id)),
            &display,
            input.history_folded,
        ),
    ))
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

fn visible_worker_output_display(content: &str, max: usize) -> Option<String> {
    if worker_output_is_noise(content) {
        return None;
    }

    if let Some(final_output) =
        agent_events::final_output_candidate(content, FINAL_OUTPUT_MAX_CHARS)
    {
        return Some(format_captured_final_result(&final_output));
    }

    let display = normalize_visible_output(content, max)?;
    if worker_output_is_final_like_display(&display) {
        return Some(format_captured_final_result(&display));
    }
    Some(display)
}

fn visible_worker_output_dedupe_display(content: &str) -> Option<String> {
    if worker_output_is_noise(content) {
        return None;
    }

    if let Some(final_output) =
        agent_events::final_output_candidate(content, FINAL_OUTPUT_MAX_CHARS)
    {
        return normalize_visible_output(&final_output, PROGRESS_OUTPUT_EXPANDED_LIMIT);
    }

    normalize_visible_output(content, PROGRESS_OUTPUT_EXPANDED_LIMIT)
}

fn visible_role_output_display(role: &str, content: &str, max: usize) -> Option<String> {
    if agent_events::is_redundant_role_output(role, content, max) {
        return None;
    }
    if worker_output_is_noise(content) {
        return None;
    }
    let display = normalize_visible_output(content, max)?;
    if role_is_pipeline_controller(role) && !role_output_is_actionable(&display) {
        return None;
    }
    Some(display)
}

fn normalize_visible_output(content: &str, max: usize) -> Option<String> {
    let display = summarize_execution_output(content, max)?;
    let display = normalize_execution_output_summary(&display);
    (!display.trim().is_empty()).then_some(display)
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

fn worker_output_is_noise(content: &str) -> bool {
    let humanized = agent_events::humanize_jsonish(content, 120);
    let raw_lower = humanized.trim().to_ascii_lowercase();
    if raw_lower.contains(" system: system")
        || raw_lower.contains(" assistant: thinking")
        || raw_lower.ends_with(" system")
        || raw_lower.ends_with(" thinking")
    {
        return true;
    }
    let normalized = agent_events::normalize_output_summary(&humanized);
    let lower = normalized.trim().to_ascii_lowercase();
    lower.is_empty()
        || matches!(lower.as_str(), "system" | "thinking")
        || lower.ends_with(" system")
        || lower.ends_with(" thinking")
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

fn role_output_is_actionable(display: &str) -> bool {
    let lower = display.to_ascii_lowercase();
    [
        "error",
        "failed",
        "failure",
        "rejected",
        "needs changes",
        "permission",
        "denied",
        "authentication",
        "api key",
        "model not found",
        "invalid model",
        "模型不存在",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
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
            content: "claude assistant: useful summary".into(),
        };

        let line = progress_line(&first, 0, &input).expect("visible worker output");
        assert_eq!(line.0, "↳");
        assert!(line.2.contains("claude-code"));
        assert!(line.2.contains("useful summary"));

        remember_visible_progress(&mut input, &first, &line.2);
        let duplicate = RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
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
                content:
                    r#"claude-code result: {"result":"Implemented it in several paragraphs."}"#
                        .into(),
            },
            0,
            &InputState::default(),
        )
        .expect("final capture marker");
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
                content: "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]".into(),
            },
            0,
            &InputState::default(),
        )
        .expect("codex stderr should be visible");

        assert_eq!(line.0, "↳");
        assert!(line.2.contains("codex"));
        assert!(line.2.contains("模型不存在"));
    }

    #[test]
    fn renders_colored_history_lines() {
        let mut input = InputState::default();
        let lines = progress_history_lines(&RunProgress::Planning, &mut input);

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("planning"));
    }
}
