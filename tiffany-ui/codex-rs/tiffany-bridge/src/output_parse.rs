#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleProfileRow {
    pub role: String,
    pub model: String,
    pub runtime: String,
    pub agent_teams: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSummary {
    pub id: String,
    pub status: String,
    pub prompt: String,
    pub flow: Option<String>,
    pub role: Option<String>,
    pub task: Option<String>,
    pub timing: Option<String>,
    pub session: Option<String>,
    pub thread: Option<String>,
    pub native: Option<String>,
    pub history: Option<String>,
    pub next: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobsRetryHandoff {
    pub job_id: String,
    pub prompt: String,
    pub restored_prompt_lines: usize,
}

pub fn parse_role_profile_rows(text: &str) -> Vec<RoleProfileRow> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix('✓')?.trim();
            let mut parts = line.split_whitespace();
            let role = parts.next()?.to_string();
            let mut model = None;
            let mut runtime = None;
            let mut agent_teams = None;
            for part in parts {
                if let Some(value) = part.strip_prefix("model=") {
                    model = Some(value.to_string());
                } else if let Some(value) = part.strip_prefix("runtime=") {
                    runtime = Some(value.to_string());
                } else if let Some(value) = part.strip_prefix("agent_teams=") {
                    agent_teams = Some(value.to_string());
                }
            }
            Some(RoleProfileRow {
                role,
                model: model?,
                runtime: runtime?,
                agent_teams: agent_teams.unwrap_or_else(|| "false".to_string()),
            })
        })
        .collect()
}

pub fn parse_prefixed_field<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(prefix).map(str::trim))
        .filter(|value| !value.is_empty())
}

pub fn retry_prompt_from_jobs_retry_output(text: &str) -> Option<String> {
    parse_prefixed_field(text, "retry prompt:")
        .and_then(unescape_retry_prompt_from_cli)
        .and_then(|prompt| nonempty_trimmed(&prompt).map(str::to_string))
}

pub fn parse_jobs_retry_handoff(text: &str) -> Option<JobsRetryHandoff> {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Job ") && line.contains(" prepared for TUI retry"))?;
    let mut parts = first.split_whitespace();
    (parts.next()? == "Job").then_some(())?;
    let job_id = parts.next()?.trim().to_string();
    if job_id.is_empty() {
        return None;
    }

    let status = parse_prefixed_field(text, "status:")?;
    if !status.contains("current TUI input queue") {
        return None;
    }

    let prompt = parse_prefixed_field(text, "prompt:")
        .map(str::to_string)
        .or_else(|| retry_prompt_from_jobs_retry_output(text).map(|prompt| one_line(&prompt)))?;
    let restored_prompt_lines = retry_prompt_from_jobs_retry_output(text)
        .map(|prompt| prompt.lines().count().max(1))
        .unwrap_or(1);

    Some(JobsRetryHandoff {
        job_id,
        prompt,
        restored_prompt_lines,
    })
}

pub fn unescape_retry_prompt_from_cli(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Some(out)
}

pub fn parse_recovered_jobs_failed_count(detail: &str) -> Option<usize> {
    let (_, rest) = detail.split_once("failed:")?;
    rest.split_whitespace().next()?.parse().ok()
}

pub fn parse_recovered_jobs_ids(detail: &str) -> Vec<String> {
    let Some((_, rest)) = detail.split_once("ids:") else {
        return Vec::new();
    };
    rest.split(',')
        .filter_map(|id| nonempty_trimmed(id).map(str::to_string))
        .collect()
}

pub fn parse_jobs_header_counts(text: &str) -> Option<(usize, usize)> {
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        if !trimmed.starts_with("active:") {
            return None;
        }
        let parts = trimmed.split_whitespace().collect::<Vec<_>>();
        let active = parts
            .windows(2)
            .find(|window| window.first().copied() == Some("active:"))
            .and_then(|window| window.get(1))
            .and_then(|value| value.parse::<usize>().ok())?;
        let shown = parts
            .windows(2)
            .find(|window| window.first().copied() == Some("shown:"))
            .and_then(|window| window.get(1))
            .and_then(|value| value.parse::<usize>().ok())?;
        Some((active, shown))
    })
}

pub fn parse_job_summaries(text: &str) -> Vec<JobSummary> {
    let mut jobs = Vec::new();
    let mut current: Option<JobSummary> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed == "Jobs"
            || trimmed.starts_with("active:")
            || trimmed.starts_with("Use `")
            || trimmed.starts_with("Queue:")
        {
            continue;
        }
        if let Some(job) = parse_job_summary_line(trimmed) {
            if let Some(current) = current.take() {
                jobs.push(current);
            }
            current = Some(job);
            continue;
        }
        if let Some(job) = current.as_mut() {
            parse_job_meta_line_into(trimmed, job);
        }
    }
    if let Some(current) = current {
        jobs.push(current);
    }
    jobs
}

