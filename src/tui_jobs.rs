use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::core::session_store::{SessionStore, TuiJob};
use crate::core::types::Session;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuiJobsSurface {
    Cli,
    TerminalTui,
}

pub fn format_tui_jobs(
    store: &SessionStore,
    limit: usize,
    surface: TuiJobsSurface,
) -> Result<String> {
    let jobs = store.list_tui_jobs(limit)?;
    if jobs.is_empty() {
        return Ok(match surface {
            TuiJobsSurface::Cli => "Jobs\n  no persisted jobs yet".to_string(),
            TuiJobsSurface::TerminalTui => {
                "Jobs\n  no persisted jobs yet\n\nQueued messages created while a run is active are tracked here.".to_string()
            }
        });
    }

    Ok(format_tui_jobs_block(store, &jobs, surface))
}

pub fn format_tui_job_detail(
    store: &SessionStore,
    job: &TuiJob,
    surface: TuiJobsSurface,
) -> String {
    format_tui_jobs_block(store, std::slice::from_ref(job), surface)
}

fn format_tui_jobs_block(store: &SessionStore, jobs: &[TuiJob], surface: TuiJobsSurface) -> String {
    let active = store.list_active_tui_jobs().unwrap_or_default();
    let counts = TuiJobCounts::from_jobs(jobs);
    let mut out = format!(
        "Jobs\n  active: {}  queued: {}  running: {}  done: {}  failed: {}  cancelled: {}  shown: {}\n\n",
        active.len(),
        counts.queued,
        counts.running,
        counts.done,
        counts.failed,
        counts.cancelled,
        jobs.len()
    );
    for job in jobs.iter() {
        out.push_str(&format_tui_job_line(job));
        out.push('\n');
        let meta = format_tui_job_meta(store, job, surface);
        if !meta.is_empty() {
            out.push_str("  ");
            out.push_str(&meta);
            out.push('\n');
        }
    }
    match surface {
        TuiJobsSurface::Cli => {
            out.push_str("\nUse `tiffany-loop` and /jobs to inspect this from the TUI.");
        }
        TuiJobsSurface::TerminalTui => {
            out.push_str(
                "\nQueue: /queue show, /queue run. Details: /thread <role>, /history status, or /process 200.",
            );
        }
    }
    out
}

#[derive(Default)]
struct TuiJobCounts {
    queued: usize,
    running: usize,
    done: usize,
    failed: usize,
    cancelled: usize,
}

impl TuiJobCounts {
    fn from_jobs(jobs: &[TuiJob]) -> Self {
        let mut counts = Self::default();
        for job in jobs {
            match job.status.as_str() {
                "queued" => counts.queued += 1,
                "running" => counts.running += 1,
                "done" => counts.done += 1,
                "failed" => counts.failed += 1,
                "cancelled" | "removed" | "skipped" => counts.cancelled += 1,
                _ => {}
            }
        }
        counts
    }
}

fn format_tui_job_line(job: &TuiJob) -> String {
    let id = job.id.to_string();
    format!(
        "{} {:<8} {:<9} {}",
        tui_job_status_icon(&job.status),
        id.chars().take(8).collect::<String>(),
        job.status,
        truncate_for_jobs(&job.prompt.replace('\n', " "), 120)
    )
}

