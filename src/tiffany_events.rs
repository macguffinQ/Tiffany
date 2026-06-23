use crate::agent_events;
use crate::pipeline::orchestrator::RunProgress;
use serde::Serialize;
use std::collections::HashSet;

const TEXT_OUTPUT_SUMMARY_MAX_CHARS: usize = 360;
const COMPACT_OUTPUT_SUMMARY_MAX_CHARS: usize = 180;
const FULL_MESSAGE_STREAM_MAX_CHARS: usize = usize::MAX;

#[derive(Debug, Serialize)]
pub struct TiffanyProgressEvent {
    pub role: &'static str,
    pub status: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_reason_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow_steps: Option<String>,
}

impl From<RunProgress> for TiffanyProgressEvent {
    fn from(event: RunProgress) -> Self {
        match event {
            RunProgress::RouteSelected { route, reason } => route_selected_event(route, reason),
            RunProgress::Planning => progress_event("planner", "running", "planning"),
            RunProgress::Planned { sub_task_count } => TiffanyProgressEvent {
                count: Some(sub_task_count),
                ..progress_event(
                    "planner",
                    "done",
                    format!("plan ready - {sub_task_count} worker run(s)"),
                )
            },
            RunProgress::Critiquing { round } => progress_event(
                "critic",
                "running",
                format!("checking plan - round {round}"),
            ),
            RunProgress::CritiqueResult { approved, issues } => TiffanyProgressEvent {
                status: if approved { "done" } else { "warning" },
                approved: Some(approved),
                issues: Some(issues),
                ..progress_event(
                    "critic",
                    "done",
                    if approved {
                        "plan approved".to_string()
                    } else {
                        format!("plan needs fixes - {issues} issue(s)")
                    },
                )
            },
            RunProgress::Replanning { attempt } => progress_event(
                "planner",
                "running",
                format!("replanning - attempt {attempt}"),
            ),
            RunProgress::ControlFallback {
                role,
                message,
                reason,
            } => {
                let role = progress_role_label(&role);
                TiffanyProgressEvent {
                    reason: Some(reason),
                    ..progress_event(role, "warning", message)
                }
            }
            RunProgress::DirectAnswer => progress_event("worker", "running", "answering directly"),
            RunProgress::Executing { sub_task_count } => TiffanyProgressEvent {
                count: Some(sub_task_count),
                ..progress_event(
                    "worker",
                    "running",
                    format!("running {sub_task_count} worker run(s)"),
                )
            },
            RunProgress::WorkerStarted {
                task_id,
                agent,
                role,
                runtime,
                cc_agent,
                model,
                provider,
                prompt,
            } => TiffanyProgressEvent {
                task_id: Some(task_id.to_string()),
                agent: Some(agent),
                worker_role: Some(role.clone()),
                runtime: Some(runtime),
                cc_agent,
                model: Some(model),
                provider,
                task_prompt: Some(prompt),
                ..progress_event("worker", "running", format!("{role} started"))
            },
            RunProgress::WorkerOutput {
                task_id,
                agent,
                role,
                content,
            } => TiffanyProgressEvent {
                task_id: Some(task_id.to_string()),
                agent: Some(agent),
                worker_role: Some(role.clone()),
                content: Some(content),
                ..progress_event("worker", "output", format!("{role} output"))
            },
            RunProgress::RoleOutput { role, content } => {
                let role = match role.as_str() {
                    "planner" => "planner",
                    "critic" => "critic",
                    "reviewer" => "reviewer",
                    "worker" => "worker",
                    _ => "orchestrator",
                };
                TiffanyProgressEvent {
                    content: Some(content),
                    ..progress_event(role, "output", format!("{role} output"))
                }
            }
            RunProgress::WorkerDone {
                task_id,
                agent,
                role,
                duration_ms,
                ok,
            } => TiffanyProgressEvent {
                status: if ok { "done" } else { "failed" },
                task_id: Some(task_id.to_string()),
                agent: Some(agent),
                worker_role: Some(role.clone()),
                duration_ms: Some(duration_ms),
                ..progress_event(
                    "worker",
                    "done",
                    if ok {
                        format!("{role} done")
                    } else {
                        format!("{role} failed")
                    },
                )
            },
            RunProgress::Reviewing { task_id } => TiffanyProgressEvent {
                task_id: Some(task_id.to_string()),
                ..progress_event("reviewer", "running", "reviewing worker output")
            },
            RunProgress::ReviewSkipped { task_id, reason } => TiffanyProgressEvent {
                task_id: Some(task_id.to_string()),
                reason: Some(reason.clone()),
                ..progress_event("reviewer", "skipped", format!("review skipped - {reason}"))
            },
            RunProgress::ReviewResult {
                task_id,
                approved,
                issues,
            } => TiffanyProgressEvent {
                status: if approved { "done" } else { "warning" },
                task_id: Some(task_id.to_string()),
                approved: Some(approved),
                issues: Some(issues),
                ..progress_event(
                    "reviewer",
                    "done",
                    if approved {
                        "review approved".to_string()
                    } else {
                        format!("review needs fixes - {issues} issue(s)")
                    },
                )
            },
            RunProgress::ReviewUnavailable { task_id, message } => TiffanyProgressEvent {
                task_id: Some(task_id.to_string()),
                reason: Some(message.clone()),
                ..progress_event(
                    "reviewer",
                    "warning",
                    format!("review unavailable - {message}"),
                )
            },
            RunProgress::Done { task_count } => TiffanyProgressEvent {
                count: Some(task_count),
                ..progress_event(
                    "orchestrator",
                    "done",
                    format!("done - {task_count} worker run(s)"),
                )
            },
            RunProgress::Failed(message) => progress_event("orchestrator", "failed", message),
        }
    }
}

