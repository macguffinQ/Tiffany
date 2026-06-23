//! Shared presentation model for orchestrator run progress.

use super::util::format_duration_ms;
use crate::pipeline::orchestrator::RunProgress;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProgressTone {
    Success,
    Running,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProgressHistoryView {
    pub(super) icon: &'static str,
    pub(super) tone: ProgressTone,
    pub(super) line: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RunStatusView {
    pub(super) stage: String,
    pub(super) detail: String,
    pub(super) assistant_update: Option<String>,
}

pub(super) fn progress_history_view(event: &RunProgress) -> Option<ProgressHistoryView> {
    match event {
        RunProgress::RouteSelected { route, reason } => {
            Some(running(format!("route {route} — {reason}")))
        }
        RunProgress::Planning => Some(running("planning")),
        RunProgress::Planned { sub_task_count } => Some(success(format!(
            "plan ready — {sub_task_count} sub-task(s)"
        ))),
        RunProgress::Critiquing { round } => {
            Some(running(format!("checking plan — round {round}")))
        }
        RunProgress::CritiqueResult { approved, issues } => {
            if *approved {
                Some(success("plan approved"))
            } else {
                Some(running(format!("plan needs changes — {issues} issue(s)")))
            }
        }
        RunProgress::Replanning { attempt } => {
            Some(running(format!("replanning — attempt {attempt}")))
        }
        RunProgress::ControlFallback {
            role,
            message,
            reason,
        } => Some(warning(format_control_fallback_line(role, message, reason))),
        RunProgress::DirectAnswer => Some(running("answering directly")),
        RunProgress::Executing { sub_task_count } => {
            Some(running(format!("running {sub_task_count} sub-task(s)")))
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
            let runtime_label = runtime_with_cc_agent(runtime, cc_agent.as_deref());
            Some(running(format_worker_lifecycle_line(
                role,
                "started",
                Some(runtime_label.as_str()),
                Some(provider_model_label(provider.as_deref(), model)),
                task_id,
                None,
            )))
        }
        RunProgress::WorkerDone {
            task_id,
            role,
            duration_ms,
            ok,
            ..
        } => {
            let line = format_worker_lifecycle_line(
                role,
                if *ok { "done" } else { "failed" },
                None,
                None,
                task_id,
                Some(format_duration_ms(*duration_ms)),
            );
            if *ok {
                Some(success(line))
            } else {
                Some(error(line))
            }
        }
        RunProgress::Reviewing { task_id } => Some(running(format_review_lifecycle_line(
            "checking worker output",
            task_id,
            None,
        ))),
        RunProgress::ReviewSkipped { task_id, reason } => Some(success(
            format_review_lifecycle_line("skipped", task_id, Some(reason.clone())),
        )),
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            if *approved {
                Some(success(format_review_lifecycle_line(
                    "passed", task_id, None,
                )))
            } else {
                Some(running(format_review_lifecycle_line(
                    "needs fixes",
                    task_id,
                    Some(format!("{issues} issue(s)")),
                )))
            }
        }
        RunProgress::ReviewUnavailable { task_id, message } => Some(running(
            format_review_lifecycle_line("unavailable", task_id, Some(message.clone())),
        )),
        RunProgress::Failed(message) => Some(error(format!("error — {message}"))),
        RunProgress::WorkerOutput { .. }
        | RunProgress::RoleOutput { .. }
        | RunProgress::Done { .. } => None,
    }
}

pub(super) fn run_status_view(event: &RunProgress) -> Option<RunStatusView> {
    match event {
        RunProgress::RouteSelected { route, reason } => Some(status(
            format!("Route: {route}"),
            reason.clone(),
            Some(format!("▸ Route {route} — {reason}")),
        )),
        RunProgress::Planning => Some(status(
            "Planning",
            "decomposing task",
            Some("▸ Planning — decomposing task…"),
        )),
        RunProgress::Planned { sub_task_count } => Some(status(
            format!("Planned ({sub_task_count} sub-tasks)"),
            "moving to critique",
            Some(format!(
                "▸ Planned {sub_task_count} sub-task(s). Moving to critique…"
            )),
        )),
        RunProgress::Critiquing { round } => Some(status(
            format!("Critiquing (round {round})"),
            "adversarial review",
            Some(format!(
                "▸ Critiquing (round {round}) — adversarial review…"
            )),
        )),
        RunProgress::CritiqueResult { approved, issues } => {
            if *approved {
                Some(status(
                    "critic: plan approved",
                    "",
                    Some("✓ critic  plan approved"),
                ))
            } else {
                Some(status(
                    format!("critic: needs changes · {issues} issue(s)"),
                    "",
                    Some(format!("● critic  plan needs changes · {issues} issue(s)")),
                ))
            }
        }
        RunProgress::Replanning { attempt } => Some(status(
            format!("Replanning (attempt {attempt})"),
            "incorporating feedback",
            Some(format!(
                "▸ Replanning (attempt {attempt}) — incorporating feedback…"
            )),
        )),
        RunProgress::ControlFallback {
            role,
            message,
            reason,
        } => Some(status(
            format!("{}: fallback", role_display_name(role)),
            reason.clone(),
            Some(format!(
                "⚠ {}  {} · {}",
                role_display_name(role),
                message,
                reason
            )),
        )),
        RunProgress::DirectAnswer => Some(status(
            "Direct answer",
            "running worker",
            Some("▸ Answering directly…"),
        )),
        RunProgress::Executing { sub_task_count } => Some(status(
            format!("Executing {sub_task_count} sub-task(s)"),
            "running workers",
            Some(format!(
                "▸ Executing {sub_task_count} sub-task(s) — running workers…"
            )),
        )),
        RunProgress::WorkerStarted {
            task_id,
            agent,
            role,
            runtime,
            cc_agent,
            model,
            provider,
            ..
        } => {
            let id = short_task_id(task_id);
            let provider_model = provider_model_label(provider.as_deref(), model);
            let runtime_label = runtime_with_cc_agent(runtime, cc_agent.as_deref());
            Some(status(
                format!("worker: {role} started · {id}"),
                format!("{runtime_label} · {provider_model} · {agent}"),
                Some(format!(
                    "● worker  {role} started · {runtime_label} · {provider_model} · {id}"
                )),
            ))
        }
        RunProgress::WorkerDone {
            task_id,
            agent,
            role,
            duration_ms,
            ok,
        } => {
            let id = short_task_id(task_id);
            Some(status(
                format!(
                    "worker: {role} {} · {id} · {}",
                    if *ok { "done" } else { "failed" },
                    format_duration_ms(*duration_ms)
                ),
                agent.clone(),
                None::<String>,
            ))
        }
        RunProgress::Reviewing { task_id } => {
            let id = short_task_id(task_id);
            Some(status(
                format!("review: checking worker output · {id}"),
                "checking output",
                Some(format!("● review  checking worker output · {id}")),
            ))
        }
        RunProgress::ReviewSkipped { task_id, reason } => {
            let id = short_task_id(task_id);
            Some(status(
                format!("review: skipped · {id}"),
                reason.clone(),
                Some(format!("✓ review  skipped · {id} · {reason}")),
            ))
        }
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => {
            let id = short_task_id(task_id);
            Some(status(
                if *approved {
                    format!("review: passed · {id}")
                } else {
                    format!("review: needs fixes · {id} · {issues} issue(s)")
                },
                "",
                None::<String>,
            ))
        }
        RunProgress::ReviewUnavailable { task_id, message } => {
            let id = short_task_id(task_id);
            Some(status(
                format!("review: unavailable · {id}"),
                message.clone(),
                Some(format!("⚠ review  unavailable · {id} · {message}")),
            ))
        }
        RunProgress::WorkerOutput { .. }
        | RunProgress::RoleOutput { .. }
        | RunProgress::Done { .. }
        | RunProgress::Failed(_) => None,
    }
}

fn success(line: impl Into<String>) -> ProgressHistoryView {
    ProgressHistoryView {
        icon: "✓",
        tone: ProgressTone::Success,
        line: line.into(),
    }
}

fn running(line: impl Into<String>) -> ProgressHistoryView {
    ProgressHistoryView {
        icon: "●",
        tone: ProgressTone::Running,
        line: line.into(),
    }
}

fn warning(line: impl Into<String>) -> ProgressHistoryView {
    ProgressHistoryView {
        icon: "⚠",
        tone: ProgressTone::Running,
        line: line.into(),
    }
}

fn error(line: impl Into<String>) -> ProgressHistoryView {
    ProgressHistoryView {
        icon: "✗",
        tone: ProgressTone::Error,
        line: line.into(),
    }
}

fn status(
    stage: impl Into<String>,
    detail: impl Into<String>,
    assistant_update: Option<impl Into<String>>,
) -> RunStatusView {
    RunStatusView {
        stage: stage.into(),
        detail: detail.into(),
        assistant_update: assistant_update.map(Into::into),
    }
}

fn format_worker_lifecycle_line(
    role: &str,
    action: &str,
    runtime: Option<&str>,
    provider_model: Option<String>,
    task_id: &uuid::Uuid,
    duration: Option<String>,
) -> String {
    let mut parts = vec![format!("{role} {action}")];
    if let Some(runtime) = runtime
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != role.trim())
    {
        parts.push(runtime.to_string());
    }
    if let Some(provider_model) = provider_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(provider_model.to_string());
    }
    parts.push(short_task_id(task_id));
    if let Some(duration) = duration {
        parts.push(duration);
    }
    format!("worker  {}", parts.join(" · "))
}