fn format_tui_job_meta(store: &SessionStore, job: &TuiJob, surface: TuiJobsSurface) -> String {
    let mut meta = Vec::new();
    let session = job_worker_session(store, job);
    if let Some(timing) = format_tui_job_timing(job, Utc::now()) {
        meta.push(format!("timing {timing}"));
    }
    if let Some(route) = job.route.as_deref().filter(|value| !value.is_empty()) {
        meta.push(format!("flow {route}"));
    }
    if let Some(role) = job.role.as_deref().filter(|value| !value.is_empty()) {
        meta.push(format!("role {role}"));
    }
    if let Some(task_id) = job.task_id {
        meta.push(format!("task {}", short_uuid(task_id)));
    }
    if let Some(session_id) = job.session_id {
        meta.push(format!(
            "session {}",
            session_id.to_string().chars().take(8).collect::<String>()
        ));
    }
    let worker_thread_id = job.worker_thread_id.or_else(|| {
        session
            .as_ref()
            .and_then(|session| session.worker_thread_id)
    });
    if let Some(worker_thread_id) = worker_thread_id {
        meta.push(format!("thread {}", short_uuid(worker_thread_id)));
    }
    let native_session_id = job.native_session_id.as_deref().or_else(|| {
        session
            .as_ref()
            .and_then(|session| session.native_session_id.as_deref())
    });
    if let Some(native) = native_session_id.filter(|value| !value.is_empty()) {
        meta.push(format!("native {}", truncate_for_jobs(native, 96)));
    }
    if let Some(error) = job.error.as_deref().filter(|value| !value.is_empty()) {
        meta.push(format!("error {}", truncate_for_jobs(error, 80)));
    }
    if let Some(result) = job.result.as_deref().filter(|value| !value.is_empty()) {
        meta.push(format!(
            "result {}",
            truncate_for_jobs(&result.replace('\n', " "), 100)
        ));
    }
    if let Some(next) = format_tui_job_next_action(job, surface, native_session_id.is_some()) {
        meta.push(format!("next {next}"));
    }
    if surface == TuiJobsSurface::TerminalTui {
        if let Some(worker_thread_id) = worker_thread_id {
            meta.push(format!(
                "history /history thread {}",
                short_uuid(worker_thread_id)
            ));
        }
    }
    meta.join("  ")
}