fn progress_event(
    role: &'static str,
    status: &'static str,
    message: impl Into<String>,
) -> TiffanyProgressEvent {
    TiffanyProgressEvent {
        role,
        status,
        message: message.into(),
        task_id: None,
        agent: None,
        worker_role: None,
        runtime: None,
        cc_agent: None,
        model: None,
        provider: None,
        task_prompt: None,
        content: None,
        approved: None,
        issues: None,
        count: None,
        duration_ms: None,
        reason: None,
        route: None,
        route_label: None,
        route_reason_label: None,
        flow_steps: None,
    }
}

fn route_selected_event(route: String, reason: String) -> TiffanyProgressEvent {
    let metadata = route_metadata(&route, &reason);
    let mut event = progress_event(
        "orchestrator",
        "running",
        format!("route selected - {route}"),
    );
    event.route = Some(route);
    event.reason = Some(reason);
    if let Some(metadata) = metadata {
        event.route_label = Some(metadata.display_label().to_string());
        event.route_reason_label = Some(metadata.short_reason_label().to_string());
        event.flow_steps = Some(metadata.flow_steps().to_string());
    }
    event
}

fn route_metadata(route: &str, reason: &str) -> Option<agent_events::OrchestrationRoute> {
    agent_events::OrchestrationRoute::from_label_or_reason(route, reason)
}

#[derive(Default)]
pub struct TiffanyTextProgressFormatter {
    seen_output_keys: HashSet<String>,
}

impl TiffanyTextProgressFormatter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn format(&mut self, event: &RunProgress) -> Option<String> {
        if let Some(key) = self.output_dedupe_key(event) {
            if !self.seen_output_keys.insert(key) {
                return None;
            }
        }

        format_text_progress_event(event)
    }

    fn output_dedupe_key(&self, event: &RunProgress) -> Option<String> {
        match event {
            RunProgress::WorkerOutput {
                task_id,
                agent,
                role,
                content,
            } => {
                let output =
                    visible_non_final_agent_output(content, TEXT_OUTPUT_SUMMARY_MAX_CHARS)?;
                Some(format!(
                    "worker:{}:{agent}:{role}:{}",
                    &task_id.to_string()[..8],
                    output.dedupe_key
                ))
            }
            RunProgress::RoleOutput { role, content } => {
                if agent_events::is_redundant_role_output(
                    role,
                    content,
                    TEXT_OUTPUT_SUMMARY_MAX_CHARS,
                ) {
                    return None;
                }
                let output =
                    visible_non_final_agent_output(content, TEXT_OUTPUT_SUMMARY_MAX_CHARS)?;
                Some(format!("role:{role}:{}", output.dedupe_key))
            }
            _ => None,
        }
    }
}