fn runtime_with_cc_agent(runtime: &str, cc_agent: Option<&str>) -> String {
    cc_agent
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .map(|agent| format!("{runtime} · agent {agent}"))
        .unwrap_or_else(|| runtime.to_string())
}

fn format_review_lifecycle_line(
    action: &str,
    task_id: &uuid::Uuid,
    suffix: Option<String>,
) -> String {
    let mut parts = vec![action.to_string(), short_task_id(task_id)];
    if let Some(suffix) = suffix {
        parts.push(suffix);
    }
    format!("review  {}", parts.join(" · "))
}

fn format_control_fallback_line(role: &str, message: &str, reason: &str) -> String {
    format!(
        "{}  {} · {}",
        role_display_name(role),
        message.trim(),
        reason.trim()
    )
}

fn role_display_name(role: &str) -> &'static str {
    match role {
        "planner" => "planner",
        "critic" => "critic",
        "reviewer" => "reviewer",
        _ => "control",
    }
}

fn provider_model_label(provider: Option<&str>, model: &str) -> String {
    match provider.map(str::trim).filter(|value| !value.is_empty()) {
        Some(provider) => format!("{provider}/{model}"),
        None => model.to_string(),
    }
}

fn short_task_id(task_id: &uuid::Uuid) -> String {
    task_id.to_string()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_history_lines_match_terminal_waterfall() {
        let task_id = uuid::Uuid::nil();
        let started = progress_history_view(&RunProgress::WorkerStarted {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            runtime: "claude-code".into(),
            cc_agent: None,
            model: "MiniMax-M3".into(),
            provider: Some("minimax".into()),
            prompt: "do it".into(),
        })
        .expect("worker started");

        assert_eq!(started.icon, "●");
        assert_eq!(started.tone, ProgressTone::Running);
        assert_eq!(
            started.line,
            "worker  worker-cc started · claude-code · minimax/MiniMax-M3 · 00000000"
        );

        let done = progress_history_view(&RunProgress::WorkerDone {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            duration_ms: 1_250,
            ok: true,
        })
        .expect("worker done");

        assert_eq!(done.icon, "✓");
        assert_eq!(done.line, "worker  worker-cc done · 00000000 · 1.2s");
    }

    #[test]
    fn status_view_updates_stage_and_assistant_text() {
        let task_id = uuid::Uuid::nil();

        let planning = run_status_view(&RunProgress::Planning).expect("planning view");
        assert_eq!(planning.stage, "Planning");
        assert_eq!(planning.detail, "decomposing task");
        assert_eq!(
            planning.assistant_update.as_deref(),
            Some("▸ Planning — decomposing task…")
        );

        let review = run_status_view(&RunProgress::ReviewResult {
            task_id,
            approved: false,
            issues: 2,
        })
        .expect("review view");
        assert_eq!(review.stage, "review: needs fixes · 00000000 · 2 issue(s)");
        assert_eq!(review.detail, "");
        assert_eq!(review.assistant_update, None);

        let skipped = progress_history_view(&RunProgress::ReviewSkipped {
            task_id,
            reason: "conversational answer".into(),
        })
        .expect("review skipped view");
        assert_eq!(skipped.icon, "✓");
        assert_eq!(
            skipped.line,
            "review  skipped · 00000000 · conversational answer"
        );

        let direct = progress_history_view(&RunProgress::DirectAnswer).expect("direct view");
        assert_eq!(direct.icon, "●");
        assert_eq!(direct.line, "answering directly");

        let route = progress_history_view(&RunProgress::RouteSelected {
            route: "single-worker".into(),
            reason: "atomic request; planner, critic, and reviewer are not needed".into(),
        })
        .expect("route view");
        assert_eq!(route.icon, "●");
        assert_eq!(
            route.line,
            "route single-worker — atomic request; planner, critic, and reviewer are not needed"
        );

        let direct_status = run_status_view(&RunProgress::DirectAnswer).expect("direct status");
        assert_eq!(direct_status.stage, "Direct answer");
        assert_eq!(direct_status.detail, "running worker");
        assert_eq!(
            direct_status.assistant_update.as_deref(),
            Some("▸ Answering directly…")
        );

        let route_status = run_status_view(&RunProgress::RouteSelected {
            route: "single-worker".into(),
            reason: "atomic request; planner, critic, and reviewer are not needed".into(),
        })
        .expect("route status");
        assert_eq!(route_status.stage, "Route: single-worker");
        assert_eq!(
            route_status.detail,
            "atomic request; planner, critic, and reviewer are not needed"
        );
        assert_eq!(
            route_status.assistant_update.as_deref(),
            Some(
                "▸ Route single-worker — atomic request; planner, critic, and reviewer are not needed"
            )
        );

        let fallback = progress_history_view(&RunProgress::ControlFallback {
            role: "critic".into(),
            message: "critique unavailable; continuing with current plan".into(),
            reason: "critic unavailable".into(),
        })
        .expect("fallback history view");
        assert_eq!(fallback.icon, "⚠");
        assert_eq!(fallback.tone, ProgressTone::Running);
        assert_eq!(
            fallback.line,
            "critic  critique unavailable; continuing with current plan · critic unavailable"
        );

        let fallback_status = run_status_view(&RunProgress::ControlFallback {
            role: "planner".into(),
            message: "planning unavailable; using original task".into(),
            reason: "planner unavailable".into(),
        })
        .expect("fallback status");
        assert_eq!(fallback_status.stage, "planner: fallback");
        assert_eq!(fallback_status.detail, "planner unavailable");
        assert_eq!(
            fallback_status.assistant_update.as_deref(),
            Some("⚠ planner  planning unavailable; using original task · planner unavailable")
        );
    }
}
