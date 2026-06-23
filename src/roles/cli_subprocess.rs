//! CLI subprocess roles: spawn `claude` or `codex` CLI to do planning /
//! critique / review. This replaces the direct-LLM-API path (src/roles/llm.rs).
//!
//! Why: the orchestrator's direct Anthropic/OpenAI API calls go through a
//! third-party proxy (minimaxi) serving a small model (MiniMax-M3) that
//! frequently returns truncated/malformed JSON. Spawning `claude` CLI uses
//! the real Anthropic backend with CC's built-in retry, streaming, and
//! session management.
//!
//! Borrowed pattern from the existing `adapters/claude_code.rs` (the worker
//! adapter) — same subprocess machinery, simpler role-specific prompt.

use crate::agent_events;
use crate::core::types::{CritiqueOutput, PlanOutput, ReviewContext, ReviewOutput, Task};
use crate::pipeline::orchestrator::RunProgress;
use crate::roles::critic::Critic;
use crate::roles::planner::Planner;
use crate::roles::reviewer::Reviewer;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::{self, UnboundedSender};

// ── Shared helper ──────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleCliRuntime {
    ClaudeCode,
    Codex,
}

impl RoleCliRuntime {
    pub fn from_runtime_id(runtime: &str) -> Option<Self> {
        match runtime {
            "claude-code" | "claude" | "cc" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RoleCliSpec {
    pub runtime: RoleCliRuntime,
    pub binary: String,
    pub model: String,
    pub env: Vec<(String, String)>,
    pub config_overrides: Vec<(String, String)>,
    pub bypass_permissions: bool,
}

impl RoleCliSpec {
    pub fn new(
        runtime: RoleCliRuntime,
        binary: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            runtime,
            binary: binary.into(),
            model: model.into(),
            env: Vec::new(),
            config_overrides: Vec::new(),
            bypass_permissions: false,
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        let value = value.into();
        if !value.trim().is_empty() {
            self.env.retain(|(existing, _)| existing != &key);
            self.env.push((key, value));
        }
        self
    }

    pub fn with_config_override(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let key = key.into();
        let value = value.into();
        if !value.trim().is_empty() {
            self.config_overrides
                .retain(|(existing, _)| existing != &key);
            self.config_overrides.push((key, value));
        }
        self
    }

    pub fn with_bypass_permissions(mut self, bypass_permissions: bool) -> Self {
        self.bypass_permissions = bypass_permissions;
        self
    }
}

impl fmt::Debug for RoleCliSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env = self
            .env
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        let config_overrides = self
            .config_overrides
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        f.debug_struct("RoleCliSpec")
            .field("runtime", &self.runtime)
            .field("binary", &self.binary)
            .field("model", &self.model)
            .field("env", &env)
            .field("config_overrides", &config_overrides)
            .field("bypass_permissions", &self.bypass_permissions)
            .finish()
    }
}

/// Spawn `claude` (or any Anthropic-protocol CLI) and read its stdout.
/// Adds a timeout so a hung CLI doesn't block the orchestrator forever.
async fn run_cli(
    spec: &RoleCliSpec,
    role: &str,
    system_prompt: &str,
    user_prompt: &str,
    timeout_secs: u64,
    progress: Option<UnboundedSender<RunProgress>>,
) -> Result<String> {
    let mut cmd = role_cli_command(spec, system_prompt, user_prompt);

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "spawning {} (is it installed? check `which {}`)",
            spec.binary, spec.binary
        )
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("no stdout from {}", spec.binary))?;

    let (stderr_tx, mut stderr_rx) = mpsc::unbounded_channel::<String>();
    if let Some(stderr) = child.stderr.take() {
        let runtime = spec.runtime.label().to_string();
        let stderr_tx = stderr_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::warn!(
                    target = "role-cli-stderr",
                    runtime = runtime.as_str(),
                    "{}",
                    line
                );
                let _ = stderr_tx.send(line);
            }
        });
    }
    drop(stderr_tx);

    let starting = format!("{} starting model={}", spec.runtime.label(), spec.model);
    if !agent_events::is_low_value_output(&starting) {
        emit_role_output(&progress, role, starting);
    }

    let read = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        read_cli_stream(stdout, spec.runtime.label(), role, &progress).await
    })
    .await;

    match read {
        Ok(Ok(output)) => {
            let status = child
                .wait()
                .await
                .with_context(|| format!("waiting for {}", spec.binary))?;
            let stderr = collect_stderr_lines(&mut stderr_rx);
            if !status.success() {
                emit_actionable_stderr(&progress, role, spec.runtime.label(), &stderr);
                return Err(anyhow!(
                    "{} exited with status {}{}",
                    spec.binary,
                    status,
                    stderr_error_suffix(&stderr)
                ));
            }
            let finished = format!("{} finished", spec.runtime.label());
            if !agent_events::is_low_value_output(&finished) {
                emit_role_output(&progress, role, finished);
            }
            Ok(output)
        }
        Ok(Err(e)) => Err(anyhow!("reading {} stdout: {}", spec.binary, e)),
        Err(_) => {
            let _ = child.kill().await;
            Err(anyhow!(
                "{} timed out after {}s (model={})",
                spec.binary,
                timeout_secs,
                spec.model
            ))
        }
    }
}

