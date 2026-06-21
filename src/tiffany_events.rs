use crate::agent_events;
use crate::pipeline::orchestrator::RunProgress;
use serde::Serialize;
use std::collections::HashSet;

const TEXT_OUTPUT_SUMMARY_MAX_CHARS: usize = 360;
const COMPACT_OUTPUT_SUMMARY_MAX_CHARS: usize = 180;

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
}

impl From<RunProgress> for TiffanyProgressEvent {
    fn from(event: RunProgress) -> Self {
        match event {
            RunProgress::Planning => Self {
                role: "planner",
                status: "running",
                message: "planning".to_string(),
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
            },
            RunProgress::Planned { sub_task_count } => Self {
                role: "planner",
                status: "done",
                message: format!("plan ready - {sub_task_count} sub-task(s)"),
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
                count: Some(sub_task_count),
                duration_ms: None,
            },
            RunProgress::Critiquing { round } => Self {
                role: "critic",
                status: "running",
                message: format!("checking plan - round {round}"),
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
            },
            RunProgress::CritiqueResult { approved, issues } => Self {
                role: "critic",
                status: if approved { "done" } else { "warning" },
                message: if approved {
                    "plan approved".to_string()
                } else {
                    format!("plan needs fixes - {issues} issue(s)")
                },
                task_id: None,
                agent: None,
                worker_role: None,
                runtime: None,
                cc_agent: None,
                model: None,
                provider: None,
                task_prompt: None,
                content: None,
                approved: Some(approved),
                issues: Some(issues),
                count: None,
                duration_ms: None,
            },
            RunProgress::Replanning { attempt } => Self {
                role: "planner",
                status: "running",
                message: format!("replanning - attempt {attempt}"),
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
            },
            RunProgress::Executing { sub_task_count } => Self {
                role: "worker",
                status: "running",
                message: format!("running {sub_task_count} sub-task(s)"),
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
                count: Some(sub_task_count),
                duration_ms: None,
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
            } => Self {
                role: "worker",
                status: "running",
                message: format!("{role} started"),
                task_id: Some(task_id.to_string()),
                agent: Some(agent),
                worker_role: Some(role),
                runtime: Some(runtime),
                cc_agent,
                model: Some(model),
                provider,
                task_prompt: Some(prompt),
                content: None,
                approved: None,
                issues: None,
                count: None,
                duration_ms: None,
            },
            RunProgress::WorkerOutput {
                task_id,
                agent,
                role,
                content,
            } => Self {
                role: "worker",
                status: "output",
                message: format!("{role} output"),
                task_id: Some(task_id.to_string()),
                agent: Some(agent),
                worker_role: Some(role),
                runtime: None,
                cc_agent: None,
                model: None,
                provider: None,
                task_prompt: None,
                content: Some(content),
                approved: None,
                issues: None,
                count: None,
                duration_ms: None,
            },
            RunProgress::RoleOutput { role, content } => {
                let role = match role.as_str() {
                    "planner" => "planner",
                    "critic" => "critic",
                    "reviewer" => "reviewer",
                    "worker" => "worker",
                    _ => "orchestrator",
                };
                Self {
                    role,
                    status: "output",
                    message: format!("{role} output"),
                    task_id: None,
                    agent: None,
                    worker_role: None,
                    runtime: None,
                    cc_agent: None,
                    model: None,
                    provider: None,
                    task_prompt: None,
                    content: Some(content),
                    approved: None,
                    issues: None,
                    count: None,
                    duration_ms: None,
                }
            }
            RunProgress::WorkerDone {
                task_id,
                agent,
                role,
                duration_ms,
                ok,
            } => Self {
                role: "worker",
                status: if ok { "done" } else { "failed" },
                message: if ok {
                    format!("{role} done")
                } else {
                    format!("{role} failed")
                },
                task_id: Some(task_id.to_string()),
                agent: Some(agent),
                worker_role: Some(role),
                runtime: None,
                cc_agent: None,
                model: None,
                provider: None,
                task_prompt: None,
                content: None,
                approved: None,
                issues: None,
                count: None,
                duration_ms: Some(duration_ms),
            },
            RunProgress::Reviewing { task_id } => Self {
                role: "reviewer",
                status: "running",
                message: "reviewing worker output".to_string(),
                task_id: Some(task_id.to_string()),
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
            },
            RunProgress::ReviewSkipped { task_id, reason } => Self {
                role: "reviewer",
                status: "skipped",
                message: format!("review skipped - {reason}"),
                task_id: Some(task_id.to_string()),
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
            },
            RunProgress::ReviewResult {
                task_id,
                approved,
                issues,
            } => Self {
                role: "reviewer",
                status: if approved { "done" } else { "warning" },
                message: if approved {
                    "review approved".to_string()
                } else {
                    format!("review needs fixes - {issues} issue(s)")
                },
                task_id: Some(task_id.to_string()),
                agent: None,
                worker_role: None,
                runtime: None,
                cc_agent: None,
                model: None,
                provider: None,
                task_prompt: None,
                content: None,
                approved: Some(approved),
                issues: Some(issues),
                count: None,
                duration_ms: None,
            },
            RunProgress::Done { task_count } => Self {
                role: "orchestrator",
                status: "done",
                message: format!("done - {task_count} sub-task(s)"),
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
                count: Some(task_count),
                duration_ms: None,
            },
            RunProgress::Failed(message) => Self {
                role: "orchestrator",
                status: "failed",
                message,
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
            },
        }
    }
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
        RunProgress::Planning => Some("● planner  planning".into()),
        RunProgress::Planned { sub_task_count } => Some(format!(
            "✓ planner  plan ready · {sub_task_count} sub-task(s)"
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
        RunProgress::Executing { sub_task_count } => {
            Some(format!("● worker   running {sub_task_count} sub-task(s)"))
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
            let output = visible_non_final_agent_output(content, TEXT_OUTPUT_SUMMARY_MAX_CHARS)?;
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
        RunProgress::Done { task_count } => {
            Some(format!("✓ done     {task_count} sub-task(s) completed"))
        }
        RunProgress::Failed(message) => Some(format!(
            "✗ error    {}",
            agent_events::humanize_jsonish(message, TEXT_OUTPUT_SUMMARY_MAX_CHARS)
        )),
    }
}

pub fn format_compact_progress_event(event: &RunProgress) -> Option<String> {
    match event {
        RunProgress::Planning => Some("plan  planning work".into()),
        RunProgress::Planned { sub_task_count } => {
            Some(format!("plan  plan ready · {sub_task_count} sub-task(s)"))
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
        RunProgress::Executing { sub_task_count } => {
            Some(format!("run  running {sub_task_count} sub-task(s)"))
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
        RunProgress::Done { task_count } => {
            Some(format!("done  {task_count} sub-task(s) completed"))
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
}