pub fn job_state_summary(job: &JobSummary) -> String {
    match job.status.as_str() {
        "queued" => "waiting in the Tiffany queue; run /queue run when ready".to_string(),
        "running" => "active worker run; refresh with /jobs, inspect /process 200, or recover stale jobs with /jobs recover".to_string(),
        "failed" => format!("needs attention; retry with /jobs retry {}", job.id),
        "done" if job.native.as_deref().and_then(nonempty_trimmed).is_some() => {
            "complete; native session is available for handoff".to_string()
        }
        "done" => "complete; result captured in Tiffany history".to_string(),
        other if !other.trim().is_empty() => format!("saved with status {other}"),
        _ => "saved job".to_string(),
    }
}

pub fn job_actions(job: &JobSummary) -> Vec<String> {
    let role = job.role.as_deref().and_then(nonempty_trimmed);
    let show_job = format!("/jobs show {}", job.id);
    let cancel_job = format!("/jobs cancel {}", job.id);
    let mut actions = match job.status.as_str() {
        "queued" => vec![
            show_job,
            cancel_job,
            "/queue run".to_string(),
            "/queue show".to_string(),
        ],
        "running" => vec![
            show_job,
            cancel_job,
            "/jobs".to_string(),
            "/process 200".to_string(),
            "/jobs recover".to_string(),
        ],
        "failed" => vec![
            show_job,
            format!("/jobs retry {}", job.id),
            "/process 200".to_string(),
            "/jobs".to_string(),
        ],
        "done" => vec![show_job, "/jobs".to_string()],
        "cancelled" | "removed" | "skipped" => {
            vec![
                show_job,
                format!("/jobs retry {}", job.id),
                "/jobs".to_string(),
            ]
        }
        _ => vec![show_job, "/jobs".to_string()],
    };
    if let Some(role) = role {
        match job.status.as_str() {
            "done" => {
                if job.native.as_deref().and_then(nonempty_trimmed).is_some() {
                    push_unique_front_action(&mut actions, format!("/continue open {role}"));
                    push_unique_action(&mut actions, format!("/thread export {role}"));
                }
                push_unique_action(&mut actions, format!("/thread {role}"));
                push_unique_action(&mut actions, format!("/history role {role}"));
            }
            "failed" => {
                push_unique_action(&mut actions, format!("/history role {role}"));
                push_unique_action(&mut actions, format!("/thread {role}"));
            }
            "running" => {
                push_unique_action(&mut actions, format!("/thread {role}"));
                push_unique_action(&mut actions, format!("/history role {role}"));
            }
            _ => {
                push_unique_action(&mut actions, format!("/thread {role}"));
            }
        }
    } else if let Some(session) = job.session.as_deref().and_then(nonempty_trimmed) {
        push_unique_action(&mut actions, format!("/history session {session}"));
    }
    if let Some(thread) = job.thread.as_deref().and_then(nonempty_trimmed) {
        push_unique_action(&mut actions, format!("/history thread {thread}"));
    }
    if let Some(history) = job.history.as_deref().and_then(nonempty_trimmed) {
        push_unique_action(&mut actions, history.to_string());
    }
    if let Some(next) = job
        .next
        .as_deref()
        .and_then(tui_command_from_jobs_next_action)
    {
        push_unique_front_action(&mut actions, next);
    }
    actions
}

fn parse_job_summary_line(line: &str) -> Option<JobSummary> {
    let (symbol, rest) = line.split_once(' ')?;
    if !matches!(symbol, "✓" | "✗" | "●" | "↳" | "○" | "·") {
        return None;
    }
    let mut parts = rest.split_whitespace();
    let id = parts.next()?.trim().to_string();
    let status = parts.next()?.trim().to_string();
    if id.is_empty() || status.is_empty() {
        return None;
    }
    let prompt = parts.collect::<Vec<_>>().join(" ");
    Some(JobSummary {
        id,
        status,
        prompt,
        flow: None,
        role: None,
        task: None,
        timing: None,
        session: None,
        thread: None,
        native: None,
        history: None,
        next: None,
        result: None,
        error: None,
    })
}

fn parse_job_meta_line_into(line: &str, job: &mut JobSummary) {
    for segment in line
        .split("  ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let Some((key, value)) = segment.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "flow" => job.flow = Some(value.to_string()),
            "role" => job.role = Some(value.to_string()),
            "task" => job.task = Some(value.to_string()),
            "timing" => job.timing = Some(value.to_string()),
            "session" => job.session = Some(value.to_string()),
            "thread" => job.thread = Some(value.to_string()),
            "native" => job.native = Some(value.to_string()),
            "history" => job.history = Some(value.to_string()),
            "next" => job.next = Some(value.to_string()),
            "error" => job.error = Some(tiffany_event_format::humanize_jsonish(value, 220)),
            "result" => job.result = Some(tiffany_event_format::humanize_jsonish(value, 220)),
            _ => {}
        }
    }
}