fn collect_stderr_lines(stderr_rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Ok(line) = stderr_rx.try_recv() {
        lines.push(line);
    }
    lines
}

fn stderr_error_suffix(lines: &[String]) -> String {
    let joined = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.is_empty() {
        return String::new();
    }
    format!("; stderr: {}", truncate_cli_text(&joined, 500))
}

fn emit_actionable_stderr(
    progress: &Option<UnboundedSender<RunProgress>>,
    role: &str,
    runtime: &str,
    lines: &[String],
) {
    let joined = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.is_empty() {
        return;
    }
    emit_role_output(
        progress,
        role,
        format!("{} stderr: {}", runtime, truncate_cli_text(&joined, 500)),
    );
}

fn role_cli_command(spec: &RoleCliSpec, system_prompt: &str, user_prompt: &str) -> Command {
    let mut cmd = match spec.runtime {
        RoleCliRuntime::ClaudeCode => claude_role_command(
            &spec.binary,
            &spec.model,
            system_prompt,
            user_prompt,
            spec.bypass_permissions,
        ),
        RoleCliRuntime::Codex => codex_role_command(spec, system_prompt, user_prompt),
    };
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    cmd
}

fn claude_role_command(
    binary: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    bypass_permissions: bool,
) -> Command {
    let mut cmd = Command::new(binary);
    cmd.args(claude_role_args(
        model,
        system_prompt,
        user_prompt,
        bypass_permissions,
    ))
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    cmd
}

fn claude_role_args(
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    bypass_permissions: bool,
) -> Vec<String> {
    let mut args = vec![
        "--print".to_string(),
        "--setting-sources".to_string(),
        "project,local".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ];
    if bypass_permissions {
        args.push("--permission-mode".to_string());
        args.push("bypassPermissions".to_string());
    }
    args.extend([
        "--model".to_string(),
        model.to_string(),
        "--append-system-prompt".to_string(),
        system_prompt.to_string(),
        user_prompt.to_string(),
    ]);
    args
}