pub fn format_text_progress_event(event: &RunProgress) -> Option<String> {
    match event {
        RunProgress::RouteSelected { route, reason } => {
            Some(format!("● route    {route} · {reason}"))
        }
        RunProgress::Planning => Some("● planner  planning".into()),
        RunProgress::Planned { sub_task_count } => Some(format!(
            "✓ planner  plan ready · {sub_task_count} worker run(s)"
        )),
        RunProgress::Critiquing { round } => {
            Some(format!("● critic   checking plan · round {round}"))
        }
        RunProgress::CritiqueResult { approved, issues } => {
            if *approved {
                Some("✓ critic   plan approved".into())
            } else {
                Some(format!("⚠ critic   plan needs changes · {issues} issue(s)"))
            }
        }
        RunProgress::Replanning { attempt } => {
            Some(format!("● planner  replanning · attempt {attempt}"))
        }
        RunProgress::ControlFallback {
            role,
            message,
            reason,
        } => Some(format!(
            "⚠ {:<8}{} · {}",
            role_label(role),
            message,
            agent_events::humanize_jsonish(reason, TEXT_OUTPUT_SUMMARY_MAX_CHARS)
        )),
        RunProgress::DirectAnswer => Some("● worker   answering directly".into()),
        RunProgress::Executing { sub_task_count } => {
            Some(format!("● worker   running {sub_task_count} worker run(s)"))
        }
        RunProgress::WorkerStarted {
            task_id,
            role,
            runtime,
            cc_agent,
            model,
            provider,
            ..
        } => {
            let cc_agent = cc_agent
                .as_deref()
                .map(|agent| format!(" · agent {agent}"))
                .unwrap_or_default();
            Some(format!(
                "● worker   {role} started · {runtime}{cc_agent} · {} · {}",
                provider_model_label(provider.as_deref(), model),
                short_id(task_id)
            ))
        }
        RunProgress::WorkerOutput {
            task_id,
            agent,
            content,
            ..
        } => {
            let output = visible_non_final_agent_output(content, FULL_MESSAGE_STREAM_MAX_CHARS)?;
            Some(format!(
                "↳ worker   {} · {agent} · {}\n{}",
                short_id(task_id),
                output.kind.label(),
                indent_block(&output.display, "  ")
            ))
        }
        RunProgress::RoleOutput { role, content } => {
            if agent_events::is_redundant_role_output(role, content, TEXT_OUTPUT_SUMMARY_MAX_CHARS)
            {
                return None;
            }
            let output = visible_non_final_agent_output(content, TEXT_OUTPUT_SUMMARY_MAX_CHARS)?;
            Some(format!(
                "↳ {:<8}{}\n{}",
                role_label(role),
                "output",
                indent_block(&output.display, "  ")
            ))
        }
        RunProgress::WorkerDone {
            task_id,
            role,
            duration_ms,
            ok,
            ..
        } => Some(format!(
            "{} worker   {role} {} · {} · {}",
            if *ok { "✓" } else { "✗" },
            if *ok { "done" } else { "failed" },
            short_id(task_id),
            format_duration_ms(*duration_ms)
        )),
        RunProgress::Reviewing { task_id } => Some(format!(
            "● reviewer checking worker output · {}",
            short_id(task_id)
        )),
        RunProgress::ReviewSkipped { task_id, reason } => Some(format!(
            "✓ reviewer skipped · {} · {}",
            short_id(task_id),
            reason
        )),
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            if *approved {
                Some(format!("✓ reviewer passed · {}", short_id(task_id)))
            } else {
                Some(format!(
                    "⚠ reviewer needs fixes · {} · {issues} issue(s)",
                    short_id(task_id)
                ))
            }
        }
        RunProgress::ReviewUnavailable { task_id, message } => Some(format!(
            "⚠ reviewer unavailable · {} · {}",
            short_id(task_id),
            agent_events::humanize_jsonish(message, TEXT_OUTPUT_SUMMARY_MAX_CHARS)
        )),
        RunProgress::Done { task_count } => {
            Some(format!("✓ done     {task_count} worker run(s) completed"))
        }
        RunProgress::Failed(message) => Some(format!(
            "✗ error    {}",
            agent_events::humanize_jsonish(message, TEXT_OUTPUT_SUMMARY_MAX_CHARS)
        )),
    }
}