fn tui_command_from_jobs_next_action(next: &str) -> Option<String> {
    let next = nonempty_trimmed(next)?;
    if next.starts_with('/') {
        return Some(next.to_string());
    }
    if let Some(command) = next.strip_prefix("open tiffany-loop and run ") {
        return nonempty_trimmed(command).map(str::to_string);
    }
    if let Some(rest) = next.strip_prefix("orchestrator jobs retry ") {
        let id = rest.split_whitespace().next()?;
        return Some(format!("/jobs retry {id}"));
    }
    if let Some(rest) = next.strip_prefix("orchestrator thread show ") {
        let role = nonempty_trimmed(rest)?;
        return Some(format!("/thread {role}"));
    }
    if let Some(rest) = next.strip_prefix("orchestrator sessions show ") {
        let id = nonempty_trimmed(rest)?;
        return Some(format!("/history session {id}"));
    }
    None
}

fn push_unique_action(actions: &mut Vec<String>, action: String) {
    if !actions.iter().any(|existing| existing == &action) {
        actions.push(action);
    }
}

fn push_unique_front_action(actions: &mut Vec<String>, action: String) {
    if let Some(index) = actions.iter().position(|existing| existing == &action) {
        actions.remove(index);
    }
    actions.insert(0, action);
}

fn nonempty_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_role_profile_rows_and_prefixed_fields() {
        let rows = parse_role_profile_rows(
            "Role profile saved: dev\n  ✓ planner       model=sonnet runtime=claude-code agent_teams=false\n  ✓ worker-cc     model=minimax-m3 runtime=claude-code agent_teams=true\n",
        );

        assert_eq!(
            parse_prefixed_field("Role profile saved: dev", "Role profile saved:"),
            Some("dev")
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, "planner");
        assert_eq!(rows[0].model, "sonnet");
        assert_eq!(rows[0].runtime, "claude-code");
        assert_eq!(rows[0].agent_teams, "false");
        assert_eq!(rows[1].role, "worker-cc");
        assert_eq!(rows[1].agent_teams, "true");
    }

    #[test]
    fn restores_retry_prompt_and_handoff() {
        let output = "Job abcd1234 prepared for TUI retry\n  status: queued in current TUI input queue\n  prompt: first line second line\n  retry prompt: first line\\nsecond line\n";

        assert_eq!(
            retry_prompt_from_jobs_retry_output(output).as_deref(),
            Some("first line\nsecond line")
        );
        let handoff = parse_jobs_retry_handoff(output).expect("handoff");
        assert_eq!(handoff.job_id, "abcd1234");
        assert_eq!(handoff.prompt, "first line second line");
        assert_eq!(handoff.restored_prompt_lines, 2);
        assert!(retry_prompt_from_jobs_retry_output("retry prompt: trailing \\").is_none());
    }

    #[test]
    fn parses_recovered_job_detail() {
        assert_eq!(
            parse_recovered_jobs_failed_count("failed: 2  ids: a,b"),
            Some(2)
        );
        assert_eq!(
            parse_recovered_jobs_ids("failed: 2  ids: a,b"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn parses_jobs_and_actions_without_raw_cli_next_actions() {
        let jobs = parse_job_summaries(
            "Jobs\n\
               active: 0  shown: 2\n\
             \n\
             ✓ 12345678 done      answer user question\n\
               timing created 1m ago  flow direct-answer  role worker-cc  native native-1  next open tiffany-loop and run /continue open worker-cc  result final answer\n\
             ✗ 87654321 failed    answer another question\n\
               timing created 2m ago  flow single-worker  role worker-codex  next orchestrator jobs retry 87654321 --emit-retry-prompt  error {\"message\":\"model not found\"}\n",
        );

        assert_eq!(
            parse_jobs_header_counts("active: 0  shown: 2"),
            Some((0, 2))
        );
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].flow.as_deref(), Some("direct-answer"));
        assert_eq!(jobs[0].role.as_deref(), Some("worker-cc"));
        assert_eq!(
            job_state_summary(&jobs[0]),
            "complete; native session is available for handoff"
        );
        assert_eq!(jobs[1].error.as_deref(), Some("model not found"));

        let done_actions = job_actions(&jobs[0]);
        assert_eq!(
            done_actions.first().map(String::as_str),
            Some("/continue open worker-cc")
        );
        assert!(
            done_actions
                .iter()
                .any(|action| action == "/thread export worker-cc")
        );
        let failed_actions = job_actions(&jobs[1]);
        assert_eq!(
            failed_actions.first().map(String::as_str),
            Some("/jobs retry 87654321")
        );
        assert!(
            !failed_actions
                .iter()
                .any(|action| action.contains("orchestrator jobs retry"))
        );
    }
}