fn codex_role_command(spec: &RoleCliSpec, system_prompt: &str, user_prompt: &str) -> Command {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let prompt = format!(
        "System instructions:\n{}\n\nUser task:\n{}",
        system_prompt.trim(),
        user_prompt.trim()
    );
    let mut cmd = Command::new(&spec.binary);
    cmd.arg("exec")
        .arg("--json")
        .arg("--ignore-user-config")
        .arg("--model")
        .arg(&spec.model)
        .arg("--cd")
        .arg(cwd);
    for (key, value) in &spec.config_overrides {
        cmd.arg("-c")
            .arg(format!("{key}={}", toml_string_literal(value)));
    }
    cmd.arg(prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd.env("CODEX_UNSAFE_ALLOW_NO_GIT", "1");
    cmd
}

fn toml_string_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

async fn read_cli_stream(
    stdout: impl tokio::io::AsyncRead + Unpin,
    runtime_label: &str,
    role: &str,
    progress: &Option<UnboundedSender<RunProgress>>,
) -> std::io::Result<String> {
    let mut reader = BufReader::new(stdout).lines();
    let mut raw_lines = Vec::new();
    let mut text_chunks = Vec::new();
    let mut final_result = None;
    let controller_role = matches!(role, "planner" | "critic" | "reviewer");
    let mut controller_json_fragment_active = false;
    let mut suppressed_controller_lines = Vec::new();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        raw_lines.push(line.clone());
        if let Some(summary) = agent_events::summarize_cli_stream_line(&line, 260) {
            let suppress_raw_json_fragment = controller_role
                && summary.kind == "raw"
                && controller_json_fragment_should_be_suppressed(
                    &line,
                    &mut controller_json_fragment_active,
                );
            if suppress_raw_json_fragment {
                suppressed_controller_lines.push(line.clone());
            }
            let text = if matches!(role, "planner" | "critic" | "reviewer")
                || matches!(
                    summary.kind.as_str(),
                    "result" | "final" | "final_answer" | "task_complete" | "turn_complete"
                ) {
                agent_events::humanize_jsonish(&summary.text, 260)
            } else {
                summary.text
            };
            let display = format!("{} {}: {}", runtime_label, summary.kind, text);
            if !suppress_raw_json_fragment
                && !agent_events::is_low_value_output(&display)
                && !agent_events::is_redundant_role_output(role, &display, 260)
            {
                emit_role_output(progress, role, display);
            }
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                final_result = Some(result.to_string());
            }
            let text = agent_events::extract_text_from_value(&value).unwrap_or_default();
            if !text.trim().is_empty() {
                text_chunks.push(text);
            }
        }
    }

    if !suppressed_controller_lines.is_empty() {
        let display = agent_events::humanize_jsonish(&suppressed_controller_lines.join("\n"), 1200);
        if !display.trim().is_empty() && !agent_events::is_low_value_output(&display) {
            emit_role_output(progress, role, display);
        }
    }

    if let Some(result) = final_result {
        return Ok(result);
    }
    if !text_chunks.is_empty() {
        return Ok(text_chunks.join("\n"));
    }
    Ok(raw_lines.join("\n"))
}

fn controller_json_fragment_should_be_suppressed(line: &str, active: &mut bool) -> bool {
    let trimmed = line.trim_start();
    let starts_structured = looks_like_controller_json_fragment(trimmed);
    if *active {
        if controller_json_fragment_looks_complete(trimmed) {
            *active = false;
        }
        return true;
    }
    if starts_structured {
        *active = !controller_json_fragment_looks_complete(trimmed);
        return true;
    }
    false
}

fn looks_like_controller_json_fragment(trimmed: &str) -> bool {
    trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('}')
        || trimmed.starts_with(']')
        || trimmed.starts_with("\"approved\"")
        || trimmed.starts_with("\"issues\"")
        || trimmed.starts_with("\"suggestions\"")
        || trimmed.starts_with("\"sub_tasks\"")
        || trimmed.starts_with("\"prompt\"")
        || (trimmed.contains("\"approved\"") && trimmed.contains("\"issues\""))
}

fn controller_json_fragment_looks_complete(trimmed: &str) -> bool {
    let trimmed = trimmed.trim_end();
    trimmed.ends_with('}') || trimmed.ends_with(']')
}

fn emit_role_output(progress: &Option<UnboundedSender<RunProgress>>, role: &str, content: String) {
    if let Some(tx) = progress {
        let _ = tx.send(RunProgress::RoleOutput {
            role: role.to_string(),
            content,
        });
    }
}