pub fn format_compact_progress_event(event: &RunProgress) -> Option<String> {
    match event {
        RunProgress::RouteSelected { route, reason } => Some(format!("route  {route} · {reason}")),
        RunProgress::Planning => Some("plan  planning work".into()),
        RunProgress::Planned { sub_task_count } => {
            Some(format!("plan  plan ready · {sub_task_count} worker run(s)"))
        }
        RunProgress::Critiquing { round } => Some(format!("critic  checking plan · round {round}")),
        RunProgress::CritiqueResult { approved, issues } => {
            if *approved {
                Some("critic  plan approved".into())
            } else {
                Some(format!("critic  plan needs changes · {issues} issue(s)"))
            }
        }
        RunProgress::Replanning { attempt } => {
            Some(format!("plan  updating plan · attempt {attempt}"))
        }
        RunProgress::ControlFallback {
            role,
            message,
            reason,
        } => Some(format!(
            "{}  {} · {}",
            compact_role_label(role),
            message,
            agent_events::humanize_jsonish(reason, COMPACT_OUTPUT_SUMMARY_MAX_CHARS)
        )),
        RunProgress::DirectAnswer => Some("run  answering directly".into()),
        RunProgress::Executing { sub_task_count } => {
            Some(format!("run  running {sub_task_count} worker run(s)"))
        }
        RunProgress::WorkerStarted {
            task_id,
            role,
            runtime,
            cc_agent,
            model,
            provider,
            ..
        } => {
            let cc_agent = cc_agent
                .as_deref()
                .map(|agent| format!(" · agent {agent}"))
                .unwrap_or_default();
            Some(format!(
                "worker  {role} started · {runtime}{cc_agent} · {} · {}",
                provider_model_label(provider.as_deref(), model),
                short_id(task_id)
            ))
        }
        RunProgress::WorkerOutput {
            task_id,
            agent,
            content,
            ..
        } => {
            let output = visible_non_final_agent_output(content, 160)?;
            let display = compact_output_summary(&output.display, 160)?;
            Some(format!(
                "worker  {} · {} {agent}: {display}",
                output.kind.label(),
                short_id(task_id),
            ))
        }
        RunProgress::RoleOutput { role, content } => {
            if agent_events::is_redundant_role_output(
                role,
                content,
                COMPACT_OUTPUT_SUMMARY_MAX_CHARS,
            ) {
                return None;
            }
            let output = visible_non_final_agent_output(content, COMPACT_OUTPUT_SUMMARY_MAX_CHARS)?;
            let display =
                compact_output_summary(&output.display, COMPACT_OUTPUT_SUMMARY_MAX_CHARS)?;
            Some(format!("{}  {display}", compact_role_label(role)))
        }
        RunProgress::WorkerDone {
            task_id,
            role,
            duration_ms,
            ok,
            ..
        } => Some(format!(
            "worker  {role} {} · {} · {}",
            if *ok { "done" } else { "failed" },
            short_id(task_id),
            format_duration_ms(*duration_ms),
        )),
        RunProgress::Reviewing { task_id } => Some(format!(
            "review  checking worker output · {}",
            short_id(task_id)
        )),
        RunProgress::ReviewSkipped { task_id, reason } => Some(format!(
            "review  skipped · {} · {}",
            short_id(task_id),
            reason
        )),
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            if *approved {
                Some(format!("review  passed · {}", short_id(task_id)))
            } else {
                Some(format!(
                    "review  needs fixes · {} · {issues} issue(s)",
                    short_id(task_id)
                ))
            }
        }
        RunProgress::ReviewUnavailable { task_id, message } => Some(format!(
            "review  unavailable · {} · {}",
            short_id(task_id),
            agent_events::humanize_jsonish(message, COMPACT_OUTPUT_SUMMARY_MAX_CHARS)
        )),
        RunProgress::Done { task_count } => {
            Some(format!("done  {task_count} worker run(s) completed"))
        }
        RunProgress::Failed(message) => Some(format!(
            "error  {}",
            agent_events::humanize_jsonish(message, COMPACT_OUTPUT_SUMMARY_MAX_CHARS)
        )),
    }
}

fn visible_non_final_agent_output(
    content: &str,
    max: usize,
) -> Option<agent_events::VisibleAgentOutput> {
    let output = agent_events::visible_agent_output(content, max)?;
    (output.kind != agent_events::VisibleAgentOutputKind::Final).then_some(output)
}

fn compact_output_summary(display: &str, max: usize) -> Option<String> {
    let summary = display
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join(" / ");
    (!summary.trim().is_empty()).then(|| agent_events::sanitize_text(&summary, max))
}

fn provider_model_label(provider: Option<&str>, model: &str) -> String {
    match provider.map(str::trim).filter(|value| !value.is_empty()) {
        Some(provider) => format!("{provider}/{model}"),
        None => model.to_string(),
    }
}

fn role_label(role: &str) -> &'static str {
    progress_role_label(role)
}

fn progress_role_label(role: &str) -> &'static str {
    match role {
        "planner" => "planner",
        "critic" => "critic",
        "reviewer" => "reviewer",
        "worker" => "worker",
        _ => "agent",
    }
}