fn format_tui_job_timing(job: &TuiJob, now: DateTime<Utc>) -> Option<String> {
    let mut parts = vec![format!(
        "created {}",
        relative_time_label(job.created_at, now)
    )];
    if job.updated_at != job.created_at {
        parts.push(format!(
            "updated {}",
            relative_time_label(job.updated_at, now)
        ));
    }
    match (job.started_at, job.ended_at) {
        (Some(started), Some(ended)) if ended >= started => {
            parts.push(format!("duration {}", duration_label(ended - started)));
        }
        (Some(started), None) if now >= started => {
            parts.push(format!("running {}", duration_label(now - started)));
        }
        _ => {}
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn relative_time_label(ts: DateTime<Utc>, now: DateTime<Utc>) -> String {
    if ts > now {
        return "just now".to_string();
    }
    let delta = now - ts;
    let secs = delta.num_seconds();
    if secs < 5 {
        "just now".to_string()
    } else {
        format!("{} ago", duration_label(delta))
    }
}

fn duration_label(delta: chrono::Duration) -> String {
    let secs = delta.num_seconds().max(0);
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    format!("{days}d")
}

fn job_worker_session(store: &SessionStore, job: &TuiJob) -> Option<Session> {
    let session_id = job.session_id?;
    store
        .get_many(&[session_id])
        .ok()
        .and_then(|mut sessions| sessions.pop())
}

fn format_tui_job_next_action(
    job: &TuiJob,
    surface: TuiJobsSurface,
    has_native_session: bool,
) -> Option<String> {
    let job_id = short_uuid(job.id);
    let command = match job.status.as_str() {
        "queued" => match surface {
            TuiJobsSurface::Cli => "open tiffany-loop and run /queue run".to_string(),
            TuiJobsSurface::TerminalTui => "/queue run".to_string(),
        },
        "running" => match surface {
            TuiJobsSurface::Cli => "open tiffany-loop and run /jobs".to_string(),
            TuiJobsSurface::TerminalTui => "/jobs".to_string(),
        },
        "failed" => match surface {
            TuiJobsSurface::Cli => {
                format!("orchestrator jobs retry {job_id} --emit-retry-prompt")
            }
            TuiJobsSurface::TerminalTui => format!("/jobs retry {job_id}"),
        },
        "done" => {
            let Some(role) = job.role.as_deref().filter(|value| !value.is_empty()) else {
                return job.session_id.map(|id| match surface {
                    TuiJobsSurface::Cli => format!("orchestrator sessions show {id}"),
                    TuiJobsSurface::TerminalTui => format!("/history session {id}"),
                });
            };
            match (surface, has_native_session) {
                (TuiJobsSurface::Cli, true) => {
                    format!("open tiffany-loop and run /continue open {role}")
                }
                (TuiJobsSurface::TerminalTui, true) => format!("/continue open {role}"),
                (TuiJobsSurface::Cli, false) => format!("orchestrator thread show {role}"),
                (TuiJobsSurface::TerminalTui, false) => format!("/thread {role}"),
            }
        }
        _ => return None,
    };
    Some(command)
}

fn tui_job_status_icon(status: &str) -> &'static str {
    match status {
        "running" => "●",
        "queued" => "↳",
        "done" => "✓",
        "failed" => "✗",
        "cancelled" | "removed" | "skipped" => "○",
        _ => "·",
    }
}

fn truncate_for_jobs(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn short_uuid(id: uuid::Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Role;

    fn test_store() -> (tempfile::TempDir, SessionStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        (tmp, store)
    }

    #[test]
    fn jobs_panel_summarizes_status_counts_and_next_actions() {
        let (_tmp, store) = test_store();
        store
            .create_tui_job("queued prompt", "queued", Some("direct"), Some("worker-cc"))
            .unwrap();
        store
            .create_tui_job(
                "running prompt",
                "running",
                Some("single"),
                Some("worker-codex"),
            )
            .unwrap();
        let done = store
            .create_tui_job("done prompt", "running", Some("single"), Some("worker-cc"))
            .unwrap();
        let worker_thread_id = uuid::Uuid::parse_str("12345678-0000-0000-0000-000000000000")
            .expect("valid worker thread id");
        let mut worker_session = Session::new(uuid::Uuid::new_v4(), "worker-cc", Role::Worker);
        worker_session.worker_thread_id = Some(worker_thread_id);
        store.finalize(&worker_session).unwrap();
        store
            .attach_tui_job_session(
                done.id,
                Some(worker_session.id),
                Some("claude-native-session"),
            )
            .unwrap();
        store
            .set_tui_job_status(done.id, "done", Some("finished\ncleanly"), None)
            .unwrap();
        let failed = store
            .create_tui_job("failed prompt", "running", None, Some("worker-gemini"))
            .unwrap();
        store
            .set_tui_job_status(failed.id, "failed", None, Some("model unavailable"))
            .unwrap();

        let rendered = format_tui_jobs(&store, 10, TuiJobsSurface::TerminalTui).unwrap();

        assert!(rendered.contains("active: 2  queued: 1  running: 1  done: 1  failed: 1"));
        assert!(rendered.contains("timing created"));
        assert!(rendered.contains("updated"));
        assert!(rendered.contains("duration"));
        assert!(rendered.contains("next /queue run"));
        assert!(rendered.contains("next /jobs"));
        assert!(rendered.contains("next /continue open worker-cc"));
        assert!(rendered.contains("next /jobs retry"));
        assert!(rendered.contains("session "));
        assert!(rendered.contains("thread 12345678"));
        assert!(rendered.contains("history /history thread 12345678"));
        assert!(rendered.contains("native claude-native-session"));
        assert!(rendered.contains("result finished cleanly"));
        assert!(rendered.contains("error model unavailable"));
        assert!(rendered.contains("/history status"));
        assert!(!rendered.contains("/session last"));
        assert!(!rendered.contains('{'));
    }

    #[test]
    fn jobs_timing_labels_are_human_readable() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-28T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let job = TuiJob {
            id: uuid::Uuid::new_v4(),
            prompt: "timed job".into(),
            status: "done".into(),
            route: Some("single-worker".into()),
            role: Some("worker-cc".into()),
            task_id: None,
            session_id: None,
            worker_thread_id: None,
            native_session_id: None,
            created_at: now - chrono::Duration::minutes(10),
            updated_at: now - chrono::Duration::minutes(2),
            started_at: Some(now - chrono::Duration::minutes(9)),
            ended_at: Some(now - chrono::Duration::minutes(3)),
            result: Some("ok".into()),
            error: None,
        };

        let timing = format_tui_job_timing(&job, now).expect("timing");

        assert_eq!(timing, "created 10m ago · updated 2m ago · duration 6m");
    }

    #[test]
    fn jobs_detail_formats_one_persisted_job() {
        let (_tmp, store) = test_store();
        let target = store
            .create_tui_job(
                "cancel this prompt",
                "queued",
                Some("direct"),
                Some("worker-cc"),
            )
            .unwrap();
        store
            .create_tui_job(
                "do not include",
                "running",
                Some("single"),
                Some("worker-codex"),
            )
            .unwrap();
        store
            .set_tui_job_status(target.id, "cancelled", None, Some("cancelled by user"))
            .unwrap();
        let target = store.get_tui_job(target.id).unwrap().expect("job");

        let rendered = format_tui_job_detail(&store, &target, TuiJobsSurface::TerminalTui);

        assert!(rendered.contains("active: 1"));
        assert!(rendered.contains("cancelled: 1"));
        assert!(rendered.contains("shown: 1"));
        assert!(rendered.contains("cancel this prompt"));
        assert!(rendered.contains("error cancelled by user"));
        assert!(!rendered.contains("do not include"));
    }

    #[test]
    fn jobs_cli_uses_shell_actions_for_follow_up() {
        let (_tmp, store) = test_store();
        let done = store
            .create_tui_job("done prompt", "running", Some("direct"), Some("worker-cc"))
            .unwrap();
        store
            .set_tui_job_status(done.id, "done", Some("ok"), None)
            .unwrap();

        let rendered = format_tui_jobs(&store, 5, TuiJobsSurface::Cli).unwrap();

        assert!(rendered.contains("next orchestrator thread show worker-cc"));
        assert!(rendered.contains("Use `tiffany-loop` and /jobs"));
    }

    #[test]
    fn jobs_cli_points_native_done_jobs_back_to_tui_continue() {
        let (_tmp, store) = test_store();
        let done = store
            .create_tui_job(
                "done prompt",
                "running",
                Some("direct"),
                Some("worker-codex"),
            )
            .unwrap();
        store
            .attach_tui_job_session(done.id, None, Some("codex-native-session"))
            .unwrap();
        store
            .set_tui_job_status(done.id, "done", Some("ok"), None)
            .unwrap();

        let rendered = format_tui_jobs(&store, 5, TuiJobsSurface::Cli).unwrap();

        assert!(rendered.contains("native codex-native-session"));
        assert!(rendered.contains("next open tiffany-loop and run /continue open worker-codex"));
    }

    #[test]
    fn jobs_next_action_uses_worker_session_native_id_for_continue() {
        let (_tmp, store) = test_store();
        let done = store
            .create_tui_job(
                "done prompt",
                "running",
                Some("single-worker"),
                Some("worker-gemini"),
            )
            .unwrap();
        let mut worker_session = Session::new(uuid::Uuid::new_v4(), "gemini", Role::Worker);
        worker_session.native_session_id = Some("gemini-native-session".to_string());
        store.finalize(&worker_session).unwrap();
        store
            .attach_tui_job_session(done.id, Some(worker_session.id), None)
            .unwrap();
        store
            .set_tui_job_status(done.id, "done", Some("ok"), None)
            .unwrap();

        let rendered = format_tui_jobs(&store, 5, TuiJobsSurface::TerminalTui).unwrap();

        assert!(rendered.contains("native gemini-native-session"));
        assert!(rendered.contains("next /continue open worker-gemini"));
        assert!(!rendered.contains("next /thread worker-gemini"));
    }

    #[test]
    fn jobs_failed_next_action_points_to_retry() {
        let (_tmp, store) = test_store();
        let failed = store
            .create_tui_job(
                "failed prompt",
                "running",
                Some("single-worker"),
                Some("worker-cc"),
            )
            .unwrap();
        store
            .set_tui_job_status(failed.id, "failed", None, Some("model unavailable"))
            .unwrap();
        let short = short_uuid(failed.id);

        let cli = format_tui_jobs(&store, 5, TuiJobsSurface::Cli).unwrap();
        let tui = format_tui_jobs(&store, 5, TuiJobsSurface::TerminalTui).unwrap();

        assert!(cli.contains(&format!(
            "next orchestrator jobs retry {short} --emit-retry-prompt"
        )));
        assert!(tui.contains(&format!("next /jobs retry {short}")));
        assert!(!tui.contains("next /process 200"));
    }
}