fn truncate_cli_text(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (idx, c) in s.chars().enumerate() {
        if idx >= max {
            out.push('…');
            return out;
        }
        if c.is_control() && c != '\n' && c != '\t' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Lenient JSON extraction (finds first `{` and matching `}`).
/// Mirrors the same helper in src/roles/llm.rs but kept private to this
/// module so we don't need to expose it.
fn extract_json(content: &str) -> Result<serde_json::Value> {
    let trimmed = content.trim();

    // Try direct parse
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Ok(v);
    }

    // Find ```json ... ``` fence
    if let Some(start) = trimmed.find("```json") {
        let after = &trimmed[start + 7..];
        if let Some(end) = after.find("```") {
            let inner = after[..end].trim();
            if let Ok(v) = serde_json::from_str(inner) {
                return Ok(v);
            }
        }
    }

    // Find any ``` ... ``` fence
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let content_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let after_content = &after[content_start..];
        if let Some(end) = after_content.find("```") {
            let inner = after_content[..end].trim();
            if let Ok(v) = serde_json::from_str(inner) {
                return Ok(v);
            }
        }
    }

    // Find first `{` and brace-match
    if let Some(start) = trimmed.find('{') {
        let mut depth = 0i32;
        let mut end = None;
        for (i, c) in trimmed[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(end) = end {
            return serde_json::from_str(&trimmed[start..end]).map_err(|e| {
                anyhow!(
                    "extracted JSON parse failed: {} -- raw: {}",
                    e,
                    &trimmed[start..end]
                )
            });
        }
    }

    let preview: String = trimmed.chars().take(500).collect();
    Err(anyhow!(
        "no JSON found in CLI response (first 500 chars):\n{}",
        preview
    ))
}

// ── System prompts (concise; Claude handles verbose fine) ─────

const PLANNER_SYSTEM: &str =
    "You are a planning agent. Decompose a top-level task into 1-5 self-contained sub-tasks \
     that can run in parallel. Each sub-task must be unambiguous and executable in isolation. \
     Use tags to mark task type (e.g. refactor, test, boilerplate). \
     If the top-level task is already atomic, return a single sub-task with the same prompt. \
     Output a JSON object only — no prose, no markdown.";

const CRITIC_SYSTEM: &str =
    "You are an adversarial critic. Find problems with the proposed plan. \
     Be strict. Only approve if the plan is clear, complete, decomposable, and likely to succeed. \
     Look for: missing edge cases, ambiguous prompts, unstated dependencies, tasks requiring knowledge not available. \
     Output a JSON object only — no prose, no markdown.";

const REVIEWER_SYSTEM: &str =
    "You are a reviewer evaluating a worker's output. \
     First classify the task. If it is conversational, explanatory, diagnostic, or a greeting, \
     approve a useful textual answer even when there is no git diff and no code was changed. \
     For implementation tasks, approve only if the work is correct, complete, and meets the original prompt's intent. \
     For implementation tasks, look for: compilation/syntax errors, missing test coverage, incomplete implementations, logic errors. \
     Output a JSON object only — no prose, no markdown.";

const TIMEOUT_SECS: u64 = 180; // CLI startup + LLM call. 3 min is enough for
                               // large replans/critiques on slow proxies (minimaxi). User asked for longer
                               // after seeing 90s timeouts on replan attempts.

// ── Planner ────────────────────────────────────────────────

pub struct ClaudeCodePlanner {
    spec: RoleCliSpec,
}

impl ClaudeCodePlanner {
    pub fn from_spec(spec: RoleCliSpec) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl Planner for ClaudeCodePlanner {
    async fn plan(&self, top_task: &Task) -> Result<PlanOutput> {
        self.plan_impl(top_task, None).await
    }

    async fn plan_with_progress(
        &self,
        top_task: &Task,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<PlanOutput> {
        self.plan_impl(top_task, progress).await
    }

    async fn replan(
        &self,
        top_task: &Task,
        critique: &crate::core::types::CritiqueOutput,
    ) -> Result<PlanOutput> {
        self.replan_impl(top_task, critique, None).await
    }

    async fn replan_with_progress(
        &self,
        top_task: &Task,
        critique: &crate::core::types::CritiqueOutput,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<PlanOutput> {
        self.replan_impl(top_task, critique, progress).await
    }
}

impl ClaudeCodePlanner {
    async fn plan_impl(
        &self,
        top_task: &Task,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<PlanOutput> {
        let user_prompt = format!(
            "Top-level task: {}\n\n\
             Decompose into 1-5 sub-tasks. Output JSON with fields:\n\
             - sub_tasks: array of {{prompt: string, tags: array of strings}}\n\
             - rationale: one-sentence explanation\n\n\
             Output JSON only.",
            top_task.prompt
        );
        let raw = run_cli(
            &self.spec,
            "planner",
            PLANNER_SYSTEM,
            &user_prompt,
            TIMEOUT_SECS,
            progress,
        )
        .await?;
        tracing::info!(
            "[claude-planner] plan response (first 300 chars):\n  {}",
            raw.chars().take(300).collect::<String>()
        );
        let json = match extract_json(&raw) {
            Ok(json) => json,
            Err(err) if !looks_like_cli_hard_error(&raw) => {
                tracing::warn!(
                    "planner returned non-JSON output; falling back to one original task: {:#}",
                    err
                );
                return Ok(single_task_plan(
                    top_task,
                    "planner returned non-JSON output; using original task",
                ));
            }
            Err(err) => return Err(err),
        };
        match parse_plan(&json) {
            Ok(plan) => Ok(plan),
            Err(err) => {
                tracing::warn!(
                    "planner JSON did not contain usable worker runs; falling back to one original task: {:#}",
                    err
                );
                Ok(single_task_plan(
                    top_task,
                    "planner returned no usable worker runs; using original task",
                ))
            }
        }
    }

    async fn replan_impl(
        &self,
        top_task: &Task,
        critique: &crate::core::types::CritiqueOutput,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<PlanOutput> {
        let user_prompt = format!(
            "Top-level task: {}\n\n\
             Previous plan was rejected. Issues:\n- {}\n\n\
             Suggestions:\n- {}\n\n\
             Re-plan. Output JSON with fields:\n\
             - sub_tasks: array of {{prompt: string, tags: array of strings}}\n\
             - rationale: one-sentence explanation\n\n\
             Output JSON only.",
            top_task.prompt,
            critique.issues.join("\n- "),
            critique.suggestions.join("\n- "),
        );
        let raw = run_cli(
            &self.spec,
            "planner",
            PLANNER_SYSTEM,
            &user_prompt,
            TIMEOUT_SECS,
            progress,
        )
        .await?;
        let json = match extract_json(&raw) {
            Ok(json) => json,
            Err(err) if !looks_like_cli_hard_error(&raw) => {
                tracing::warn!(
                    "replanner returned non-JSON output; falling back to one original task: {:#}",
                    err
                );
                return Ok(single_task_plan(
                    top_task,
                    "replanner returned non-JSON output; using original task",
                ));
            }
            Err(err) => return Err(err),
        };
        match parse_plan(&json) {
            Ok(plan) => Ok(plan),
            Err(err) => {
                tracing::warn!(
                    "replanner JSON did not contain usable worker runs; falling back to one original task: {:#}",
                    err
                );
                Ok(single_task_plan(
                    top_task,
                    "replanner returned no usable worker runs; using original task",
                ))
            }
        }
    }
}

fn parse_plan(json: &serde_json::Value) -> Result<PlanOutput> {
    let rationale = json
        .get("rationale")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sub_tasks_json = json
        .get("sub_tasks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut sub_tasks = vec![];
    for st in &sub_tasks_json {
        let prompt = st
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if prompt.is_empty() {
            continue;
        }
        let tags: Vec<String> = st
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut t = Task::new(prompt);
        t.tags = tags;
        sub_tasks.push(t);
    }
    // Fallback: if no sub-tasks parsed, treat top task as one
    if sub_tasks.is_empty() {
        return Err(anyhow!("planner returned no worker runs (parse failed)"));
    }
    Ok(PlanOutput {
        sub_tasks,
        rationale,
        estimated_cost_usd: 0.0,
    })
}

fn looks_like_cli_hard_error(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.contains("api error:")
        || lower.contains("authentication")
        || lower.contains("unauthorized")
        || lower.contains("permission denied")
        || lower.contains("model not found")
        || raw.contains("模型不存在")
}

fn single_task_plan(top_task: &Task, rationale: impl Into<String>) -> PlanOutput {
    let mut task = Task::new(top_task.prompt.clone());
    task.tags = top_task.tags.clone();
    task.worktree = top_task.worktree.clone();
    task.agent_hint = top_task.agent_hint.clone();
    task.cc_agent_hint = top_task.cc_agent_hint.clone();
    PlanOutput {
        sub_tasks: vec![task],
        rationale: rationale.into(),
        estimated_cost_usd: 0.0,
    }
}

// ── Critic ─────────────────────────────────────────────────

pub struct ClaudeCodeCritic {
    spec: RoleCliSpec,
}

impl ClaudeCodeCritic {
    pub fn from_spec(spec: RoleCliSpec) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl Critic for ClaudeCodeCritic {
    async fn critique(&self, top_task: &Task, plan: &PlanOutput) -> Result<CritiqueOutput> {
        self.critique_impl(top_task, plan, None).await
    }

    async fn critique_with_progress(
        &self,
        top_task: &Task,
        plan: &PlanOutput,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<CritiqueOutput> {
        self.critique_impl(top_task, plan, progress).await
    }
}

impl ClaudeCodeCritic {
    async fn critique_impl(
        &self,
        top_task: &Task,
        plan: &PlanOutput,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<CritiqueOutput> {
        let plan_json = serde_json::to_string_pretty(&serde_json::json!({
            "rationale": plan.rationale,
            "sub_tasks": plan.sub_tasks.iter().map(|t| serde_json::json!({
                "prompt": t.prompt,
                "tags": t.tags,
            })).collect::<Vec<_>>(),
        }))?;
        let user_prompt = format!(
            "Top-level task: {}\n\nProposed plan:\n{}\n\n\
             Critique it. Output JSON with fields:\n\
             - approved: bool\n\
             - issues: array of strings (empty if approved)\n\
             - suggestions: array of strings\n\n\
             Output JSON only.",
            top_task.prompt, plan_json
        );
        let raw = run_cli(
            &self.spec,
            "critic",
            CRITIC_SYSTEM,
            &user_prompt,
            TIMEOUT_SECS,
            progress,
        )
        .await?;
        let json = extract_json(&raw)?;
        let approved = json
            .get("approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let issues: Vec<String> = json
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let suggestions: Vec<String> = json
            .get("suggestions")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(CritiqueOutput {
            approved,
            issues,
            suggestions,
        })
    }
}

// ── Reviewer ───────────────────────────────────────────────

pub struct ClaudeCodeReviewer {
    spec: RoleCliSpec,
}

impl ClaudeCodeReviewer {
    pub fn from_spec(spec: RoleCliSpec) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl Reviewer for ClaudeCodeReviewer {
    async fn review(&self, task: &Task, ctx: &ReviewContext) -> Result<ReviewOutput> {
        self.review_impl(task, ctx, None).await
    }

    async fn review_with_progress(
        &self,
        task: &Task,
        ctx: &ReviewContext,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<ReviewOutput> {
        self.review_impl(task, ctx, progress).await
    }
}

impl ClaudeCodeReviewer {
    async fn review_impl(
        &self,
        task: &Task,
        ctx: &ReviewContext,
        progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<ReviewOutput> {
        let worker_text = extract_worker_text(&ctx.session_log_path)
            .unwrap_or_else(|err| format!("(worker output unavailable: {err})"));
        let worker_text = truncate_review_text(&worker_text, 8_000);
        let diff = read_diff_capped(&ctx.worktree_path, 8_000);
        let diff = if diff.trim().is_empty() {
            "(no file changes; this is acceptable for conversational, diagnostic, or explanation tasks)".to_string()
        } else {
            diff
        };
        let user_prompt = format!(
            "Task that was attempted:\n{}\n\n\
             Worker output:\n```\n{}\n```\n\n\
             Git diff:\n```diff\n{}\n```\n\n\
             Review against the user's intent. For conversational questions, greetings, explanations, or diagnostics, \
             approve a useful textual answer even when there is no diff. Only reject if the output is wrong, incomplete, \
             unsafe, or clearly ignores the task.\n\n\
             Output JSON with fields:\n\
             - approved: bool\n\
             - issues: array of strings\n\n\
             Output JSON only.",
            task.prompt, worker_text, diff
        );
        let raw = run_cli(
            &self.spec,
            "reviewer",
            REVIEWER_SYSTEM,
            &user_prompt,
            TIMEOUT_SECS,
            progress,
        )
        .await?;
        let json = extract_json(&raw)?;
        let approved = json
            .get("approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let issues: Vec<String> = json
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(ReviewOutput { approved, issues })
    }
}

fn extract_worker_text(path: &std::path::Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading worker session log {}", path.display()))?;
    let mut out = String::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                out.push_str(line);
                out.push('\n');
                continue;
            }
        };
        append_event_text(&mut out, &value);
    }
    if out.trim().is_empty() {
        return Err(anyhow!("no worker text found"));
    }
    Ok(out)
}

fn append_event_text(out: &mut String, value: &serde_json::Value) {
    if let Some(text) = value.as_str() {
        out.push_str(text);
        out.push('\n');
        return;
    }

    if let Some(text) = value
        .get("result")
        .or_else(|| value.get("text"))
        .or_else(|| value.get("line"))
        .and_then(|v| v.as_str())
    {
        out.push_str(text);
        out.push('\n');
    }

    if let Some(arr) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    out.push_str(text);
                    out.push('\n');
                }
            }
        }
    }

    if let Some(obj) = value.as_object() {
        for key in ["payload", "event", "message"] {
            if let Some(nested) = obj.get(key) {
                if !nested.is_string() {
                    append_event_text(out, nested);
                }
            }
        }
    }
}

fn read_diff_capped(worktree: &std::path::Path, cap: usize) -> String {
    let output = std::process::Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(worktree)
        .output();
    let raw = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        _ => return String::new(),
    };
    truncate_review_text(&raw, cap)
}

fn truncate_review_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…(truncated)", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_claude_stream_json_text_blocks() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "planning step one" }
                ]
            }
        })
        .to_string();

        let summary = agent_events::summarize_cli_stream_line(&line, 200).unwrap();

        assert_eq!(summary.kind, "assistant");
        assert_eq!(summary.text, "planning step one");
    }

    #[test]
    fn extracts_final_result_from_stream_line() {
        let line = serde_json::json!({
            "type": "result",
            "result": "{\"approved\":true,\"issues\":[]}"
        })
        .to_string();

        let summary = agent_events::summarize_cli_stream_line(&line, 200).unwrap();

        assert_eq!(summary.kind, "result");
        assert_eq!(summary.text, "{\"approved\":true,\"issues\":[]}");
    }

    #[test]
    fn reviewer_context_extracts_worker_text_from_session_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "content": [
                            { "type": "text", "text": "你好，我可以帮你写代码。" }
                        ]
                    }
                }),
                serde_json::json!({
                    "type": "result",
                    "result": "最终回答"
                }),
                "plain fallback line"
            ),
        )
        .unwrap();

        let text = extract_worker_text(&path).unwrap();

        assert!(text.contains("你好，我可以帮你写代码。"));
        assert!(text.contains("最终回答"));
        assert!(text.contains("plain fallback line"));
    }

    #[test]
    fn reviewer_context_truncates_on_char_boundary() {
        let text = truncate_review_text("你好abcdef", 7);

        assert_eq!(text, "你好a…(truncated)");
    }

    #[test]
    fn hard_cli_error_detection_catches_model_errors() {
        assert!(looks_like_cli_hard_error(
            "API Error: 400 [1211][模型不存在，请检查模型代码。]"
        ));
        assert!(looks_like_cli_hard_error("authentication failed"));
        assert!(!looks_like_cli_hard_error(
            "I cannot produce JSON, but the task is simple."
        ));
    }

    #[test]
    fn stderr_error_suffix_keeps_actionable_cli_error() {
        let suffix = stderr_error_suffix(&[
            "".to_string(),
            "error: unexpected argument '--cwd' found".to_string(),
            "Usage: codex exec --cd <DIR> [PROMPT]".to_string(),
        ]);

        assert!(suffix.contains("stderr: error: unexpected argument '--cwd' found"));
        assert!(suffix.contains("codex exec --cd"));
    }

    #[test]
    fn controller_json_fragments_are_suppressed_until_summarized() {
        let mut active = false;

        assert!(controller_json_fragment_should_be_suppressed(
            r#"{"approved": false, "issues": ["missing"#,
            &mut active
        ));
        assert!(active);
        assert!(controller_json_fragment_should_be_suppressed(
            r#"test"]}"#,
            &mut active
        ));
        assert!(!active);
        assert!(!controller_json_fragment_should_be_suppressed(
            "ordinary assistant text",
            &mut active
        ));
    }

    #[test]
    fn controller_json_fragment_detector_covers_all_controller_shapes() {
        for line in [
            r#"{"sub_tasks":["#,
            r#""approved": false,"#,
            r#""issues": ["missing"],"#,
            r#""suggestions": []"#,
            r#""prompt": "do work""#,
        ] {
            assert!(looks_like_controller_json_fragment(line), "{line}");
        }
        assert!(!looks_like_controller_json_fragment("normal worker text"));
    }

    #[test]
    fn single_task_plan_preserves_original_task_context() {
        let mut top = Task::new("answer directly");
        top.tags = vec!["chat".to_string()];
        top.agent_hint = Some("worker-cc".to_string());
        top.worktree = Some(PathBuf::from("/tmp/project"));

        let plan = single_task_plan(&top, "fallback");

        assert_eq!(plan.sub_tasks.len(), 1);
        assert_eq!(plan.sub_tasks[0].prompt, "answer directly");
        assert_eq!(plan.sub_tasks[0].tags, vec!["chat".to_string()]);
        assert_eq!(plan.sub_tasks[0].agent_hint.as_deref(), Some("worker-cc"));
        assert_eq!(
            plan.sub_tasks[0].worktree.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
    }

    #[test]
    fn claude_role_args_ignore_user_settings_for_provider_isolation() {
        let args = claude_role_args("MiniMax-M3", "system", "hi", false);

        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--setting-sources" && pair[1] == "project,local"));
        assert!(!args.iter().any(|arg| arg == "user"));
    }

    #[test]
    fn claude_role_args_can_bypass_permissions_for_noninteractive_runs() {
        let args = claude_role_args("MiniMax-M3", "system", "hi", true);

        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--permission-mode" && pair[1] == "bypassPermissions"));
    }

    #[test]
    fn codex_role_command_uses_current_workdir_flag() {
        let spec = RoleCliSpec::new(RoleCliRuntime::Codex, "codex", "gpt-5.1");
        let cmd = codex_role_command(&spec, "system", "user");
        let args = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.iter().any(|arg| arg == "--cd"));
        assert!(!args.iter().any(|arg| arg == "--cwd"));
    }
}