fn compact_role_label(role: &str) -> &'static str {
    match role {
        "planner" => "plan",
        "critic" => "critic",
        "reviewer" => "review",
        _ => "role",
    }
}

fn short_id(id: &uuid::Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return format!("{duration_ms}ms");
    }
    let seconds = duration_ms / 1_000;
    let millis = duration_ms % 1_000;
    if seconds < 60 {
        if millis == 0 {
            return format!("{seconds}s");
        }
        return format!("{seconds}.{}s", millis / 100);
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}m{seconds:02}s")
}

fn indent_block(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn text_formatter_parses_role_json_without_raw_json() {
        let mut formatter = TiffanyTextProgressFormatter::new();
        let event = RunProgress::RoleOutput {
            role: "critic".into(),
            content: r#"critic>
{"approved":false,"issues":["raw JSON leaked"],"suggestions":["show parsed summaries"]}"#
                .into(),
        };

        let line = formatter.format(&event).expect("visible critic output");

        assert!(line.contains("needs changes: 1 issue(s)"));
        assert!(line.contains("raw JSON leaked"));
        assert!(!line.contains("\"approved\""));
        assert!(!line.contains("critic>"));
    }

    #[test]
    fn text_formatter_hides_duplicate_worker_output() {
        let task_id = Uuid::new_v4();
        let mut formatter = TiffanyTextProgressFormatter::new();
        let first = RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: "claude assistant: useful summary".into(),
        };
        let duplicate = RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: "claude result: useful summary".into(),
        };

        assert!(formatter.format(&first).is_some());
        assert!(formatter.format(&duplicate).is_none());
    }

    #[test]
    fn text_formatter_labels_worker_output_kind() {
        let task_id = Uuid::parse_str("12345678-0000-0000-0000-000000000000").unwrap();

        let line = format_text_progress_event(&RunProgress::WorkerOutput {
            task_id,
            agent: "worker-codex".into(),
            role: "worker-codex".into(),
            content: "codex local_shell_call: tool shell: cargo test --all".into(),
        })
        .expect("tool call line");

        assert!(line.contains("12345678 · worker-codex · tool call"));
        assert!(line.contains("tool shell: cargo test --all"));

        let compact = format_compact_progress_event(&RunProgress::WorkerOutput {
            task_id,
            agent: "worker-codex".into(),
            role: "worker-codex".into(),
            content: "codex local_shell_call: tool shell: cargo test --all".into(),
        })
        .expect("compact tool call line");

        assert!(compact.contains("worker  tool call · 12345678 worker-codex"));
        assert!(compact.contains("tool shell: cargo test --all"));
    }

    #[test]
    fn text_formatter_does_not_truncate_worker_message_stream() {
        let task_id = Uuid::parse_str("12345678-0000-0000-0000-000000000000").unwrap();
        let long_message = format!("{}END", "x".repeat(TEXT_OUTPUT_SUMMARY_MAX_CHARS + 128));

        let line = format_text_progress_event(&RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: format!("claude-code assistant: {long_message}"),
        })
        .expect("worker output line");

        assert!(line.contains(&long_message));
        assert!(!line.contains("…"));
    }

    #[test]
    fn text_formatter_shows_worker_route_metadata() {
        let task_id = Uuid::parse_str("12345678-0000-0000-0000-000000000000").unwrap();
        let line = format_text_progress_event(&RunProgress::WorkerStarted {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            runtime: "claude-code".into(),
            cc_agent: Some("reviewer".into()),
            model: "MiniMax-M3".into(),
            provider: Some("minimax".into()),
            prompt: "do work".into(),
        })
        .expect("started line");

        assert_eq!(
            line,
            "● worker   worker-cc started · claude-code · agent reviewer · minimax/MiniMax-M3 · 12345678"
        );
    }

    #[test]
    fn compact_formatter_matches_tui_process_capture_shape() {
        let task_id = Uuid::parse_str("12345678-0000-0000-0000-000000000000").unwrap();

        let direct =
            format_compact_progress_event(&RunProgress::DirectAnswer).expect("direct answer line");
        assert_eq!(direct, "run  answering directly");

        let running = format_compact_progress_event(&RunProgress::Executing { sub_task_count: 1 })
            .expect("executing line");
        assert_eq!(running, "run  running 1 worker run(s)");
        assert!(!running.contains("sub-task"));

        let route = format_compact_progress_event(&RunProgress::RouteSelected {
            route: "single-worker".into(),
            reason: "atomic request; planner, critic, and reviewer are not needed".into(),
        })
        .expect("route line");
        assert_eq!(
            route,
            "route  single-worker · atomic request; planner, critic, and reviewer are not needed"
        );

        let started = format_compact_progress_event(&RunProgress::WorkerStarted {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            runtime: "claude-code".into(),
            cc_agent: None,
            model: "MiniMax-M3".into(),
            provider: Some("minimax".into()),
            prompt: "do work".into(),
        })
        .expect("started line");
        assert_eq!(
            started,
            "worker  worker-cc started · claude-code · minimax/MiniMax-M3 · 12345678"
        );

        let output = format_compact_progress_event(&RunProgress::RoleOutput {
            role: "critic".into(),
            content: r#"{"approved":false,"issues":["missing test"]}"#.into(),
        })
        .expect("critic output");
        assert!(output.contains("critic  needs changes: 1 issue(s)"));
        assert!(output.contains("missing test"));
        assert!(!output.contains('{'));

        let skipped = format_compact_progress_event(&RunProgress::ReviewSkipped {
            task_id,
            reason: "conversational answer".into(),
        })
        .expect("review skipped line");
        assert_eq!(
            skipped,
            "review  skipped · 12345678 · conversational answer"
        );
    }

    #[test]
    fn control_fallback_formats_as_warning_not_role_output() {
        let event = RunProgress::ControlFallback {
            role: "planner".into(),
            message: "planning unavailable; using original task".into(),
            reason: "planner unavailable".into(),
        };

        let line = format_text_progress_event(&event).expect("fallback line");
        assert_eq!(
            line,
            "⚠ planner planning unavailable; using original task · planner unavailable"
        );
        assert!(!line.contains("output"));

        let compact = format_compact_progress_event(&event).expect("compact fallback");
        assert_eq!(
            compact,
            "plan  planning unavailable; using original task · planner unavailable"
        );

        let json =
            serde_json::to_value(TiffanyProgressEvent::from(event)).expect("serializes fallback");
        assert_eq!(json["role"], "planner");
        assert_eq!(json["status"], "warning");
        assert_eq!(json["reason"], "planner unavailable");
        assert_eq!(json["message"], "planning unavailable; using original task");
        assert!(json.get("content").is_none());
    }

    #[test]
    fn json_event_keeps_review_skip_reason_structured() {
        let task_id = Uuid::parse_str("12345678-0000-0000-0000-000000000000").unwrap();
        let event = TiffanyProgressEvent::from(RunProgress::ReviewSkipped {
            task_id,
            reason: "conversational answer".into(),
        });

        let json = serde_json::to_value(event).expect("serializes progress event");

        assert_eq!(json["role"], "reviewer");
        assert_eq!(json["status"], "skipped");
        assert_eq!(json["task_id"], "12345678-0000-0000-0000-000000000000");
        assert_eq!(json["reason"], "conversational answer");
        assert_eq!(json["message"], "review skipped - conversational answer");
    }

    #[test]
    fn direct_answer_event_is_visible_and_structured() {
        let line = format_text_progress_event(&RunProgress::DirectAnswer)
            .expect("direct answer text line");
        assert_eq!(line, "● worker   answering directly");

        let json = serde_json::to_value(TiffanyProgressEvent::from(RunProgress::DirectAnswer))
            .expect("serializes direct answer event");
        assert_eq!(json["role"], "worker");
        assert_eq!(json["status"], "running");
        assert_eq!(json["message"], "answering directly");
        assert!(json.get("count").is_none());
    }

    #[test]
    fn route_selected_event_is_visible_and_structured() {
        let event = RunProgress::RouteSelected {
            route: "single-worker".into(),
            reason: "atomic request; planner, critic, and reviewer are not needed".into(),
        };

        let line = format_text_progress_event(&event).expect("route text line");
        assert_eq!(
            line,
            "● route    single-worker · atomic request; planner, critic, and reviewer are not needed"
        );

        let json =
            serde_json::to_value(TiffanyProgressEvent::from(event)).expect("serializes route");
        assert_eq!(json["role"], "orchestrator");
        assert_eq!(json["status"], "running");
        assert_eq!(json["message"], "route selected - single-worker");
        assert_eq!(json["route"], "single-worker");
        assert_eq!(json["route_label"], "single");
        assert_eq!(json["route_reason_label"], "atomic worker");
        assert_eq!(json["flow_steps"], "worker -> answer");
        assert_eq!(
            json["reason"],
            "atomic request; planner, critic, and reviewer are not needed"
        );
    }
}
