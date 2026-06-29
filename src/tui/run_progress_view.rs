//! Shared presentation model for orchestrator run progress.

use super::util::format_duration_ms;
use crate::agent_events::OrchestrationRoute;
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
        RunProgress::RouteSelected { route, reason } => Some(running(format_route_line(
            route,
            reason,
            RouteLineStyle::History,
        ))),
        RunProgress::RouteUpdated { route, reason } => Some(warning(format_route_line(
            route,
            reason,
            RouteLineStyle::History,
        ))),
        RunProgress::Planning => Some(running("planning")),
        RunProgress::Planned { sub_task_count } => Some(success(format!(
            "plan ready — {sub_task_count} worker run(s)"
        ))),
        RunProgress::WorkerReady { run_count, route } => Some(success(format_worker_ready_line(
            *run_count,
            route,
            RouteLineStyle::History,
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
            Some(running(format!("running {sub_task_count} worker run(s)")))
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
        RunProgress::WorkerThreadReady {
            task_id,
            role,
            thread_id,
            native_session_id,
            reused,
        } => Some(success(format_worker_thread_line(
            role,
            task_id,
            thread_id,
            native_session_id.as_deref(),
            *reused,
        ))),
        RunProgress::WorkerThreadWaiting {
            task_id,
            role,
            thread_id,
            native_session_id,
        } => Some(running(format_worker_thread_waiting_line(
            role,
            task_id,
            thread_id,
            native_session_id.as_deref(),
        ))),
        RunProgress::WorkerRecovery {
            task_id,
            role,
            thread_id,
            native_session_id,
            recovery,
            ..
        } => Some(warning(format_worker_recovery_line(
            role,
            task_id,
            thread_id,
            native_session_id.as_deref(),
            recovery,
        ))),
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
                Some(warning(format_review_lifecycle_line(
                    "needs fixes",
                    task_id,
                    Some(format!("{issues} issue(s)")),
                )))
            }
        }
        RunProgress::ReviewUnavailable { task_id, message } => Some(warning(
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
        RunProgress::RouteSelected { route, reason } => {
            let summary = route_summary(route, reason);
            Some(status(
                format!("route: {}", summary.label),
                summary.reason,
                Some(format_route_line(route, reason, RouteLineStyle::Assistant)),
            ))
        }
        RunProgress::RouteUpdated { route, reason } => {
            let summary = route_summary(route, reason);
            Some(status(
                format!("route: {}", summary.label),
                summary.reason,
                Some(format_route_line(route, reason, RouteLineStyle::Assistant)),
            ))
        }
        RunProgress::Planning => Some(status(
            "Planning",
            "decomposing task",
            Some("▸ Planning — decomposing task…"),
        )),
        RunProgress::Planned { sub_task_count } => Some(status(
            format!("Plan ready ({sub_task_count} worker run(s))"),
            "moving to critique",
            Some(format!(
                "▸ Plan ready · {sub_task_count} worker run(s). Moving to critique…"
            )),
        )),
        RunProgress::WorkerReady { run_count, route } => {
            let summary = route_summary(route, "");
            Some(status(
                format!("Worker ready ({run_count} run(s))"),
                format!("{} · {}", summary.label, summary.reason),
                Some(format_worker_ready_line(
                    *run_count,
                    route,
                    RouteLineStyle::Assistant,
                )),
            ))
        }
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
        } => {
            let display =
                crate::agent_events::control_fallback_display(role, message, reason, usize::MAX);
            Some(status(
                format!("{}: fallback", display.role_label),
                display.summary.clone(),
                Some(format!("⚠ {}  {}", display.role_label, display.summary)),
            ))
        }
        RunProgress::DirectAnswer => Some(status(
            "Direct answer",
            "running worker",
            Some("▸ Answering directly…"),
        )),
        RunProgress::Executing { sub_task_count } => Some(status(
            format!("Running {sub_task_count} worker run(s)"),
            "workers active",
            Some(format!("▸ Workers running · {sub_task_count} run(s)…")),
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
        RunProgress::WorkerThreadReady {
            task_id,
            role,
            thread_id,
            native_session_id,
            reused,
        } => {
            let id = short_task_id(task_id);
            let native = native_session_id
                .as_deref()
                .map(str::trim)
                .unwrap_or("none");
            Some(status(
                format!(
                    "worker: {role} thread {} · {id}",
                    if *reused { "reused" } else { "created" }
                ),
                format!("thread {} · native {native}", short_task_id(thread_id)),
                Some(format!(
                    "✓ worker  {}",
                    format_worker_thread_line(
                        role,
                        task_id,
                        thread_id,
                        native_session_id.as_deref(),
                        *reused
                    )
                )),
            ))
        }
        RunProgress::WorkerThreadWaiting {
            task_id,
            role,
            thread_id,
            native_session_id,
        } => {
            let id = short_task_id(task_id);
            let native = native_session_id
                .as_deref()
                .map(str::trim)
                .unwrap_or("none");
            Some(status(
                format!("worker: {role} waiting · {id}"),
                format!("thread {} · native {native}", short_task_id(thread_id)),
                Some(format!(
                    "● worker  {}",
                    format_worker_thread_waiting_line(
                        role,
                        task_id,
                        thread_id,
                        native_session_id.as_deref()
                    )
                )),
            ))
        }
        RunProgress::WorkerRecovery {
            task_id,
            role,
            thread_id,
            native_session_id,
            recovery,
            content,
            ..
        } => {
            let id = short_task_id(task_id);
            let native = native_session_id
                .as_deref()
                .map(str::trim)
                .unwrap_or("none");
            Some(status(
                format!("worker: {role} recovery · {recovery} · {id}"),
                format!("thread {} · native {native}", short_task_id(thread_id)),
                Some(format!(
                    "⚠ worker  {}\n{}",
                    format_worker_recovery_line(
                        role,
                        task_id,
                        thread_id,
                        native_session_id.as_deref(),
                        recovery
                    ),
                    content
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

fn format_worker_thread_line(
    role: &str,
    task_id: &uuid::Uuid,
    thread_id: &uuid::Uuid,
    native_session_id: Option<&str>,
    reused: bool,
) -> String {
    let native = native_session_id
        .map(str::trim)
        .map(|id| format!(" · native {id}"))
        .unwrap_or_default();
    format!(
        "{role} thread {} · {} · task {}{native}",
        if reused { "reused" } else { "created" },
        short_task_id(thread_id),
        short_task_id(task_id)
    )
}

fn format_worker_thread_waiting_line(
    role: &str,
    task_id: &uuid::Uuid,
    thread_id: &uuid::Uuid,
    native_session_id: Option<&str>,
) -> String {
    let native = native_session_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| format!(" · native {id}"))
        .unwrap_or_default();
    format!(
        "{role} waiting for worker session · thread {} · task {}{native}",
        short_task_id(thread_id),
        short_task_id(task_id),
    )
}

fn format_worker_recovery_line(
    role: &str,
    task_id: &uuid::Uuid,
    thread_id: &uuid::Uuid,
    native_session_id: Option<&str>,
    recovery: &str,
) -> String {
    let native = native_session_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| format!(" · native {id}"))
        .unwrap_or_default();
    format!(
        "{role} native session recovery · {recovery} · thread {} · task {}{native}",
        short_task_id(thread_id),
        short_task_id(task_id),
    )
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
    let display = crate::agent_events::control_fallback_display(role, message, reason, usize::MAX);
    format!("{}  {}", display.role_label, display.summary)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteLineStyle {
    History,
    Assistant,
}

struct RouteSummary {
    label: String,
    reason: String,
    steps: Option<&'static str>,
}

fn route_summary(route: &str, reason: &str) -> RouteSummary {
    let metadata = OrchestrationRoute::from_label_or_reason(route, reason);
    RouteSummary {
        label: metadata
            .map(|route| route.display_label().to_string())
            .unwrap_or_else(|| route.trim().to_string()),
        reason: metadata
            .map(|route| route.short_reason_label().to_string())
            .unwrap_or_else(|| reason.trim().to_string()),
        steps: metadata.map(OrchestrationRoute::flow_steps),
    }
}

fn format_route_line(route: &str, reason: &str, style: RouteLineStyle) -> String {
    let summary = route_summary(route, reason);
    let label = if summary.label.trim().is_empty() {
        route.trim()
    } else {
        summary.label.as_str()
    };
    let reason = summary.reason.trim();
    let mut line = match style {
        RouteLineStyle::History => format!("route {label}"),
        RouteLineStyle::Assistant => format!("▸ route {label}"),
    };
    if !reason.is_empty() {
        line.push_str(" · ");
        line.push_str(reason);
    }
    if let Some(steps) = summary.steps {
        line.push_str(" · ");
        line.push_str(steps);
    }
    line
}

fn format_worker_ready_line(run_count: usize, route: &str, style: RouteLineStyle) -> String {
    let summary = route_summary(route, "");
    let mut parts = vec![format!("{run_count} run(s)")];
    if !summary.label.trim().is_empty() {
        parts.push(summary.label);
    }
    if !summary.reason.trim().is_empty() {
        parts.push(summary.reason);
    }
    if let Some(steps) = summary.steps {
        parts.push(steps.to_string());
    }
    match style {
        RouteLineStyle::History => format!("worker ready · {}", parts.join(" · ")),
        RouteLineStyle::Assistant => {
            format!("▸ Worker ready · {}. Starting worker…", parts.join(" · "))
        }
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
    fn worker_thread_line_keeps_full_native_session_id() {
        let task_id = uuid::Uuid::parse_str("12345678-0000-0000-0000-000000000000").unwrap();
        let thread_id = uuid::Uuid::parse_str("87654321-0000-0000-0000-000000000000").unwrap();

        let view = progress_history_view(&RunProgress::WorkerThreadReady {
            task_id,
            role: "worker-cc".into(),
            thread_id,
            native_session_id: Some("fake-claude-native-session".into()),
            reused: true,
        })
        .expect("thread ready view");

        assert!(view.line.contains("native fake-claude-native-session"));
        assert!(!view.line.contains("native fake-cla "));

        let status = run_status_view(&RunProgress::WorkerThreadReady {
            task_id,
            role: "worker-cc".into(),
            thread_id,
            native_session_id: Some("fake-claude-native-session".into()),
            reused: true,
        })
        .expect("thread ready status");
        assert!(status.detail.contains("native fake-claude-native-session"));

        let waiting = progress_history_view(&RunProgress::WorkerThreadWaiting {
            task_id,
            role: "worker-cc".into(),
            thread_id,
            native_session_id: Some("fake-claude-native-session".into()),
        })
        .expect("thread waiting view");
        assert_eq!(waiting.icon, "●");
        assert_eq!(waiting.tone, ProgressTone::Running);
        assert!(waiting.line.contains("waiting for worker session"));
        assert!(waiting.line.contains("native fake-claude-native-session"));

        let waiting_status = run_status_view(&RunProgress::WorkerThreadWaiting {
            task_id,
            role: "worker-cc".into(),
            thread_id,
            native_session_id: Some("fake-claude-native-session".into()),
        })
        .expect("thread waiting status");
        assert_eq!(waiting_status.stage, "worker: worker-cc waiting · 12345678");
        assert!(waiting_status
            .detail
            .contains("native fake-claude-native-session"));
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

        let needs_fixes = progress_history_view(&RunProgress::ReviewResult {
            task_id,
            approved: false,
            issues: 2,
        })
        .expect("review needs fixes history");
        assert_eq!(needs_fixes.icon, "⚠");
        assert_eq!(needs_fixes.tone, ProgressTone::Running);
        assert_eq!(
            needs_fixes.line,
            "review  needs fixes · 00000000 · 2 issue(s)"
        );

        let unavailable = progress_history_view(&RunProgress::ReviewUnavailable {
            task_id,
            message: "no JSON found".into(),
        })
        .expect("review unavailable history");
        assert_eq!(unavailable.icon, "⚠");
        assert_eq!(
            unavailable.line,
            "review  unavailable · 00000000 · no JSON found"
        );

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

        let executing = progress_history_view(&RunProgress::Executing { sub_task_count: 1 })
            .expect("executing view");
        assert_eq!(executing.line, "running 1 worker run(s)");
        assert!(!executing.line.contains("sub-task"));

        let route = progress_history_view(&RunProgress::RouteSelected {
            route: "single-worker".into(),
            reason: "atomic request; planner, critic, and reviewer are not needed".into(),
        })
        .expect("route view");
        assert_eq!(route.icon, "●");
        assert_eq!(
            route.line,
            "route single · atomic worker · worker -> answer"
        );
        assert!(!route.line.contains("planner, critic"));

        let full_route = progress_history_view(&RunProgress::RouteSelected {
            route: "full-pipeline".into(),
            reason:
                "project or implementation work; planner, critic, worker, and reviewer will run"
                    .into(),
        })
        .expect("full route view");
        assert_eq!(
            full_route.line,
            "route full · implementation · planner -> critic -> worker -> reviewer -> answer"
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
        assert_eq!(route_status.stage, "route: single");
        assert_eq!(route_status.detail, "atomic worker");
        assert_eq!(
            route_status.assistant_update.as_deref(),
            Some("▸ route single · atomic worker · worker -> answer")
        );

        let worker_ready = progress_history_view(&RunProgress::WorkerReady {
            run_count: 1,
            route: "single-worker".into(),
        })
        .expect("worker ready view");
        assert_eq!(worker_ready.icon, "✓");
        assert_eq!(
            worker_ready.line,
            "worker ready · 1 run(s) · single · atomic worker · worker -> answer"
        );

        let worker_ready_status = run_status_view(&RunProgress::WorkerReady {
            run_count: 1,
            route: "single-worker".into(),
        })
        .expect("worker ready status");
        assert_eq!(worker_ready_status.stage, "Worker ready (1 run(s))");
        assert_eq!(worker_ready_status.detail, "single · atomic worker");
        assert_eq!(
            worker_ready_status.assistant_update.as_deref(),
            Some(
                "▸ Worker ready · 1 run(s) · single · atomic worker · worker -> answer. Starting worker…"
            )
        );

        let direct_ready = progress_history_view(&RunProgress::WorkerReady {
            run_count: 1,
            route: "direct-answer".into(),
        })
        .expect("direct worker ready view");
        assert_eq!(
            direct_ready.line,
            "worker ready · 1 run(s) · direct · chat/explain · worker -> answer"
        );

        let direct_ready_status = run_status_view(&RunProgress::WorkerReady {
            run_count: 1,
            route: "direct-answer".into(),
        })
        .expect("direct worker ready status");
        assert_eq!(direct_ready_status.stage, "Worker ready (1 run(s))");
        assert_eq!(direct_ready_status.detail, "direct · chat/explain");
        assert_eq!(
            direct_ready_status.assistant_update.as_deref(),
            Some(
                "▸ Worker ready · 1 run(s) · direct · chat/explain · worker -> answer. Starting worker…"
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
        assert_eq!(
            fallback_status.detail,
            "single-worker fallback · planner unavailable"
        );
        assert_eq!(
            fallback_status.assistant_update.as_deref(),
            Some("⚠ planner  single-worker fallback · planner unavailable")
        );

        let soft_fallback = progress_history_view(&RunProgress::ControlFallback {
            role: "planner".into(),
            message: "replan fell back to original task".into(),
            reason: "replanner returned no usable worker runs; using original task".into(),
        })
        .expect("soft fallback history");
        assert_eq!(
            soft_fallback.line,
            "planner  single-worker fallback · planner returned no worker runs"
        );
    }
}
