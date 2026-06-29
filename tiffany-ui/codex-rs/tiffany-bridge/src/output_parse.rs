use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleProfileRow {
    pub role: String,
    pub model: String,
    pub runtime: String,
    pub agent_teams: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleSummary {
    pub name: String,
    pub model: String,
    pub display_model: Option<String>,
    pub provider: Option<String>,
    pub api_model: Option<String>,
    pub runtime: String,
    pub teams: bool,
    pub health: Option<String>,
    pub thread: Option<String>,
    pub native: Option<String>,
    pub last: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleOptionSummary {
    pub model: String,
    pub provider: String,
    pub api_model: String,
    pub runtimes: String,
    pub teams: String,
    pub roles: String,
    pub health: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSummary {
    pub name: String,
    pub kind: String,
    pub auth: String,
    pub endpoint: String,
    pub models: Option<String>,
    pub roles: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadSummary {
    pub role: String,
    pub active: bool,
    pub runtime: String,
    pub model: String,
    pub thread: Option<String>,
    pub native: Option<String>,
    pub last: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCliHandoff {
    pub role: String,
    pub runtime: Option<String>,
    pub worker_thread_id: Option<String>,
    pub native_session: String,
    pub command: String,
    pub worktree: Option<String>,
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

pub fn parse_provider_summaries(text: &str) -> Vec<ProviderSummary> {
    if let Some(provider) = parse_provider_detail_summary(text) {
        return vec![provider];
    }

    let mut providers = Vec::new();
    let mut in_config_providers = false;
    let mut in_missing_providers = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("Missing providers referenced by models:") {
            in_config_providers = false;
            in_missing_providers = true;
            continue;
        }
        if in_missing_providers {
            if trimmed.starts_with("Actions:")
                || trimmed.starts_with("Health:")
                || trimmed.starts_with("Providers:")
                || trimmed.starts_with("Provider ")
            {
                in_missing_providers = false;
            } else if let Some(provider) = parse_missing_provider_row(trimmed) {
                providers.push(provider);
                continue;
            } else {
                continue;
            }
        }
        if trimmed.starts_with("Providers (") && trimmed.ends_with(':') {
            in_config_providers = true;
            continue;
        }
        if in_config_providers {
            if trimmed.starts_with("Models (")
                || trimmed.starts_with("Roles (")
                || trimmed.starts_with("Tag overrides")
                || trimmed.starts_with("───")
            {
                in_config_providers = false;
                continue;
            }
            if let Some(provider) = parse_config_show_provider(trimmed) {
                providers.push(provider);
            }
            continue;
        }
        if let Some(provider) = parse_provider_registry_row(trimmed) {
            providers.push(provider);
            continue;
        }
        if let Some(provider) = parse_provider_table_row(trimmed) {
            providers.push(provider);
        }
    }

    providers
}

pub fn parse_role_summaries(text: &str) -> Vec<RoleSummary> {
    text.lines()
        .filter_map(parse_role_summary_line)
        .collect::<Vec<_>>()
}

pub fn parse_role_option_summaries(text: &str) -> Vec<RoleOptionSummary> {
    text.lines()
        .filter_map(parse_role_option_summary_line)
        .collect::<Vec<_>>()
}

pub fn parse_thread_list_summaries(text: &str) -> Vec<ThreadSummary> {
    text.lines()
        .filter_map(parse_thread_list_line)
        .collect::<Vec<_>>()
}

pub fn parse_thread_fields(text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            fields.insert(key, value.to_string());
        }
    }
    fields
}

pub fn parse_worker_thread_title(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Worker thread "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn parse_native_cli_handoff(text: &str) -> Option<NativeCliHandoff> {
    if !text.contains("Worker thread") {
        return None;
    }
    let fields = parse_thread_fields(text);
    let role = fields
        .get("role")
        .cloned()
        .or_else(|| parse_worker_thread_title(text))
        .unwrap_or_else(|| "worker".to_string());
    let native_session = fields
        .get("native session")
        .and_then(|value| nonempty_trimmed(value))
        .filter(|value| *value != "none")?
        .to_string();
    let command = fields
        .get("native handoff")
        .and_then(|value| nonempty_trimmed(value))
        .or_else(|| {
            fields
                .get("native resume")
                .and_then(|value| nonempty_trimmed(value))
        })
        .filter(|value| *value != "none")?
        .to_string();
    let worktree = fields
        .get("worktree")
        .and_then(|value| nonempty_trimmed(value))
        .filter(|value| *value != "none")
        .map(ToString::to_string);

    Some(NativeCliHandoff {
        role,
        runtime: fields
            .get("runtime")
            .and_then(|value| nonempty_trimmed(value))
            .map(ToString::to_string),
        worker_thread_id: fields
            .get("tiffany thread")
            .and_then(|value| nonempty_trimmed(value))
            .map(ToString::to_string),
        native_session,
        command,
        worktree,
    })
}

fn parse_provider_detail_summary(text: &str) -> Option<ProviderSummary> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let header = lines.next()?;
    let name = header
        .strip_prefix("Provider ")
        .filter(|name| !name.eq_ignore_ascii_case("registry"))
        .filter(|name| !matches!(*name, "setup" | "delete"))?
        .trim();
    if name.is_empty() || name.contains(':') {
        return None;
    }

    let mut kind = None;
    let mut auth = None;
    let mut endpoint = None;
    let mut models = None;
    let mut roles = None;

    for line in lines {
        if let Some(value) = colon_value(line, "type") {
            kind = Some(value.to_string());
        } else if let Some(value) = colon_value(line, "auth") {
            auth = Some(value.to_string());
        } else if let Some(value) = colon_value(line, "endpoint") {
            endpoint = Some(value.to_string());
        } else if let Some(value) = colon_value(line, "models") {
            models = Some(value.to_string());
        } else if let Some(value) = colon_value(line, "roles") {
            roles = Some(value.to_string());
        }
    }

    Some(ProviderSummary {
        name: name.to_string(),
        kind: kind?,
        auth: auth.unwrap_or_else(|| "-".to_string()),
        endpoint: endpoint.unwrap_or_else(|| "-".to_string()),
        models,
        roles,
    })
}

fn colon_value<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    if key.trim() == label {
        Some(value.trim())
    } else {
        None
    }
}

fn parse_missing_provider_row(line: &str) -> Option<ProviderSummary> {
    let name = line.strip_prefix('⚠')?.trim();
    if name.is_empty() {
        return None;
    }
    Some(ProviderSummary {
        name: name.to_string(),
        kind: "missing".to_string(),
        auth: "missing".to_string(),
        endpoint: "-".to_string(),
        models: None,
        roles: None,
    })
}

fn parse_provider_registry_row(line: &str) -> Option<ProviderSummary> {
    let rest = line
        .strip_prefix('✓')
        .or_else(|| line.strip_prefix('⚠'))
        .or_else(|| line.strip_prefix('●'))?
        .trim();
    let parts = rest.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }

    let mut auth = None;
    let mut endpoint = None;
    let mut models = None;
    let mut roles = None;
    for part in parts.iter().skip(2) {
        if let Some(value) = part.strip_prefix("auth=") {
            auth = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("endpoint=") {
            endpoint = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("models=") {
            models = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("roles=") {
            roles = Some(value.to_string());
        }
    }

    Some(ProviderSummary {
        name: parts[0].to_string(),
        kind: parts[1].to_string(),
        auth: auth.unwrap_or_else(|| "-".to_string()),
        endpoint: endpoint.unwrap_or_else(|| "-".to_string()),
        models,
        roles,
    })
}

fn parse_provider_table_row(line: &str) -> Option<ProviderSummary> {
    if line.starts_with("Providers ")
        || line.starts_with("Provider registry")
        || line.starts_with("Provider ")
        || line.starts_with("provider ")
        || line.starts_with("configured:")
        || line.starts_with("Providers:")
        || line.starts_with("Model bindings:")
        || line.starts_with("Missing providers")
        || line.starts_with("Actions:")
        || line.starts_with("Health:")
        || line.starts_with("===")
        || line.starts_with("config file:")
        || line.starts_with("No providers configured")
        || line.starts_with("Create one with:")
    {
        return None;
    }
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    if parts[0] == "-" {
        return None;
    }
    Some(ProviderSummary {
        name: parts[0].to_string(),
        kind: parts[1].to_string(),
        auth: parts[2].to_string(),
        endpoint: parts[3].to_string(),
        models: parts.get(4).map(|value| (*value).to_string()),
        roles: parts.get(5).map(|value| (*value).to_string()),
    })
}

fn parse_config_show_provider(line: &str) -> Option<ProviderSummary> {
    let line = line.strip_prefix("- ")?;
    let parts = line.split_whitespace().collect::<Vec<_>>();
    let name = parts.first()?.to_string();
    let kind = parts
        .iter()
        .find_map(|part| part.strip_prefix("type="))
        .unwrap_or("unknown")
        .to_string();
    let auth = parts
        .iter()
        .position(|part| part.starts_with("api_key="))
        .map(|idx| {
            let first = parts[idx].trim_start_matches("api_key=");
            if first == "✓" && parts.get(idx + 1) == Some(&"set") {
                "set".to_string()
            } else {
                first.to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string());
    let endpoint = parts
        .iter()
        .find_map(|part| {
            part.strip_prefix("base_url=")
                .or_else(|| part.strip_prefix("endpoint="))
        })
        .unwrap_or("-")
        .to_string();
    Some(ProviderSummary {
        name,
        kind,
        auth,
        endpoint,
        models: parts
            .iter()
            .find_map(|part| part.strip_prefix("models="))
            .map(ToString::to_string),
        roles: parts
            .iter()
            .find_map(|part| part.strip_prefix("roles="))
            .map(ToString::to_string),
    })
}

fn parse_role_option_summary_line(line: &str) -> Option<RoleOptionSummary> {
    let trimmed = line.trim();
    let trimmed = trimmed
        .strip_prefix('✓')
        .or_else(|| trimmed.strip_prefix('⚠'))?
        .trim();
    let mut parts = trimmed.split_whitespace();
    let model = parts.next()?.to_string();
    let mut provider = None;
    let mut api_model = None;
    let mut runtimes = None;
    let mut teams = None;
    let mut roles = None;
    let mut health = None;
    for part in parts {
        if let Some(value) = part.strip_prefix("provider=") {
            provider = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("api_model=") {
            api_model = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("runtimes=") {
            runtimes = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("teams=") {
            teams = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("roles=") {
            roles = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("health=") {
            health = Some(value.to_string());
        }
    }
    Some(RoleOptionSummary {
        model,
        provider: provider?,
        api_model: api_model?,
        runtimes: runtimes.unwrap_or_else(|| "-".to_string()),
        teams: teams.unwrap_or_else(|| "off".to_string()),
        roles: roles.unwrap_or_else(|| "-".to_string()),
        health: health.unwrap_or_else(|| "unknown".to_string()),
    })
}

fn parse_role_summary_line(line: &str) -> Option<RoleSummary> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("Registered roles")
        || trimmed.starts_with("Register:")
        || trimmed.starts_with("Roles (")
    {
        return None;
    }
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let normalized = trimmed.replace('→', " ");
    let parts = normalized.split_whitespace().collect::<Vec<_>>();
    let name = parts.first()?.to_string();
    let model = parts
        .iter()
        .find_map(|part| part.strip_prefix("model="))?
        .to_string();
    let display_model = parts
        .iter()
        .find_map(|part| {
            part.strip_prefix('(')
                .and_then(|value| value.strip_suffix(')'))
        })
        .map(ToString::to_string);
    let runtime = parts
        .iter()
        .find_map(|part| part.strip_prefix("runtime="))
        .unwrap_or("runtime")
        .to_string();
    let teams = parts
        .iter()
        .any(|part| *part == "[agent_teams]" || part.strip_prefix("teams=") == Some("true"));

    Some(RoleSummary {
        name,
        model,
        display_model,
        provider: parts
            .iter()
            .find_map(|part| part.strip_prefix("provider="))
            .map(ToString::to_string),
        api_model: parts
            .iter()
            .find_map(|part| {
                part.strip_prefix("api_model=")
                    .or_else(|| part.strip_prefix("model_name="))
            })
            .map(ToString::to_string),
        runtime,
        teams,
        health: parts
            .iter()
            .find_map(|part| part.strip_prefix("health="))
            .map(ToString::to_string),
        thread: parts
            .iter()
            .find_map(|part| part.strip_prefix("thread="))
            .map(ToString::to_string),
        native: parts
            .iter()
            .find_map(|part| part.strip_prefix("native="))
            .map(ToString::to_string),
        last: parts
            .iter()
            .find_map(|part| part.strip_prefix("last="))
            .map(ToString::to_string),
    })
}

fn parse_thread_list_line(line: &str) -> Option<ThreadSummary> {
    let trimmed = line.trim();
    let active = if let Some(rest) = trimmed.strip_prefix("● ") {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("○ ") {
        (false, rest)
    } else {
        return None;
    };
    let (active, rest) = active;
    let parts = rest.split_whitespace().collect::<Vec<_>>();
    let role = parts.first()?.to_string();
    let joined = parts.get(1..).unwrap_or(&[]).join(" ");
    if !active {
        let detail = joined
            .split(" · ")
            .next()
            .unwrap_or("not configured")
            .trim()
            .to_string();
        return Some(ThreadSummary {
            role,
            active,
            runtime: detail,
            model: String::new(),
            thread: None,
            native: None,
            last: None,
        });
    }

    let segments = joined.split(" · ").map(str::trim).collect::<Vec<_>>();
    let runtime = segments.first().copied().unwrap_or("runtime").to_string();
    let model = segments.get(1).copied().unwrap_or("model").to_string();
    Some(ThreadSummary {
        role,
        active,
        runtime,
        model,
        thread: segment_value(&segments, "thread"),
        native: segment_value(&segments, "native"),
        last: segment_value(&segments, "last"),
    })
}

fn segment_value(segments: &[&str], prefix: &str) -> Option<String> {
    let needle = format!("{prefix} ");
    segments
        .iter()
        .find_map(|segment| segment.strip_prefix(&needle))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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

pub fn job_repair_hint(job: &JobSummary) -> Option<String> {
    let error = job.error.as_deref().and_then(nonempty_trimmed)?;
    let error_lower = error.to_ascii_lowercase();
    let role_command = job
        .role
        .as_deref()
        .and_then(nonempty_trimmed)
        .map(|role| format!("/role {role}"))
        .unwrap_or_else(|| "/roles".to_string());

    if contains_any(
        &error_lower,
        &[
            "model not found",
            "model does not exist",
            "invalid model",
            "model unavailable",
            "unknown model",
        ],
    ) || error.contains("模型不存在")
    {
        return Some(format!(
            "check model binding with {role_command}, then run /doctor"
        ));
    }

    if contains_any(
        &error_lower,
        &[
            "api key",
            "apikey",
            "unauthorized",
            "forbidden",
            "authentication",
            "permission denied",
            "invalid key",
        ],
    ) {
        return Some("check provider auth with /provider, then run /doctor".to_string());
    }

    if contains_any(
        &error_lower,
        &[
            "endpoint",
            "base url",
            "base_url",
            "connection refused",
            "dns",
            "network timeout",
            "timeout",
        ],
    ) {
        return Some(
            "check provider endpoint/network with /provider, then run /doctor".to_string(),
        );
    }

    if contains_any(
        &error_lower,
        &[
            "runtime",
            "binary",
            "executable",
            "command not found",
            "no such file or directory",
        ],
    ) {
        return Some("check runtime binary with /doctor, then review /roles".to_string());
    }

    if contains_any(&error_lower, &["rate limit", "429", "quota"]) {
        return Some(format!(
            "provider is rate limited; retry later or switch model with {role_command}"
        ));
    }

    None
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
            "error" => {
                job.error = Some(tiffany_event_format::humanize_agent_status_text(value, 220))
            }
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

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
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
    fn parses_provider_outputs() {
        let providers = parse_provider_summaries(
            "Providers (2):\n\
             - anthropic type=anthropic api_key=✓ set base_url=https://api.anthropic.com models=2 roles=3\n\
             - minimax type=openai-compatible api_key=MINIMAX_API_KEY endpoint=https://api.minimax.io models=1 roles=1\n\
             Models (3):\n",
        );

        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].name, "anthropic");
        assert_eq!(providers[0].kind, "anthropic");
        assert_eq!(providers[0].auth, "set");
        assert_eq!(providers[0].endpoint, "https://api.anthropic.com");
        assert_eq!(providers[0].models.as_deref(), Some("2"));
        assert_eq!(providers[0].roles.as_deref(), Some("3"));
        assert_eq!(providers[1].auth, "MINIMAX_API_KEY");

        let detail = parse_provider_summaries(
            "Provider anthropic\n\
             type: anthropic\n\
             auth: ANTHROPIC_API_KEY\n\
             endpoint: https://api.anthropic.com\n\
             models: 2\n\
             roles: planner,worker-cc\n",
        );
        assert_eq!(detail.len(), 1);
        assert_eq!(detail[0].name, "anthropic");
        assert_eq!(detail[0].roles.as_deref(), Some("planner,worker-cc"));
    }

    #[test]
    fn parses_role_outputs() {
        let roles = parse_role_summaries(
            "Registered roles\n\
             planner -> model=sonnet (claude-sonnet-4-6) provider=anthropic api_model=claude-sonnet-4-6 runtime=claude-code health=ready thread=thread-1 native=native-1 last=session-1\n\
             worker-cc -> model=minimax-m3 provider=minimax model_name=MiniMax-M3 runtime=claude-code teams=true health=missing-key\n",
        );

        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0].name, "planner");
        assert_eq!(roles[0].display_model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(roles[0].thread.as_deref(), Some("thread-1"));
        assert_eq!(roles[1].api_model.as_deref(), Some("MiniMax-M3"));
        assert!(roles[1].teams);
        assert_eq!(roles[1].health.as_deref(), Some("missing-key"));

        let options = parse_role_option_summaries(
            "Role options\n\
             ✓ sonnet provider=anthropic api_model=claude-sonnet-4-6 runtimes=claude-code teams=on roles=planner,reviewer health=ready\n\
             ⚠ minimax-m3 provider=minimax api_model=MiniMax-M3 roles=worker-cc health=missing-key\n",
        );
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].teams, "on");
        assert_eq!(options[1].runtimes, "-");
        assert_eq!(options[1].health, "missing-key");
    }

    #[test]
    fn parses_thread_outputs() {
        let threads = parse_thread_list_summaries(
            "Worker threads\n\
             Roles:\n\
               ○ worker-cc          no worker thread yet\n\
               ● worker-codex       codex · openai/gpt-4o · thread thread-1 · native native-1 · last session-1\n",
        );

        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].role, "worker-cc");
        assert!(!threads[0].active);
        assert_eq!(threads[0].runtime, "no worker thread yet");
        assert!(threads[1].active);
        assert_eq!(threads[1].runtime, "codex");
        assert_eq!(threads[1].model, "openai/gpt-4o");
        assert_eq!(threads[1].thread.as_deref(), Some("thread-1"));
        assert_eq!(threads[1].native.as_deref(), Some("native-1"));
        assert_eq!(threads[1].last.as_deref(), Some("session-1"));

        let detail = "Worker thread thread-1\n\
             role: worker-codex\n\
             runtime: codex\n\
             native session: native-1\n\
             native handoff: cd /tmp/project && codex resume native-1\n";
        let fields = parse_thread_fields(detail);
        assert_eq!(
            parse_worker_thread_title(detail).as_deref(),
            Some("thread-1")
        );
        assert_eq!(fields.get("role").map(String::as_str), Some("worker-codex"));
        assert_eq!(
            fields.get("native handoff").map(String::as_str),
            Some("cd /tmp/project && codex resume native-1")
        );
    }

    #[test]
    fn parses_native_cli_handoff_from_thread_output() {
        let handoff = parse_native_cli_handoff(
            "Worker thread thread-title\n\
             role: worker-codex\n\
             runtime: codex\n\
             Tiffany thread: tiffany-thread-1\n\
             native session: native-1\n\
             native resume: codex exec resume native-1\n\
             native handoff: cd /tmp/project && codex resume native-1\n\
             worktree: /tmp/project\n",
        )
        .expect("handoff");

        assert_eq!(handoff.role, "worker-codex");
        assert_eq!(handoff.runtime.as_deref(), Some("codex"));
        assert_eq!(
            handoff.worker_thread_id.as_deref(),
            Some("tiffany-thread-1")
        );
        assert_eq!(handoff.native_session, "native-1");
        assert_eq!(handoff.command, "cd /tmp/project && codex resume native-1");
        assert_eq!(handoff.worktree.as_deref(), Some("/tmp/project"));
        assert!(!handoff.command.contains("codex exec resume"));

        let fallback = parse_native_cli_handoff(
            "Worker thread worker-gemini\n\
             runtime: gemini\n\
             native session: latest\n\
             native resume: gemini --resume latest\n",
        )
        .expect("resume fallback");
        assert_eq!(fallback.role, "worker-gemini");
        assert_eq!(fallback.command, "gemini --resume latest");

        assert!(
            parse_native_cli_handoff(
                "Worker thread worker-cc\n\
                 native session: none\n\
                 native handoff: none\n",
            )
            .is_none()
        );
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

        let model_error = parse_job_summaries(
            "Jobs\n\
               active: 0  shown: 1\n\
             \n\
             ✗ abcd1234 failed    queued follow-up\n\
               timing created 1m ago  flow single-worker  role worker-codex  error {\"message\":\"[1211][模型不存在] invalid model\"}\n",
        );
        assert_eq!(
            model_error[0].error.as_deref(),
            Some("model not found: 模型不存在 (1211)")
        );
    }

    #[test]
    fn job_repair_hints_cover_common_provider_model_runtime_failures() {
        let job = |error: &str, role: Option<&str>| JobSummary {
            id: "abcd1234".to_string(),
            status: "failed".to_string(),
            prompt: "prompt".to_string(),
            flow: Some("single-worker".to_string()),
            role: role.map(str::to_string),
            task: None,
            timing: None,
            session: None,
            thread: None,
            native: None,
            history: None,
            next: None,
            result: None,
            error: Some(error.to_string()),
        };

        assert_eq!(
            job_repair_hint(&job(
                "[1211][模型不存在] invalid model",
                Some("worker-codex")
            ))
            .as_deref(),
            Some("check model binding with /role worker-codex, then run /doctor")
        );
        assert_eq!(
            job_repair_hint(&job("401 unauthorized: invalid API key", None)).as_deref(),
            Some("check provider auth with /provider, then run /doctor")
        );
        assert_eq!(
            job_repair_hint(&job("network timeout while calling endpoint", None)).as_deref(),
            Some("check provider endpoint/network with /provider, then run /doctor")
        );
        assert_eq!(
            job_repair_hint(&job("runtime binary command not found", Some("worker-cc"))).as_deref(),
            Some("check runtime binary with /doctor, then review /roles")
        );
        assert_eq!(
            job_repair_hint(&job("429 rate limit exceeded", Some("worker-gemini"))).as_deref(),
            Some("provider is rate limited; retry later or switch model with /role worker-gemini")
        );
        assert_eq!(
            job_repair_hint(&job("worker returned a bad answer", Some("worker-cc"))),
            None
        );
    }
}
