//! Shared event parsing for Claude Code, Codex CLI, ACP, and TUI surfaces.

use serde_json::{Map, Value};

const TOOL_DETAIL_MAX_CHARS: usize = 220;
const TOOL_RESULT_MAX_CHARS: usize = 240_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEventSummary {
    pub kind: String,
    pub text: String,
    pub tool_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibleAgentOutputKind {
    Final,
    Answer,
    Question,
    Approval,
    ToolCall,
    ToolResult,
    Diff,
    Patch,
    FileUpdate,
    Stderr,
    Actionable,
    Normal,
}

impl VisibleAgentOutputKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Final => "final",
            Self::Answer => "answer",
            Self::Question => "question",
            Self::Approval => "approval",
            Self::ToolCall => "tool call",
            Self::ToolResult => "tool result",
            Self::Diff => "diff",
            Self::Patch => "patch",
            Self::FileUpdate => "file update",
            Self::Stderr => "stderr",
            Self::Actionable => "alert",
            Self::Normal => "output",
        }
    }

    pub fn is_process_event(self) -> bool {
        matches!(
            self,
            Self::Approval
                | Self::ToolCall
                | Self::ToolResult
                | Self::Diff
                | Self::Patch
                | Self::FileUpdate
        )
    }

    fn is_artifact_event(self) -> bool {
        matches!(self, Self::Diff | Self::Patch | Self::FileUpdate)
    }
}

pub fn visible_agent_output_kind_for_event_kind(
    event_kind: &str,
) -> Option<VisibleAgentOutputKind> {
    match normalize_event_kind(event_kind)?.as_str() {
        "stderr" => Some(VisibleAgentOutputKind::Stderr),
        "approval"
        | "permission_request"
        | "command_approval"
        | "patch_approval"
        | "guardian_assessment" => Some(VisibleAgentOutputKind::Approval),
        "tool"
        | "tool_use"
        | "exec"
        | "local_shell_call"
        | "function_call"
        | "custom_tool_call"
        | "tool_search_call"
        | "web_search_call"
        | "image_generation_call"
        | "mcp_tool_call" => Some(VisibleAgentOutputKind::ToolCall),
        "tool_result"
        | "function_call_output"
        | "custom_tool_call_output"
        | "tool_search_output" => Some(VisibleAgentOutputKind::ToolResult),
        "diff" => Some(VisibleAgentOutputKind::Diff),
        "patch" => Some(VisibleAgentOutputKind::Patch),
        "file_change" | "file_update" => Some(VisibleAgentOutputKind::FileUpdate),
        "result" | "final" | "final_answer" | "task_complete" | "turn_complete" => {
            Some(VisibleAgentOutputKind::Final)
        }
        "assistant" | "answer" => Some(VisibleAgentOutputKind::Answer),
        "status" | "process_exit" => Some(VisibleAgentOutputKind::Actionable),
        _ => None,
    }
}

pub fn normalize_event_kind(event_kind: &str) -> Option<String> {
    let raw = event_kind.trim();
    if raw.is_empty() {
        return None;
    }
    let normalized = raw
        .trim_start_matches("codex.")
        .trim_start_matches("claude.")
        .trim_start_matches("claude_code.")
        .replace(['-', '.'], "_")
        .to_ascii_lowercase();
    let kind = match normalized.as_str() {
        "assistant_message"
        | "assistant_delta"
        | "agent_message"
        | "agent_reasoning"
        | "gemini"
        | "model"
        | "message"
        | "text"
        | "response_output_text"
        | "response_output_text_delta"
        | "content_block_delta" => "assistant",
        "tool"
        | "tool_use"
        | "tool_call"
        | "tool_invocation"
        | "exec"
        | "exec_command"
        | "exec_command_begin"
        | "command"
        | "shell"
        | "bash"
        | "local_shell_call"
        | "function_call"
        | "custom_tool_call"
        | "tool_search_call"
        | "web_search_call"
        | "image_generation_call"
        | "mcp_tool_call" => "tool_use",
        "tool_result"
        | "tool_output"
        | "tool_response"
        | "exec_output"
        | "exec_result"
        | "exec_command_end"
        | "command_output"
        | "command_result"
        | "function_call_output"
        | "custom_tool_call_output"
        | "tool_search_output" => "tool_result",
        "patch_apply"
        | "patch_apply_begin"
        | "patch_apply_updated"
        | "patch_apply_end"
        | "apply_patch"
        | "patch_set" => "patch",
        "turn_diff" => "diff",
        "file" | "file_write" | "file_edit" | "file_create" | "file_delete" | "file_changed"
        | "file_change" | "file_update" => "file_update",
        "permission"
        | "permission_request"
        | "approval_request"
        | "command_approval"
        | "patch_approval"
        | "guardian_assessment" => "approval",
        "error" | "stderr" => "stderr",
        "response_completed" | "turn_complete" | "task_complete" | "final_answer" => {
            "turn_complete"
        }
        "user_message" => "user",
        _ => normalized.as_str(),
    };
    Some(kind.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleAgentOutput {
    pub kind: VisibleAgentOutputKind,
    pub display: String,
    pub dedupe_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFailureCategory {
    Model,
    Auth,
    Permission,
    RateLimit,
    Trust,
    Runtime,
    Dependency,
    Network,
    Parse,
}

impl AgentFailureCategory {
    pub fn title(self) -> &'static str {
        match self {
            Self::Model => "model not found",
            Self::Auth => "provider authentication failed",
            Self::Permission => "permission blocked execution",
            Self::RateLimit => "provider rate limit or quota",
            Self::Trust => "project trust or git check blocked execution",
            Self::Runtime => "worker runtime is not available",
            Self::Dependency => "dependency blocked by failed worker",
            Self::Network => "network or endpoint failure",
            Self::Parse => "agent output could not be parsed",
        }
    }

    pub fn action(self) -> &'static str {
        match self {
            Self::Model => {
                "Check the role's provider/model in /role, then run /doctor."
            }
            Self::Auth => {
                "Set the provider key in /provider or the referenced env var, then run /doctor."
            }
            Self::Permission => {
                "For Claude Code workers, enable cc_bypass_permissions or grant the permission manually; verify with /doctor."
            }
            Self::RateLimit => {
                "Retry later or switch the affected role to another provider/model with /role."
            }
            Self::Trust => {
                "Trust the project or run from a Git repo; for Codex workers, verify the runtime supports --skip-git-repo-check with /doctor."
            }
            Self::Runtime => {
                "Install the missing worker CLI or update the role runtime in /role; verify with /doctor."
            }
            Self::Dependency => {
                "Open /process full, fix the failed worker first, then rerun the request or queue the dependent work."
            }
            Self::Network => {
                "Check the provider endpoint, proxy, and network, then retry."
            }
            Self::Parse => {
                "Inspect /process full, then adjust the planner/critic/reviewer role or model."
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentFailureHint {
    pub category: AgentFailureCategory,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlFallbackDisplay {
    pub role_label: &'static str,
    pub summary: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrchestrationRoute {
    DirectAnswer,
    SingleWorker,
    FullPipeline,
}

impl OrchestrationRoute {
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "direct-answer" | "direct" => Some(Self::DirectAnswer),
            "single-worker" | "single" => Some(Self::SingleWorker),
            "full-pipeline" | "full" => Some(Self::FullPipeline),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DirectAnswer => "direct-answer",
            Self::SingleWorker => "single-worker",
            Self::FullPipeline => "full-pipeline",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            Self::DirectAnswer => "direct",
            Self::SingleWorker => "single",
            Self::FullPipeline => "full",
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            Self::DirectAnswer => "simple conversational or explanatory request",
            Self::SingleWorker => "atomic request; planner, critic, and reviewer are not needed",
            Self::FullPipeline => {
                "project or implementation work; planner, critic, worker, and reviewer will run"
            }
        }
    }

    pub fn from_reason(reason: &str) -> Option<Self> {
        [Self::DirectAnswer, Self::SingleWorker, Self::FullPipeline]
            .into_iter()
            .find(|route| route.reason() == reason)
    }

    pub fn from_label_or_reason(label: &str, reason: &str) -> Option<Self> {
        Self::from_label(label).or_else(|| Self::from_reason(reason))
    }

    pub fn reason_display_label(reason: &str) -> Option<&'static str> {
        Self::from_reason(reason).map(Self::short_reason_label)
    }

    pub fn short_reason_label(self) -> &'static str {
        match self {
            Self::DirectAnswer => "chat/explain",
            Self::SingleWorker => "atomic worker",
            Self::FullPipeline => "implementation",
        }
    }

    pub fn flow_steps(self) -> &'static str {
        match self {
            Self::DirectAnswer | Self::SingleWorker => "worker -> answer",
            Self::FullPipeline => "planner -> critic -> worker -> reviewer -> answer",
        }
    }

    pub fn review_skip_reason(self) -> Option<&'static str> {
        match self {
            Self::DirectAnswer => Some("conversational answer"),
            Self::SingleWorker => Some("single worker route"),
            Self::FullPipeline => None,
        }
    }
}

const DIRECT_QUESTION_PREFIXES: &[&str] = &[
    "如何",
    "怎么",
    "为什么",
    "为啥",
    "能不能",
    "可以吗",
    "可以不",
    "行不行",
    "啥是",
    "什么是",
    "介绍",
    "解释",
    "说明",
    "建议",
    "计划",
    "坏处",
    "how ",
    "what ",
    "why ",
    "can ",
    "could ",
];

const DIRECT_CHAT_MARKERS: &[&str] = &[
    "你好",
    "你叫",
    "你能",
    "干啥",
    "能干啥",
    "是谁",
    "是什么",
    "hello",
    "hi",
];

const DIAGNOSTIC_MARKERS: &[&str] = &[
    "报错",
    "错误",
    "日志",
    "看日志",
    "网络",
    "安装报错",
    "不能用",
    "无法启动",
    "失败",
    "doctor",
];

const IMPERATIVE_ACTION_MARKERS: &[&str] = &[
    "修改",
    "改一下",
    "改成",
    "修复",
    "优化",
    "重构",
    "实现",
    "新增",
    "删除",
    "创建",
    "新建",
    "生成",
    "搭建",
    "脚手架",
    "写参赛",
    "写 agent",
    "写个 agent",
    "提交",
    "推送",
    "执行",
    "完成",
    "继续做",
    "开始做",
    "来做",
    "去做",
    "做一下",
    "做吧",
    "做啊",
    "搞定",
    "开发",
    "编译",
    "构建",
    "测试",
    "报错",
    "日志",
    "权限",
    "配置",
    "安装",
    "发布",
    "接入",
    "完善",
    "整合",
    "fix",
    "implement",
    "create",
    "generate",
    "scaffold",
    "write code",
    "change",
    "edit",
    "refactor",
    "test",
    "build",
    "debug",
    "error",
    "commit",
    "push",
    "release",
    "install",
    "configure",
];

const ENGINEERING_ACTION_MARKERS: &[&str] = &[
    "修改",
    "改一下",
    "改成",
    "修复",
    "优化",
    "重构",
    "实现",
    "新增",
    "删除",
    "创建",
    "新建",
    "生成",
    "搭建",
    "脚手架",
    "写参赛",
    "写 agent",
    "写个 agent",
    "提交",
    "推送",
    "执行",
    "完成",
    "继续做",
    "开始做",
    "来做",
    "去做",
    "做一下",
    "做吧",
    "做啊",
    "搞定",
    "开发",
    "编译",
    "构建",
    "测试",
    "报错",
    "日志",
    "权限",
    "配置",
    "安装",
    "发布",
    "接入",
    "完善",
    "整合",
    "fix",
    "implement",
    "create",
    "generate",
    "scaffold",
    "write code",
    "change",
    "edit",
    "refactor",
    "test",
    "build",
    "debug",
    "error",
    "commit",
    "push",
    "release",
    "install",
    "configure",
    "workflow",
    "ci",
    "tui",
    "provider",
    "worker",
];

const CONTINUATION_ACTION_MARKERS: &[&str] = &[
    "继续",
    "继续做",
    "按照计划",
    "按计划",
    "照顺序",
    "下一步",
    "都做",
    "全部完成",
    "complete the plan",
    "continue",
    "next step",
];

const CONFIRMATION_CONTINUATIONS: &[&str] = &[
    "好",
    "好啊",
    "好的",
    "可以",
    "可以啊",
    "行",
    "行啊",
    "做",
    "做吧",
    "做啊",
    "ok",
    "okay",
    "go ahead",
    "do it",
];

const FULL_PIPELINE_CONTEXT_MARKERS: &[&str] = &[
    "当前工程",
    "这个工程",
    "当前项目",
    "这个项目",
    "项目",
    "仓库",
    "代码",
    "代码库",
    "tiffany-loop",
    "orchestrator",
    "编排",
    "tui",
    "provider",
    "worker",
    "role",
    "角色",
    "codex",
    "claude",
    "readme",
    "action",
    "release",
    "brew",
    "homebrew",
    "github",
    "ci",
    "workflow",
    "提交",
    "推送",
    "src/",
    "tests/",
    ".rs",
    ".toml",
    ".md",
];

impl AgentFailureHint {
    pub fn title(&self) -> &'static str {
        self.category.title()
    }

    pub fn action(&self) -> &'static str {
        self.category.action()
    }
}

pub fn request_looks_direct_answer(text: &str) -> bool {
    classify_orchestration_route(text) == OrchestrationRoute::DirectAnswer
}

pub fn contains_engineering_action(text: &str) -> bool {
    let lower = text.to_lowercase();
    ENGINEERING_ACTION_MARKERS
        .iter()
        .any(|needle| lower.contains(needle))
}

pub fn looks_like_direct_request(text: &str) -> bool {
    looks_like_direct_answer_request(text)
}

pub fn classify_orchestration_route(text: &str) -> OrchestrationRoute {
    let request = current_user_request(text);
    let request_lower = request.to_lowercase();
    let context_lower = contextual_history(text).to_lowercase();
    let has_full_context = has_marker(&request_lower, FULL_PIPELINE_CONTEXT_MARKERS);
    let has_imperative_action = has_marker(&request_lower, IMPERATIVE_ACTION_MARKERS);
    let is_continuation = looks_like_continuation_request(request);
    if is_continuation && has_marker(&context_lower, FULL_PIPELINE_CONTEXT_MARKERS) {
        return OrchestrationRoute::FullPipeline;
    }
    if is_continuation && !has_imperative_action {
        return OrchestrationRoute::SingleWorker;
    }
    if looks_like_link_reference(request) {
        if has_imperative_action && has_full_context {
            return OrchestrationRoute::FullPipeline;
        }
        return OrchestrationRoute::SingleWorker;
    }
    if looks_like_direct_answer_request(request) {
        return OrchestrationRoute::DirectAnswer;
    }
    if has_imperative_action && has_full_context {
        return OrchestrationRoute::FullPipeline;
    }
    if looks_like_single_worker_request(request) {
        return OrchestrationRoute::SingleWorker;
    }
    OrchestrationRoute::FullPipeline
}

fn looks_like_direct_answer_request(text: &str) -> bool {
    let request = text.trim();
    let lower = request.to_lowercase();
    if DIRECT_CHAT_MARKERS
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return !has_marker(&lower, IMPERATIVE_ACTION_MARKERS);
    }
    if request.ends_with('?') || request.ends_with('？') {
        return !starts_with_imperative_action(&lower);
    }
    DIRECT_QUESTION_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        && !starts_with_imperative_action(&lower)
}

fn looks_like_single_worker_request(request: &str) -> bool {
    let lower = request.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    if has_marker(&lower, DIAGNOSTIC_MARKERS) && !has_marker(&lower, FULL_PIPELINE_CONTEXT_MARKERS)
    {
        return true;
    }
    if !has_marker(&lower, IMPERATIVE_ACTION_MARKERS) {
        return false;
    }
    !has_marker(&lower, FULL_PIPELINE_CONTEXT_MARKERS)
}

fn looks_like_continuation_request(request: &str) -> bool {
    let lower = request.trim().to_lowercase();
    has_marker(&lower, CONTINUATION_ACTION_MARKERS)
        || CONFIRMATION_CONTINUATIONS
            .iter()
            .any(|marker| lower == *marker)
}

fn starts_with_imperative_action(lower: &str) -> bool {
    IMPERATIVE_ACTION_MARKERS
        .iter()
        .any(|marker| lower.starts_with(marker))
}

fn has_marker(lower: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| lower.contains(marker))
}

pub fn looks_like_link_reference(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return true;
    }
    trimmed
        .split_whitespace()
        .any(|part| part.starts_with("http://") || part.starts_with("https://"))
}

pub fn current_user_request(text: &str) -> &str {
    let Some(idx) = text.rfind("Current user request:") else {
        return text.trim();
    };
    let request = text[idx + "Current user request:".len()..].trim();
    if request.is_empty() {
        text.trim()
    } else {
        request
    }
}

fn contextual_history(text: &str) -> &str {
    let before_request = text
        .rfind("Current user request:")
        .map(|idx| &text[..idx])
        .unwrap_or(text);
    for marker in ["Conversation context:", "Previous turns:"] {
        if let Some(idx) = before_request.rfind(marker) {
            return before_request[idx + marker.len()..].trim();
        }
    }
    before_request.trim()
}

pub fn agent_failure_hint(content: &str, max: usize) -> Option<AgentFailureHint> {
    let display = humanize_jsonish(content, max);
    let evidence = normalize_output_summary(&display);
    let evidence = if evidence.trim().is_empty() {
        humanize_user_visible_text(content, max)
    } else {
        humanize_user_visible_text(evidence.trim(), max)
    };
    if evidence.trim().is_empty() {
        return None;
    }

    let lower = format!("{content}\n{evidence}").to_ascii_lowercase();
    if user_action_request_display(content, &evidence, max).is_some() {
        return None;
    }
    let category = classify_failure_category(&lower)?;
    Some(AgentFailureHint { category, evidence })
}

pub fn humanize_user_visible_text(content: &str, max: usize) -> String {
    let text = content
        .replace("sub_task(s)", "worker run(s)")
        .replace("sub-tasks", "worker runs")
        .replace("sub-task", "worker run")
        .replace("sub_tasks", "worker runs");
    sanitize_text(text.trim(), max)
}

pub fn classify_json_line(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| event_kind_from_value(&value))
        .unwrap_or_else(|| "raw".to_string())
}

pub fn summarize_cli_stream_line(line: &str, max: usize) -> Option<AgentEventSummary> {
    let value = match serde_json::from_str::<Value>(line) {
        Ok(value) => value,
        Err(_) => {
            let text = sanitize_text(line, max);
            return (!text.trim().is_empty()).then(|| AgentEventSummary {
                kind: "raw".into(),
                text,
                tool_name: None,
            });
        }
    };

    summarize_event_value(&value, max)
}

pub fn summarize_event_value(value: &Value, max: usize) -> Option<AgentEventSummary> {
    if let Some(payload) = event_wrapper_payload(value) {
        return summarize_event_value(payload, max);
    }
    let kind = event_kind_from_value(value).unwrap_or_else(|| "event".to_string());
    let text = value
        .as_object()
        .and_then(summarize_named_agent_event_object)
        .or_else(|| value.as_object().and_then(summarize_inline_tool_object))
        .or_else(|| extract_text_from_value(value))
        .or_else(|| {
            value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("line")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| summarize_json_value(value));
    let text = sanitize_text(&text, max);
    let tool_name = tool_name_from_value(value);
    (!text.trim().is_empty()).then(|| AgentEventSummary {
        kind,
        text,
        tool_name,
    })
}

pub fn format_runtime_output(agent: &str, event_kind: &str, payload: &Value, max: usize) -> String {
    let summary = summarize_event_payload(payload, max);
    if summary.trim().is_empty() {
        format!("{agent} {event_kind}")
    } else {
        format!("{agent} {event_kind}: {summary}")
    }
}

pub fn tool_name_from_value(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => {
            if let Some(name) = object
                .get("name")
                .or_else(|| object.get("displayName"))
                .or_else(|| object.get("tool_name"))
                .or_else(|| object.get("tool"))
                .and_then(Value::as_str)
                .and_then(non_empty)
            {
                return Some(name);
            }
            if let Some(name) = object
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .and_then(non_empty)
            {
                return Some(name);
            }
            for key in [
                "input",
                "parameters",
                "args",
                "arguments",
                "action",
                "payload",
                "item",
                "event_msg",
                "response_item",
            ] {
                if let Some(found) = object.get(key).and_then(tool_name_from_value) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(tool_name_from_value),
        _ => None,
    }
}

pub fn summarize_event_payload(payload: &Value, max: usize) -> String {
    if payload.is_null() {
        return String::new();
    }
    if let Some(text) = payload.as_str() {
        return sanitize_text(text, max);
    }
    if let Some(summary) = payload
        .as_object()
        .and_then(summarize_named_agent_event_object)
    {
        return sanitize_text(&summary, max);
    }
    if let Some(text) = extract_text_from_value(payload) {
        return sanitize_text(&text, max);
    }
    sanitize_text(&summarize_json_value(payload), max)
}

pub fn extract_text_from_value(value: &Value) -> Option<String> {
    let parts = collect_text_parts(value);
    if !parts.is_empty() {
        return Some(parts.join("\n"));
    }

    let object = value.as_object()?;
    if let Some(name) = object
        .get("name")
        .or_else(|| object.get("displayName"))
        .or_else(|| object.get("tool_name"))
        .and_then(Value::as_str)
        .and_then(non_empty)
    {
        return Some(format!("tool {name}"));
    }
    object
        .get("type")
        .and_then(Value::as_str)
        .and_then(non_empty)
}

fn collect_text_parts(value: &Value) -> Vec<String> {
    if let Some(text) = value.as_str().and_then(non_empty) {
        return vec![text];
    }
    if let Some(array) = value.as_array() {
        return dedupe_text_parts(array.iter().flat_map(collect_text_parts).collect());
    }
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    if let Some(summary) = summarize_named_agent_event_object(object) {
        return vec![summary];
    }
    if let Some(summary) = summarize_inline_tool_object(object) {
        return vec![summary];
    }

    let mut parts = Vec::new();
    for key in [
        "text",
        "summary",
        "delta",
        "line",
        "result",
        "message",
        "content",
        "error",
        "item",
        "event_msg",
        "response_item",
        "payload",
    ] {
        if let Some(value) = object.get(key) {
            parts.extend(collect_text_parts(value));
        }
    }
    dedupe_text_parts(parts)
}

fn dedupe_text_parts(parts: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for part in parts {
        if out.last().map(String::as_str) != Some(part.as_str()) {
            out.push(part);
        }
    }
    out
}

pub fn final_output_candidate(content: &str, max: usize) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    for marker in [
        " result: ",
        " final: ",
        " final_answer: ",
        " task_complete: ",
        " turn_complete: ",
    ] {
        if let Some(idx) = lower.find(marker) {
            let raw = &content[idx + marker.len()..];
            let result = strip_known_final_heading(&humanize_jsonish(raw, max));
            if !result.trim().is_empty() {
                return Some(result);
            }
        }
    }

    let normalized = normalize_output_summary(&humanize_jsonish(content, max));
    if let Some(raw) = strip_final_heading(&normalized) {
        let result = humanize_jsonish(raw, max);
        if !result.trim().is_empty() {
            return Some(result);
        }
    }
    None
}

pub fn clean_visible_agent_output(content: &str, max: usize) -> Option<String> {
    let cleaned = content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return Some(String::new());
            }
            if trimmed == "thinking" || trimmed.ends_with(" system: system") {
                return None;
            }
            Some(strip_runtime_output_prefix(line))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if cleaned.is_empty() {
        return None;
    }

    let display = humanize_jsonish(&cleaned, max);
    let display = normalize_output_summary(&display);
    let display = strip_known_final_heading(&display);
    let display = normalize_agent_status_display(&display);
    (!is_low_value_output(&display)).then_some(display)
}
pub fn visible_agent_output(content: &str, max: usize) -> Option<VisibleAgentOutput> {
    let hinted_kind = visible_output_kind_hint(content);
    let hinted_tool_event = hinted_kind.is_some_and(VisibleAgentOutputKind::is_process_event);
    if !hinted_tool_event {
        if let Some(final_output) = final_output_candidate(content, max) {
            let display = sanitize_text(&normalize_output_summary(&final_output), max);
            let display = strip_known_final_heading(&display);
            let display = display.trim().to_string();
            if display.is_empty() {
                return None;
            }
            return Some(VisibleAgentOutput {
                kind: VisibleAgentOutputKind::Final,
                dedupe_key: sanitize_text(&normalize_output_summary(&display), max),
                display,
            });
        }
    }

    let display = clean_visible_agent_output(content, max)?;
    if let Some(approval) = user_action_request_display(content, &display, max) {
        return Some(VisibleAgentOutput {
            kind: VisibleAgentOutputKind::Approval,
            dedupe_key: sanitize_text(&normalize_output_summary(&approval), max),
            display: approval,
        });
    }
    if let Some(question) = question_output_display(content, &display, max) {
        return Some(VisibleAgentOutput {
            kind: VisibleAgentOutputKind::Question,
            dedupe_key: sanitize_text(&normalize_output_summary(&question), max),
            display: question,
        });
    }
    let display_kind = visible_output_kind_from_display(&display);
    let kind = if let Some(kind) = display_kind.filter(|kind| kind.is_artifact_event()) {
        kind
    } else if hinted_kind == Some(VisibleAgentOutputKind::Answer)
        && looks_like_actionable_output(&display)
    {
        VisibleAgentOutputKind::Actionable
    } else {
        hinted_kind.unwrap_or_else(|| {
            if let Some(kind) = display_kind {
                kind
            } else if looks_like_actionable_output(&display) {
                VisibleAgentOutputKind::Actionable
            } else {
                VisibleAgentOutputKind::Normal
            }
        })
    };
    let dedupe_key = sanitize_text(&normalize_output_summary(&display), max);
    Some(VisibleAgentOutput {
        kind,
        display,
        dedupe_key,
    })
}

fn visible_output_kind_from_display(display: &str) -> Option<VisibleAgentOutputKind> {
    let lower = display.trim_start().to_ascii_lowercase();
    if lower.starts_with("diff --git")
        || lower.starts_with("--- ")
        || lower.starts_with("+++ ")
        || lower.starts_with("@@ ")
        || lower.starts_with("files changed:")
    {
        return Some(VisibleAgentOutputKind::Diff);
    }
    if lower.starts_with("*** begin patch") || lower.starts_with("patch:") {
        return Some(VisibleAgentOutputKind::Patch);
    }
    if lower.starts_with("file changed:")
        || lower.starts_with("file updated:")
        || lower.starts_with("file created:")
        || lower.starts_with("file deleted:")
        || lower.starts_with("file created successfully at:")
        || (lower.starts_with("the file ") && lower.contains(" has been updated successfully"))
    {
        return Some(VisibleAgentOutputKind::FileUpdate);
    }
    if lower.starts_with("tool result:") || lower == "tool result" {
        return Some(VisibleAgentOutputKind::ToolResult);
    }
    if lower.starts_with("tool error:") || lower == "tool error" {
        return Some(VisibleAgentOutputKind::Actionable);
    }
    if lower.starts_with("tool ") {
        return Some(VisibleAgentOutputKind::ToolCall);
    }
    None
}

fn visible_output_kind_hint(content: &str) -> Option<VisibleAgentOutputKind> {
    let (prefix, body) = content.trim_start().split_once(": ")?;
    if prefix_is_stderr(prefix) {
        return Some(VisibleAgentOutputKind::Stderr);
    }
    let kind = runtime_output_kind_from_prefix(prefix)?;
    match kind.as_str() {
        "tool"
        | "tool_use"
        | "exec"
        | "local_shell_call"
        | "function_call"
        | "custom_tool_call"
        | "tool_search_call"
        | "web_search_call"
        | "image_generation_call"
        | "mcp_tool_call" => Some(VisibleAgentOutputKind::ToolCall),
        "tool_result"
        | "function_call_output"
        | "custom_tool_call_output"
        | "tool_search_output" => Some(VisibleAgentOutputKind::ToolResult),
        "approval" => Some(VisibleAgentOutputKind::Approval),
        "diff" => Some(VisibleAgentOutputKind::Diff),
        "patch" => Some(VisibleAgentOutputKind::Patch),
        "file_change" | "file_update" => Some(VisibleAgentOutputKind::FileUpdate),
        "user" | "output" => visible_output_kind_from_display(body),
        "assistant" => Some(VisibleAgentOutputKind::Answer),
        "status" | "process_exit" => visible_output_kind_from_display(body).or_else(|| {
            if user_action_request_display(content, body, TOOL_DETAIL_MAX_CHARS).is_some() {
                Some(VisibleAgentOutputKind::Approval)
            } else {
                looks_like_actionable_output(body).then_some(VisibleAgentOutputKind::Actionable)
            }
        }),
        _ => None,
    }
}

fn question_output_display(content: &str, display: &str, max: usize) -> Option<String> {
    let normalized = normalize_output_summary(display);
    let normalized = normalized.trim();
    let lower = format!("{content}\n{normalized}").to_ascii_lowercase();
    let normalized_lower = normalized.to_ascii_lowercase();

    if normalized_lower.starts_with("question requested")
        || normalized_lower.starts_with("waiting for user input")
    {
        return Some(sanitize_text(normalized, max));
    }

    if lower.contains("tool error: answer questions")
        || lower.contains("tool askuserquestion error")
        || lower.contains("request_user_input error")
    {
        return Some(sanitize_text(
            "waiting for user input: Answer questions?",
            max,
        ));
    }

    if lower.contains("askuserquestion") || lower.contains("request_user_input") {
        let content_detail = question_detail_from_text(content);
        let display_detail = question_detail_from_text(display);
        let detail = content_detail.or(display_detail);
        return Some(sanitize_text(
            &format_question_waiting_summary(
                detail.or_else(|| Some("Answer questions?".to_string())),
            ),
            max,
        ));
    }

    None
}

fn question_detail_from_text(text: &str) -> Option<String> {
    if let Some(value) = first_embedded_json_value(text) {
        if let Some(object) = value.as_object() {
            if let Some(detail) = question_tool_detail_text(object) {
                return Some(detail);
            }
        }
    }

    for marker in ["AskUserQuestion", "ask_user_question", "request_user_input"] {
        let Some(idx) = text.to_ascii_lowercase().find(&marker.to_ascii_lowercase()) else {
            continue;
        };
        let detail = text[idx + marker.len()..]
            .trim_start_matches(|ch: char| ch == ':' || ch == '-' || ch.is_whitespace())
            .trim();
        if !detail.is_empty()
            && !detail.eq_ignore_ascii_case("tool")
            && !detail.eq_ignore_ascii_case("tool_use")
        {
            return Some(humanize_argument_text(detail));
        }
    }
    None
}

fn first_embedded_json_value(text: &str) -> Option<Value> {
    let start = text
        .char_indices()
        .find_map(|(idx, ch)| (ch == '{').then_some(idx))?;
    let mut iter = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
    iter.next()?.ok()
}

fn user_action_request_display(content: &str, display: &str, max: usize) -> Option<String> {
    let lower = format!("{content}\n{display}").to_ascii_lowercase();
    if !looks_like_user_action_request(&lower) {
        return None;
    }

    let label = if contains_any(
        &lower,
        &[
            "request_permissions",
            "request permissions",
            "permission request",
            "permissions request",
            "grant permission",
            "grant permissions",
            "additional permissions",
            "sandbox",
        ],
    ) {
        "waiting for permission approval"
    } else if contains_any(&lower, &["patch", "file change", "apply_patch"]) {
        "waiting for patch approval"
    } else if contains_any(&lower, &["command", "shell", "exec"]) {
        "waiting for command approval"
    } else {
        "waiting for user approval"
    };

    let detail = user_action_request_detail(display);
    match detail {
        Some(detail) => Some(format!(
            "{label}: {}",
            sanitize_text(detail.trim(), max.min(220))
        )),
        None => Some(label.to_string()),
    }
}

fn looks_like_user_action_request(lower: &str) -> bool {
    if contains_any(
        lower,
        &[
            "permission denied",
            "operation not permitted",
            "rejected",
            "denied",
            "failed",
            "failure",
            "not confirmed",
            "forbidden",
            "unauthorized",
            "error:",
        ],
    ) {
        return false;
    }

    contains_any(
        lower,
        &[
            "request_permissions",
            "request permissions",
            "permission request",
            "permissions request",
            "request approval",
            "approval request",
            "requires approval",
            "approval required",
            "needs approval",
            "waiting for approval",
            "approve this command",
            "approve command",
            "allow this command",
            "allow command",
            "grant permissions",
            "grant permission",
            "requires additional permissions",
            "additional permissions",
        ],
    )
}

fn user_action_request_detail(display: &str) -> Option<String> {
    let normalized = normalize_output_summary(display);
    let mut detail = normalized.trim();
    for prefix in [
        "tool request_permissions:",
        "tool request permissions:",
        "request_permissions:",
        "request permissions:",
        "permission request:",
        "permissions request:",
        "approval request:",
        "request approval:",
        "command requires approval:",
        "command approval required:",
        "command needs approval:",
        "approve command:",
        "approve this command:",
        "allow command:",
        "allow this command:",
        "patch requires approval:",
        "patch approval required:",
        "patch needs approval:",
        "requires approval:",
        "approval required:",
        "needs approval:",
        "waiting for approval:",
    ] {
        if detail.to_ascii_lowercase().starts_with(prefix) {
            detail = detail[prefix.len()..].trim();
            break;
        }
    }
    if detail.is_empty()
        || matches!(
            detail.to_ascii_lowercase().as_str(),
            "tool request_permissions"
                | "tool request permissions"
                | "request_permissions"
                | "request permissions"
                | "approval request"
                | "request approval"
        )
    {
        return None;
    }
    Some(detail.to_string())
}

fn prefix_is_stderr(prefix: &str) -> bool {
    let mut parts = prefix.split_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some("stderr"), None, None) => true,
        (Some(runtime), Some("stderr"), None) if is_known_runtime_prefix(runtime) => true,
        _ => false,
    }
}

pub fn strip_runtime_output_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    let Some((prefix, rest)) = trimmed.split_once(": ") else {
        return line.to_string();
    };
    if runtime_output_kind_from_prefix(prefix).is_some() {
        return rest.to_string();
    }
    line.to_string()
}

pub fn runtime_output_kind(content: &str) -> Option<String> {
    let prefix = content.trim_start().split_once(": ")?.0;
    runtime_output_kind_from_prefix(prefix)
}

pub fn strip_known_final_heading(content: &str) -> String {
    strip_final_heading(content)
        .unwrap_or_else(|| content.trim())
        .to_string()
}

fn strip_final_heading(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    for heading in [
        "final result",
        "final answer",
        "final_result",
        "final_answer",
    ] {
        if trimmed.eq_ignore_ascii_case(heading) {
            return Some("");
        }
        if trimmed
            .get(..heading.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(heading))
        {
            let rest = &trimmed[heading.len()..];
            if let Some(rest) = rest.strip_prefix(':') {
                return Some(rest.trim_start());
            }
            if rest.starts_with('\n') || rest.starts_with("\r\n") {
                return Some(rest.trim_start());
            }
        }
    }
    None
}

pub fn normalized_output_key(content: &str, max: usize) -> Option<String> {
    let display = humanize_jsonish(content, max);
    if is_low_value_output(&display) {
        return None;
    }
    Some(sanitize_text(&normalize_output_summary(&display), max))
}

pub fn normalize_output_summary(display: &str) -> String {
    let trimmed = display.trim();
    let secondary = strip_secondary_output_prefix(trimmed);
    if secondary != trimmed {
        return humanize_user_visible_text(secondary, usize::MAX);
    }
    let Some((prefix, body)) = trimmed.split_once(": ") else {
        return humanize_user_visible_text(trimmed, usize::MAX);
    };

    match runtime_output_kind_from_prefix(prefix) {
        Some(_) => {
            humanize_user_visible_text(strip_secondary_output_prefix(body.trim()), usize::MAX)
        }
        None => humanize_user_visible_text(trimmed, usize::MAX),
    }
}

fn strip_secondary_output_prefix(body: &str) -> &str {
    let lower = body.to_ascii_lowercase();
    for prefix in [
        "tool result:",
        "tool_result:",
        "tool output:",
        "tool_output:",
        "tool error:",
        "tool_error:",
        "assistant:",
        "result:",
        "final:",
    ] {
        if lower.starts_with(prefix) {
            return body[prefix.len()..].trim_start();
        }
    }
    body
}

fn runtime_output_kind_from_prefix(prefix: &str) -> Option<String> {
    let prefix = prefix.to_ascii_lowercase();
    let mut parts = prefix.split_whitespace();
    let runtime = parts.next()?;
    let kind = parts.next()?;
    if !is_known_runtime_prefix(runtime) {
        return None;
    }
    known_runtime_output_kind(kind)
}

fn is_known_runtime_prefix(runtime: &str) -> bool {
    matches!(
        runtime,
        "agent"
            | "claude"
            | "claude-code"
            | "cc"
            | "codex"
            | "gemini"
            | "worker-cc"
            | "worker-codex"
            | "worker-gemini"
    )
}

fn known_runtime_output_kind(kind: &str) -> Option<String> {
    normalize_event_kind(kind).filter(|kind| {
        matches!(
            kind.as_str(),
            "assistant"
                | "event"
                | "output"
                | "user"
                | "system"
                | "status"
                | "process_exit"
                | "approval"
                | "diff"
                | "patch"
                | "file_update"
                | "stderr"
                | "result"
                | "final"
                | "turn_complete"
                | "tool_use"
                | "tool_result"
        )
    })
}

fn normalize_agent_status_display(display: &str) -> String {
    let trimmed = strip_claude_context_note(display.trim());
    let lower = trimmed.to_ascii_lowercase();
    if let Some(path) = lower
        .starts_with("file created successfully at:")
        .then(|| trimmed["file created successfully at:".len()..].trim())
        .filter(|path| !path.is_empty())
    {
        return format!("file created: {path}");
    }
    if lower.starts_with("the file ") && lower.contains(" has been updated successfully") {
        if let Some(path) = trimmed
            .strip_prefix("The file ")
            .or_else(|| trimmed.strip_prefix("the file "))
            .and_then(|rest| rest.split(" has been updated successfully").next())
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            return format!("file updated: {path}");
        }
    }
    trimmed.to_string()
}

fn strip_claude_context_note(text: &str) -> &str {
    for marker in [
        " (file state is current in your context",
        " (no need to Read it back",
    ] {
        if let Some(idx) = text.find(marker) {
            return text[..idx].trim_end();
        }
    }
    text
}

pub fn humanize_jsonish(content: &str, max: usize) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(summary) = summarize_angle_role_block(trimmed, max) {
        return humanize_user_visible_text(&summary, max);
    }
    if let Some(summary) = summarize_json_code_fence(trimmed) {
        return humanize_user_visible_text(&summary, max);
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return humanize_user_visible_text(&summarize_json_value(&value), max);
    }
    if let Some((prefix, raw)) = trimmed.split_once(": ") {
        let raw = raw.trim();
        if looks_like_json(raw) {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                let summary = summarize_json_value(&value);
                return humanize_user_visible_text(&format!("{}: {}", prefix.trim(), summary), max);
            }
        }
    }
    if let Some(summary) = summarize_jsonish_lines(trimmed) {
        return humanize_user_visible_text(&summary, max);
    }
    if let Some(summary) = summarize_embedded_json_with_context(trimmed) {
        return humanize_user_visible_text(&summary, max);
    }
    if let Some(summary) = summarize_loose_structured_json(trimmed) {
        return humanize_user_visible_text(&summary, max);
    }
    humanize_user_visible_text(trimmed, max)
}

pub fn is_low_value_output(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    let stderr_like = lower.contains(" stderr:") || lower.ends_with(" stderr");
    if stderr_like && !looks_like_actionable_stderr(&lower) {
        return true;
    }
    if looks_like_internal_tool_selection(&lower) {
        return true;
    }
    if looks_like_successful_process_exit(&lower) {
        return true;
    }

    matches!(
        lower.as_str(),
        "system"
            | "thinking"
            | "claude system"
            | "claude-code system"
            | "claude assistant: thinking"
            | "claude-code assistant: thinking"
            | "claude finished"
            | "claude-code finished"
            | "codex system"
            | "codex assistant: thinking"
            | "codex finished"
            | "tool result"
            | "(bash completed with no output)"
            | "bash completed with no output"
            | "(tool completed with no output)"
            | "tool completed with no output"
    ) || lower.contains(" heartbeat")
        || lower.ends_with(" system: system")
        || lower.ends_with(" assistant: assistant")
        || lower.ends_with(" assistant: thinking")
        || lower.contains("starting model=")
        || lower.contains("could not create path aliases")
}

fn looks_like_successful_process_exit(lower: &str) -> bool {
    if !lower.contains("exited with status") {
        return false;
    }
    lower.contains("exit status: 0")
        || lower.ends_with(" status 0")
        || lower.ends_with(" status: 0")
}

fn looks_like_internal_tool_selection(lower: &str) -> bool {
    let lower = lower.trim();
    if !lower.starts_with("tool ") {
        return false;
    }
    lower.starts_with("tool toolsearch:")
        || lower.starts_with("tool tool_search:")
        || lower.starts_with("tool select:")
        || lower.starts_with("tool selecttool:")
}

fn looks_like_actionable_stderr(lower: &str) -> bool {
    looks_like_actionable_lowercase_output(lower)
}

pub fn looks_like_actionable_output(text: &str) -> bool {
    looks_like_actionable_lowercase_output(&text.to_ascii_lowercase())
}

fn looks_like_actionable_lowercase_output(lower: &str) -> bool {
    [
        "error",
        "failed",
        "failure",
        "rejected",
        "needs changes",
        "panic",
        "unauthorized",
        "authentication",
        "not authenticated",
        "forbidden",
        "permission",
        "denied",
        "rate limit",
        "api key",
        "login",
        "native session busy",
        "native session missing",
        "native session was not found",
        "exited with status",
        "trusted directory",
        "skip-git-repo-check",
        "git repo check",
        "model not found",
        "unknown model",
        "invalid model",
        "not found",
        "模型不存在",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn classify_failure_category(lower: &str) -> Option<AgentFailureCategory> {
    if contains_any(
        lower,
        &[
            "model not found",
            "unknown model",
            "invalid model",
            "model does not exist",
            "模型不存在",
            "模型代码",
            "1211",
        ],
    ) {
        return Some(AgentFailureCategory::Model);
    }
    if contains_any(
        lower,
        &[
            "api key",
            "unauthorized",
            "authentication",
            "not authenticated",
            "invalid auth",
            "invalid token",
            "missing token",
            "401",
        ],
    ) {
        return Some(AgentFailureCategory::Auth);
    }
    if contains_any(
        lower,
        &[
            "permission denied",
            "permission",
            "forbidden",
            "not allowed",
            "approval",
            "sandbox",
            "403",
        ],
    ) {
        return Some(AgentFailureCategory::Permission);
    }
    if contains_any(
        lower,
        &[
            "not inside a trusted directory",
            "trusted directory",
            "--skip-git-repo-check",
            "skip-git-repo-check",
            "git repo check",
        ],
    ) {
        return Some(AgentFailureCategory::Trust);
    }
    if contains_any(
        lower,
        &[
            "rate limit",
            "quota",
            "too many requests",
            "insufficient quota",
            "429",
        ],
    ) {
        return Some(AgentFailureCategory::RateLimit);
    }
    if contains_any(
        lower,
        &[
            "is it installed",
            "no such file or directory",
            "command not found",
            "runtime missing",
            "runtime-missing",
            "worker runtime",
            "failed to spawn",
            "spawning ",
        ],
    ) {
        return Some(AgentFailureCategory::Runtime);
    }
    if contains_any(
        lower,
        &[
            "task dag blocked",
            "failed dependencies",
            "behind failed dependencies",
            "blocked by failed",
            "dependency failed",
            "failed dependency",
        ],
    ) {
        return Some(AgentFailureCategory::Dependency);
    }
    if contains_any(
        lower,
        &[
            "connection refused",
            "connection reset",
            "dns",
            "timeout",
            "timed out",
            "network",
            "endpoint",
            "tls",
            "ssl",
            "econn",
        ],
    ) {
        return Some(AgentFailureCategory::Network);
    }
    if contains_any(
        lower,
        &[
            "parse failed",
            "malformed",
            "invalid json",
            "expected value",
            "no sub_tasks",
            "returned no sub_tasks",
        ],
    ) {
        return Some(AgentFailureCategory::Parse);
    }
    None
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

pub fn is_redundant_role_output(role: &str, content: &str, max: usize) -> bool {
    if role == "worker" {
        return false;
    }
    let display = humanize_jsonish(content, max);
    let normalized = normalize_output_summary(&display);
    let lines = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let lower = lines
        .first()
        .copied()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match role {
        "planner" => lower.starts_with("plan ready"),
        "critic" | "reviewer" => {
            lower.starts_with("approved")
                || (lower.starts_with("needs changes")
                    && !role_output_has_actionable_detail(&lines))
        }
        _ => false,
    }
}

pub fn control_fallback_display(
    role: &str,
    message: &str,
    reason: &str,
    max: usize,
) -> ControlFallbackDisplay {
    let role_label = control_role_label(role);
    let reason = humanize_jsonish(reason, max);
    let reason = reason.trim();
    let message = message.trim();

    let summary = if control_fallback_is_single_worker_downgrade(message, reason) {
        if reason.is_empty() {
            "single-worker fallback".to_string()
        } else {
            format!("single-worker fallback · {reason}")
        }
    } else if reason.is_empty() {
        message.to_string()
    } else {
        format!("{message} · {reason}")
    };

    ControlFallbackDisplay {
        role_label,
        summary,
    }
}

fn control_role_label(role: &str) -> &'static str {
    match role {
        "planner" => "planner",
        "critic" => "critic",
        "reviewer" => "reviewer",
        _ => "control",
    }
}

fn control_fallback_is_single_worker_downgrade(message: &str, reason: &str) -> bool {
    let message = message.to_ascii_lowercase();
    let reason = reason.to_ascii_lowercase();
    message.contains("fell back to original task")
        || message.contains("using original task")
        || reason.contains("using original task")
        || reason.contains("no usable worker runs")
        || reason.contains("non-json output")
}

fn role_output_has_actionable_detail(lines: &[&str]) -> bool {
    lines.iter().skip(1).any(|line| {
        let lower = line.trim().to_ascii_lowercase();
        !matches!(
            lower.as_str(),
            "issue:" | "issues:" | "suggestion:" | "suggestions:"
        )
    })
}

pub fn sanitize_text(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max {
            out.push('…');
            return out;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn event_kind_from_value(value: &Value) -> Option<String> {
    if let Some(payload) = event_wrapper_payload(value) {
        if let Some(kind) = event_kind_from_value(payload) {
            return Some(kind);
        }
    }
    if let Some(kind) = semantic_event_kind_from_value(value) {
        return Some(kind);
    }
    let raw = value
        .get("type")
        .or_else(|| value.get("kind"))
        .or_else(|| value.get("event"))
        .or_else(|| value.get("event_type"))
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)?;
    normalize_event_kind(raw)
}

fn semantic_event_kind_from_value(value: &Value) -> Option<String> {
    if let Some(payload) = event_wrapper_payload(value) {
        if let Some(kind) = semantic_event_kind_from_value(payload) {
            return Some(kind);
        }
    }

    let object = value.as_object()?;
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("system"))
    {
        return Some("system".to_string());
    }
    if let Some(kind) = object
        .get("message")
        .and_then(semantic_event_kind_from_value)
    {
        return Some(kind);
    }
    if let Some(kind) = semantic_event_kind_from_object(object) {
        return Some(kind);
    }
    if let Some(kind) = semantic_message_event_kind(object) {
        return Some(kind);
    }
    if let Some(kind) = semantic_event_kind_from_named_fields(object) {
        return Some(kind);
    }
    if let Some(kind) = semantic_event_kind_from_type_field(object) {
        return Some(kind);
    }

    for key in [
        "item",
        "event_msg",
        "response_item",
        "payload",
        "event",
        "msg",
        "data",
    ] {
        if let Some(kind) = object.get(key).and_then(semantic_event_kind_from_value) {
            return Some(kind);
        }
    }

    None
}

fn semantic_event_kind_from_type_field(object: &Map<String, Value>) -> Option<String> {
    let kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .or_else(|| object.get("event"))
        .or_else(|| object.get("event_type"))
        .and_then(Value::as_str)
        .and_then(normalize_event_kind)?;
    matches!(
        kind.as_str(),
        "assistant"
            | "tool_use"
            | "tool_result"
            | "diff"
            | "patch"
            | "file_update"
            | "approval"
            | "stderr"
            | "turn_complete"
            | "system"
            | "user"
    )
    .then_some(kind)
}

fn semantic_event_kind_from_named_fields(object: &Map<String, Value>) -> Option<String> {
    if object.get("exec_command_end").is_some() {
        return Some("tool_result".to_string());
    }
    if object.get("exec_command_begin").is_some() {
        return Some("tool_use".to_string());
    }
    if object.get("patch_apply_begin").is_some()
        || object.get("patch_apply_updated").is_some()
        || object.get("patch_apply_end").is_some()
    {
        return Some("patch".to_string());
    }
    if object.get("turn_diff").is_some() {
        return Some("diff".to_string());
    }
    if object.get("guardian_assessment").is_some() {
        return Some("approval".to_string());
    }
    if object.get("toolResults").is_some() {
        return Some("tool_result".to_string());
    }
    if let Some(tool_calls) = object.get("toolCalls").and_then(Value::as_array) {
        if tool_calls.iter().any(gemini_tool_call_has_result) {
            return Some("tool_result".to_string());
        }
        if !tool_calls.is_empty() {
            return Some("tool_use".to_string());
        }
    }
    if object.get("agent_message").is_some()
        || object.get("agent_reasoning").is_some()
        || object.get("gemini").is_some()
        || object.get("model").is_some()
    {
        return Some("assistant".to_string());
    }
    if object.get("user_message").is_some() {
        return Some("user".to_string());
    }
    None
}

fn event_wrapper_payload(value: &Value) -> Option<&Value> {
    let object = value.as_object()?;
    let wrapper_type = object
        .get("type")
        .or_else(|| object.get("kind"))
        .or_else(|| object.get("event"))
        .or_else(|| object.get("event_type"))
        .and_then(Value::as_str)
        .map(|value| value.trim().replace(['-', '.'], "_").to_ascii_lowercase())?;
    if matches!(wrapper_type.as_str(), "event_msg" | "response_item") {
        object.get("payload").filter(|payload| payload.is_object())
    } else {
        None
    }
}

fn semantic_event_kind_from_object(object: &Map<String, Value>) -> Option<String> {
    let content = object.get("content").and_then(Value::as_array)?;
    let mut saw_tool_result = false;
    let mut saw_tool_use = false;
    let mut saw_text = false;

    for item in content {
        let Some(kind) = item
            .get("type")
            .or_else(|| item.get("kind"))
            .or_else(|| item.get("event"))
            .and_then(Value::as_str)
            .and_then(normalize_event_kind)
        else {
            continue;
        };

        match kind.as_str() {
            "tool_result" => saw_tool_result = true,
            "tool_use" => saw_tool_use = true,
            "assistant" => saw_text = true,
            _ => {}
        }
    }

    if saw_tool_result {
        Some("tool_result".to_string())
    } else if saw_tool_use {
        Some("tool_use".to_string())
    } else if saw_text {
        Some("assistant".to_string())
    } else {
        None
    }
}

fn semantic_message_event_kind(object: &Map<String, Value>) -> Option<String> {
    let raw_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(|value| value.trim().replace(['-', '.'], "_").to_ascii_lowercase())?;
    if raw_type != "message" {
        return None;
    }
    match object.get("role").and_then(Value::as_str) {
        Some("assistant") => Some("assistant".to_string()),
        Some("user") => Some("user".to_string()),
        _ => None,
    }
}

fn tool_summary_kind_from_raw(raw_kind: &str) -> Option<String> {
    let normalized = raw_kind
        .trim()
        .trim_start_matches("codex.")
        .trim_start_matches("claude.")
        .trim_start_matches("claude_code.")
        .replace(['-', '.'], "_")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "local_shell_call" => Some("local_shell_call".to_string()),
        "function_call" => Some("function_call".to_string()),
        "function_call_output" => Some("function_call_output".to_string()),
        "custom_tool_call" => Some("custom_tool_call".to_string()),
        "custom_tool_call_output" => Some("custom_tool_call_output".to_string()),
        "tool_search_call" => Some("tool_search_call".to_string()),
        "tool_search_output" => Some("tool_search_output".to_string()),
        "web_search_call" => Some("web_search_call".to_string()),
        "image_generation_call" => Some("image_generation_call".to_string()),
        _ => normalize_event_kind(raw_kind),
    }
}

fn summarize_named_agent_event_object(object: &Map<String, Value>) -> Option<String> {
    for key in [
        "payload",
        "item",
        "event_msg",
        "response_item",
        "event",
        "msg",
        "data",
    ] {
        if let Some(summary) = object
            .get(key)
            .and_then(Value::as_object)
            .and_then(summarize_named_agent_event_object)
        {
            return Some(summary);
        }
    }

    if let Some(message) = object.get("agent_message") {
        return json_string_or_message(message);
    }
    if let Some(message) = object.get("agent_reasoning") {
        return json_string_or_message(message);
    }
    if let Some(message) = object.get("user_message") {
        return json_string_or_message(message);
    }
    if let Some(message) = object.get("message").and_then(json_string_or_message) {
        return Some(message);
    }
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("system"))
    {
        return Some("system".to_string());
    }
    if let Some(message) = object.get("gemini").or_else(|| object.get("model")) {
        return json_string_or_message(message);
    }
    if let Some(tool) = object.get("exec_command_begin") {
        return codex_exec_command_summary(tool);
    }
    if let Some(tool) = object.get("exec_command_end") {
        return codex_exec_result_summary(tool);
    }
    if let Some(patch) = object
        .get("patch_apply_begin")
        .or_else(|| object.get("patch_apply_updated"))
        .or_else(|| object.get("patch_apply_end"))
    {
        return patch_summary_text(patch).map(|patch| sanitize_text(&patch, TOOL_RESULT_MAX_CHARS));
    }
    if let Some(diff) = object.get("turn_diff") {
        return diff_summary_text(diff).map(|diff| sanitize_text(&diff, TOOL_RESULT_MAX_CHARS));
    }
    if let Some(guardian) = object.get("guardian_assessment") {
        return guardian_assessment_summary(guardian);
    }
    if object
        .get("type")
        .and_then(Value::as_str)
        .and_then(normalize_event_kind)
        .as_deref()
        == Some("approval")
    {
        return guardian_assessment_summary(&Value::Object(object.clone()));
    }
    if let Some(tool_calls) = object.get("toolCalls").and_then(Value::as_array) {
        let summaries = tool_calls
            .iter()
            .filter_map(gemini_tool_call_summary)
            .collect::<Vec<_>>();
        if !summaries.is_empty() {
            return Some(summaries.join("\n"));
        }
    }
    if let Some(tool_results) = object.get("toolResults").and_then(Value::as_array) {
        let summaries = tool_results
            .iter()
            .filter_map(gemini_tool_result_summary)
            .collect::<Vec<_>>();
        if !summaries.is_empty() {
            return Some(summaries.join("\n"));
        }
    }
    if let Some(summary) = gemini_tool_call_summary(&Value::Object(object.clone())) {
        return Some(summary);
    }
    if let Some(summary) = gemini_tool_result_summary(&Value::Object(object.clone())) {
        return Some(summary);
    }
    None
}

fn summarize_json_value(value: &Value) -> String {
    if let Some(object) = value.as_object() {
        if let Some(summary) = summarize_error_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_plan_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_review_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_named_agent_event_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_inline_tool_object(object) {
            return summary;
        }
        if let Some(summary) = summarize_tool_object(object) {
            return summary;
        }
    }
    if let Some(text) = extract_text_from_value(value) {
        return text;
    }
    if let Some(array) = value.as_array() {
        return summarize_json_array(array);
    }
    if let Some(object) = value.as_object() {
        let keys = object
            .keys()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if !keys.is_empty() {
            return format!("structured output: {keys}");
        }
        return "structured output".into();
    }
    value.to_string()
}

fn summarize_error_object(object: &Map<String, Value>) -> Option<String> {
    let error = object.get("error")?;
    let message = if let Some(text) = error.as_str().and_then(non_empty) {
        text
    } else if let Some(error_object) = error.as_object() {
        error_object
            .get("message")
            .or_else(|| error_object.get("description"))
            .or_else(|| error_object.get("detail"))
            .and_then(value_to_text)
            .or_else(|| extract_text_from_value(error))
            .unwrap_or_else(|| "request failed".into())
    } else {
        value_to_text(error).unwrap_or_else(|| "request failed".into())
    };

    let code = error
        .as_object()
        .and_then(|error_object| {
            error_object
                .get("code")
                .or_else(|| error_object.get("status"))
                .or_else(|| error_object.get("type"))
        })
        .and_then(value_to_text);

    Some(match code {
        Some(code) if !message.contains(&code) => format!("error: {message} ({code})"),
        _ => format!("error: {message}"),
    })
}

fn summarize_plan_object(object: &Map<String, Value>) -> Option<String> {
    let sub_tasks = object.get("sub_tasks").and_then(Value::as_array)?;
    let mut out = format!("plan ready: {} worker run(s)", sub_tasks.len());

    for (idx, task) in sub_tasks.iter().take(6).enumerate() {
        let prompt = task
            .get("prompt")
            .or_else(|| task.get("title"))
            .or_else(|| task.get("name"))
            .and_then(value_to_text)
            .unwrap_or_else(|| "task".into());
        out.push_str(&format!(
            "\n  {}. {}",
            idx + 1,
            sanitize_text(prompt.trim(), 140)
        ));

        if let Some(agent) = task
            .get("agent_hint")
            .or_else(|| task.get("agent"))
            .or_else(|| task.get("role"))
            .and_then(value_to_text)
        {
            out.push_str(&format!("\n     agent: {}", sanitize_text(&agent, 80)));
        }
    }

    if sub_tasks.len() > 6 {
        out.push_str(&format!("\n  … {} more", sub_tasks.len() - 6));
    }

    Some(out)
}

fn summarize_review_object(object: &Map<String, Value>) -> Option<String> {
    let approved = object.get("approved").and_then(Value::as_bool)?;
    let issues = value_array_texts(object.get("issues"));
    let suggestions = value_array_texts(object.get("suggestions"));
    let mut out = if approved {
        format!("approved: {} issue(s)", issues.len())
    } else {
        format!("needs changes: {} issue(s)", issues.len())
    };

    for issue in issues.iter().take(6) {
        out.push_str("\n  - ");
        out.push_str(&sanitize_text(issue.trim(), 180));
    }
    if issues.len() > 6 {
        out.push_str(&format!("\n  … {} more issue(s)", issues.len() - 6));
    }

    if !suggestions.is_empty() {
        out.push_str("\n  suggestions:");
        for suggestion in suggestions.iter().take(3) {
            out.push_str("\n  - ");
            out.push_str(&sanitize_text(suggestion.trim(), 180));
        }
        if suggestions.len() > 3 {
            out.push_str(&format!(
                "\n  … {} more suggestion(s)",
                suggestions.len() - 3
            ));
        }
    }

    Some(out)
}

fn summarize_tool_object(object: &Map<String, Value>) -> Option<String> {
    let raw_name = object
        .get("tool_name")
        .or_else(|| object.get("name"))
        .or_else(|| object.get("tool"))
        .and_then(Value::as_str)
        .and_then(non_empty)?;
    let name = sanitize_text(&raw_name, 80);
    if is_question_tool_name(&raw_name) {
        return Some(format_question_request_summary(question_tool_detail_text(
            object,
        )));
    }
    if is_permission_tool_name(&raw_name) {
        let detail = permission_request_detail_text(object);
        return Some(match detail {
            Some(detail) => format!("tool {name}: {}", sanitize_text(&detail, 160)),
            None => format!("tool {name}"),
        });
    }

    let detail = tool_detail_text(object);
    Some(match detail {
        Some(detail) => format!("tool {name}: {}", sanitize_text(&detail, 160)),
        None => format!("tool {name}"),
    })
}

fn summarize_inline_tool_object(object: &Map<String, Value>) -> Option<String> {
    let raw_kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .or_else(|| object.get("event"))
        .or_else(|| object.get("event_type"))
        .and_then(Value::as_str)?;
    let kind = tool_summary_kind_from_raw(raw_kind)?;
    summarize_inline_tool_object_with_kind(object, &kind)
}

fn summarize_inline_tool_object_with_kind(
    object: &Map<String, Value>,
    kind: &str,
) -> Option<String> {
    match kind {
        "tool_use" => summarize_tool_object(object).or_else(|| {
            if object
                .get("command")
                .or_else(|| object.get("cmd"))
                .is_some()
            {
                summarize_local_shell_call_object(object)
            } else {
                summarize_function_call_object(object)
            }
        }),
        "tool_result" => summarize_tool_result_object(object),
        "local_shell_call" => summarize_local_shell_call_object(object),
        "function_call" => summarize_function_call_object(object),
        "function_call_output" => summarize_function_call_output_object(object),
        "custom_tool_call" => summarize_custom_tool_call_object(object),
        "custom_tool_call_output" => summarize_custom_tool_call_output_object(object),
        "tool_search_call" => summarize_tool_search_call_object(object),
        "tool_search_output" => summarize_tool_search_output_object(object),
        "web_search_call" => summarize_web_search_call_object(object),
        "image_generation_call" => summarize_image_generation_call_object(object),
        _ => None,
    }
}

fn summarize_local_shell_call_object(object: &Map<String, Value>) -> Option<String> {
    let action = object.get("action").and_then(Value::as_object);
    let detail = action
        .and_then(|action| {
            action
                .get("command")
                .or_else(|| action.get("cmd"))
                .and_then(command_value_to_text)
        })
        .or_else(|| {
            object
                .get("command")
                .or_else(|| object.get("cmd"))
                .and_then(command_value_to_text)
        })
        .or_else(|| object.get("status").and_then(value_to_text));
    Some(format_tool_summary("shell", detail))
}

fn summarize_function_call_object(object: &Map<String, Value>) -> Option<String> {
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(non_empty)?;
    let name = object
        .get("namespace")
        .and_then(Value::as_str)
        .and_then(non_empty)
        .map(|namespace| format!("{namespace}.{name}"))
        .unwrap_or(name);
    let detail = object.get("arguments").and_then(argument_value_to_text);
    Some(format_tool_summary(&name, detail))
}

fn summarize_function_call_output_object(object: &Map<String, Value>) -> Option<String> {
    summarize_output_body(
        object.get("output"),
        object.get("name").and_then(Value::as_str),
        object
            .get("success")
            .and_then(Value::as_bool)
            .map(|success| !success)
            .unwrap_or(false),
    )
}

fn summarize_custom_tool_call_object(object: &Map<String, Value>) -> Option<String> {
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(non_empty)?;
    let detail = object
        .get("input")
        .or_else(|| object.get("arguments"))
        .and_then(argument_value_to_text);
    Some(format_tool_summary(&name, detail))
}

fn summarize_custom_tool_call_output_object(object: &Map<String, Value>) -> Option<String> {
    summarize_output_body(
        object.get("output"),
        object.get("name").and_then(Value::as_str),
        object
            .get("success")
            .and_then(Value::as_bool)
            .map(|success| !success)
            .unwrap_or(false),
    )
}

fn summarize_tool_search_call_object(object: &Map<String, Value>) -> Option<String> {
    let detail = object
        .get("execution")
        .and_then(value_to_text)
        .or_else(|| object.get("arguments").and_then(argument_value_to_text))
        .or_else(|| object.get("status").and_then(value_to_text));
    Some(format_tool_summary("search", detail))
}

fn summarize_tool_search_output_object(object: &Map<String, Value>) -> Option<String> {
    let status = object.get("status").and_then(value_to_text);
    let count = object.get("tools").and_then(Value::as_array).map(Vec::len);
    let detail = match (status, count) {
        (Some(status), Some(count)) => Some(format!("{status}, {count} tool(s)")),
        (Some(status), None) => Some(status),
        (None, Some(count)) => Some(format!("{count} tool(s)")),
        (None, None) => object.get("execution").and_then(value_to_text),
    };
    Some(format_tool_result_summary(Some("search"), detail, false))
}

fn json_string_or_message(value: &Value) -> Option<String> {
    value_to_text(value)
        .or_else(|| value.get("message").and_then(json_string_or_message))
        .or_else(|| value.get("text").and_then(json_string_or_message))
        .or_else(|| value.get("content").and_then(json_string_or_message))
        .or_else(|| value.get("summary").and_then(json_string_or_message))
        .or_else(|| extract_text_from_value(value))
}

fn codex_exec_command_summary(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let detail = object
        .get("command")
        .or_else(|| object.get("cmd"))
        .or_else(|| object.get("argv"))
        .or_else(|| object.get("args"))
        .and_then(command_value_to_text)
        .or_else(|| object.get("action").and_then(codex_exec_command_summary));
    Some(format_tool_summary("shell", detail))
}

fn codex_exec_result_summary(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let mut parts = Vec::new();
    if let Some(exit_code) = object
        .get("exit_code")
        .or_else(|| object.get("exitCode"))
        .and_then(value_to_text)
    {
        parts.push(format!("exit {exit_code}"));
    }
    if let Some(stdout) = object.get("stdout").and_then(output_body_text) {
        parts.push(stdout);
    }
    if let Some(stderr) = object.get("stderr").and_then(output_body_text) {
        parts.push(stderr);
    }
    let detail = if parts.is_empty() {
        output_body_text(value)
    } else {
        Some(parts.join("\n"))
    };
    Some(format_tool_result_summary(Some("shell"), detail, false))
}

fn patch_summary_text(value: &Value) -> Option<String> {
    value
        .get("patch")
        .or_else(|| value.get("changes"))
        .or_else(|| value.get("summary"))
        .and_then(output_body_text)
        .or_else(|| output_body_text(value))
}

fn diff_summary_text(value: &Value) -> Option<String> {
    value
        .get("diff")
        .or_else(|| value.get("unified_diff"))
        .or_else(|| value.get("changes"))
        .and_then(output_body_text)
        .or_else(|| output_body_text(value))
}

fn guardian_assessment_summary(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let action = object.get("action").and_then(guardian_action_summary);
    let status = object.get("status").and_then(value_to_text);
    let outcome = object.get("outcome").and_then(value_to_text);
    let detail = [status, outcome, action]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    if detail.trim().is_empty() {
        Some("waiting for permission approval".to_string())
    } else {
        Some(format!("waiting for permission approval: {detail}"))
    }
}

fn guardian_action_summary(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let kind = object
        .get("type")
        .or_else(|| object.get("source"))
        .and_then(value_to_text)
        .unwrap_or_else(|| "action".to_string());
    let detail = object
        .get("command")
        .or_else(|| object.get("cmd"))
        .or_else(|| object.get("argv"))
        .and_then(command_value_to_text)
        .or_else(|| object.get("tool").and_then(value_to_text))
        .or_else(|| object.get("name").and_then(value_to_text));
    Some(match detail {
        Some(detail) if !detail.trim().is_empty() => format!("{kind}: {detail}"),
        _ => kind,
    })
}

fn gemini_tool_call_has_result(value: &Value) -> bool {
    value.get("result").is_some()
        || value.get("response").is_some()
        || value.get("functionResponse").is_some()
        || value.get("error").is_some()
}

fn gemini_tool_call_summary(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let gemini_shape = object.get("displayName").is_some()
        || object.get("args").is_some()
        || object.get("status").is_some()
        || object.get("response").is_some()
        || object.get("functionResponse").is_some();
    if !gemini_shape {
        return None;
    }
    if let Some(result) = object
        .get("result")
        .or_else(|| object.get("response"))
        .or_else(|| object.get("functionResponse"))
    {
        let is_error = object
            .get("status")
            .and_then(value_to_text)
            .map(|status| {
                matches!(
                    status.to_ascii_lowercase().as_str(),
                    "cancelled" | "canceled" | "error" | "failed"
                )
            })
            .unwrap_or(false)
            || object.get("error").is_some()
            || gemini_response_has_error(result);
        let name = object
            .get("displayName")
            .or_else(|| object.get("name"))
            .and_then(Value::as_str);
        let detail = gemini_response_text(result)
            .or_else(|| object.get("error").and_then(output_body_text))
            .or_else(|| object.get("status").and_then(value_to_text));
        return Some(format_tool_result_summary(name, detail, is_error));
    }

    let name = object
        .get("displayName")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .and_then(non_empty)?;
    let detail = object
        .get("description")
        .and_then(value_to_text)
        .or_else(|| object.get("args").and_then(argument_value_to_text));
    Some(format_tool_summary(&name, detail))
}

fn gemini_tool_result_summary(value: &Value) -> Option<String> {
    if !gemini_tool_result_like(value) {
        return None;
    }
    let name = value
        .get("displayName")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str);
    let is_error = gemini_response_has_error(value);
    let detail = gemini_response_text(value);
    Some(format_tool_result_summary(name, detail, is_error))
}

fn gemini_response_text(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(gemini_response_text)
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        Value::Object(_) => value
            .get("functionResponse")
            .and_then(gemini_response_text)
            .or_else(|| value.get("response").and_then(gemini_response_text))
            .or_else(|| value.get("error").and_then(output_body_text))
            .or_else(|| value.get("output").and_then(output_body_text))
            .or_else(|| value.get("result").and_then(output_body_text)),
        _ => output_body_text(value),
    }
}

fn gemini_tool_result_like(value: &Value) -> bool {
    value.get("functionResponse").is_some()
        || value.get("response").is_some()
        || value.get("error").is_some()
        || value.get("output").is_some()
}

fn gemini_response_has_error(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(gemini_response_has_error),
        Value::Object(_) => {
            value.get("error").is_some()
                || value
                    .get("functionResponse")
                    .is_some_and(gemini_response_has_error)
                || value.get("response").is_some_and(gemini_response_has_error)
        }
        _ => false,
    }
}

fn summarize_web_search_call_object(object: &Map<String, Value>) -> Option<String> {
    let detail = object
        .get("action")
        .and_then(web_search_action_text)
        .or_else(|| object.get("status").and_then(value_to_text));
    Some(format_tool_summary("web_search", detail))
}

fn summarize_image_generation_call_object(object: &Map<String, Value>) -> Option<String> {
    let detail = object
        .get("revised_prompt")
        .and_then(value_to_text)
        .or_else(|| object.get("status").and_then(value_to_text));
    Some(format_tool_summary("image_generation", detail))
}

fn summarize_tool_result_object(object: &Map<String, Value>) -> Option<String> {
    let content = object
        .get("content")
        .and_then(tool_result_content_text)
        .or_else(|| object.get("result").and_then(value_to_text))
        .or_else(|| object.get("output").and_then(output_body_text))
        .unwrap_or_default();
    if object
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && is_question_tool_error(&content)
    {
        return Some(format_question_waiting_summary(Some(
            content.trim().to_string(),
        )));
    }
    let prefix = if object
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool error"
    } else {
        "tool result"
    };
    if content.trim().is_empty() {
        Some(prefix.to_string())
    } else {
        Some(format!(
            "{prefix}: {}",
            sanitize_text(content.trim(), TOOL_RESULT_MAX_CHARS)
        ))
    }
}

fn summarize_output_body(
    output: Option<&Value>,
    name: Option<&str>,
    is_error: bool,
) -> Option<String> {
    let content = output.and_then(output_body_text);
    Some(format_tool_result_summary(name, content, is_error))
}

fn format_tool_summary(name: &str, detail: Option<String>) -> String {
    match detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!(
                "tool {name}: {}",
                sanitize_text(detail.trim(), TOOL_DETAIL_MAX_CHARS)
            )
        }
        _ => format!("tool {name}"),
    }
}

fn format_tool_result_summary(
    name: Option<&str>,
    detail: Option<String>,
    is_error: bool,
) -> String {
    let prefix = match (
        name.and_then(|name| non_empty(name).map(|name| sanitize_text(&name, 80))),
        is_error,
    ) {
        (Some(name), true) => format!("tool {name} error"),
        (Some(name), false) => format!("tool {name} result"),
        (None, true) => "tool error".to_string(),
        (None, false) => "tool result".to_string(),
    };
    match detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!(
                "{prefix}: {}",
                sanitize_text(detail.trim(), TOOL_RESULT_MAX_CHARS)
            )
        }
        _ => prefix,
    }
}

fn is_question_tool_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "askuserquestion" | "ask_user_question" | "request_user_input"
    )
}

fn is_permission_tool_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "request_permissions"
            | "request_permission"
            | "requestpermissions"
            | "permission_request"
            | "permissions_request"
    )
}

fn is_question_tool_error(content: &str) -> bool {
    let lower = content.trim().to_ascii_lowercase();
    lower == "answer questions?"
        || lower == "answer questions"
        || lower.contains("answer questions?")
        || lower.contains("request user input")
}

fn format_question_request_summary(detail: Option<String>) -> String {
    match detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!("question requested: {}", sanitize_text(detail.trim(), 220))
        }
        _ => "question requested".to_string(),
    }
}

fn format_question_waiting_summary(detail: Option<String>) -> String {
    match detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!(
                "waiting for user input: {}",
                sanitize_text(detail.trim(), 220)
            )
        }
        _ => "waiting for user input".to_string(),
    }
}

fn permission_request_detail_text(object: &Map<String, Value>) -> Option<String> {
    let input = object
        .get("input")
        .or_else(|| object.get("parameters"))
        .or_else(|| object.get("args"))
        .or_else(|| object.get("arguments"));
    let source = input.and_then(Value::as_object).unwrap_or(object);

    let mut parts = Vec::new();
    for key in [
        "permissions",
        "permission",
        "scopes",
        "scope",
        "access",
        "capabilities",
        "capability",
    ] {
        if let Some(text) = source.get(key).and_then(permission_value_to_text) {
            push_unique_summary_line(&mut parts, text);
        }
    }
    for key in ["reason", "rationale", "description", "message", "prompt"] {
        if let Some(text) = source.get(key).and_then(value_to_text) {
            push_unique_summary_line(&mut parts, text);
        }
    }
    if parts.is_empty() {
        tool_detail_text(object)
    } else {
        Some(parts.join(" · "))
    }
}

fn permission_value_to_text(value: &Value) -> Option<String> {
    if let Some(text) = value_to_text(value) {
        return Some(text);
    }
    let array = value.as_array()?;
    let parts = array.iter().filter_map(value_to_text).collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(" / "))
}

fn question_tool_detail_text(object: &Map<String, Value>) -> Option<String> {
    for key in ["question", "prompt", "message", "header"] {
        if let Some(text) = object.get(key).and_then(value_to_text) {
            return Some(text);
        }
    }
    if let Some(text) = object.get("questions").and_then(question_array_detail_text) {
        return Some(text);
    }

    let input = object
        .get("input")
        .or_else(|| object.get("parameters"))
        .or_else(|| object.get("args"))
        .or_else(|| object.get("arguments"))?;
    if let Some(text) = input.as_str().and_then(non_empty) {
        return Some(humanize_argument_text(&text));
    }
    let input = input.as_object()?;
    for key in ["question", "prompt", "message", "header"] {
        if let Some(text) = input.get(key).and_then(value_to_text) {
            return Some(text);
        }
    }
    if let Some(text) = input.get("questions").and_then(question_array_detail_text) {
        return Some(text);
    }
    tool_detail_text(object)
}

fn question_array_detail_text(value: &Value) -> Option<String> {
    let questions = value.as_array()?;
    let text = questions
        .iter()
        .filter_map(|question| {
            question
                .get("question")
                .or_else(|| question.get("prompt"))
                .or_else(|| question.get("header"))
                .and_then(value_to_text)
                .or_else(|| extract_text_from_value(question))
        })
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ");
    (!text.trim().is_empty()).then_some(text)
}

fn tool_result_content_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str().and_then(non_empty) {
        return Some(text);
    }
    let text = collect_text_parts(value).join("\n");
    (!text.trim().is_empty()).then_some(text)
}

fn tool_detail_text(object: &Map<String, Value>) -> Option<String> {
    for key in ["command", "cmd", "file_path", "path", "url", "query"] {
        if let Some(text) = object.get(key).and_then(command_value_to_text) {
            return Some(text);
        }
    }

    let input = object
        .get("input")
        .or_else(|| object.get("parameters"))
        .or_else(|| object.get("args"))
        .or_else(|| object.get("arguments"))?;
    if let Some(text) = input.as_str().and_then(non_empty) {
        return Some(humanize_argument_text(&text));
    }
    let input = input.as_object()?;
    for key in ["command", "cmd", "file_path", "path", "url", "query"] {
        if let Some(text) = input.get(key).and_then(command_value_to_text) {
            return Some(text);
        }
    }
    match (
        input.get("pattern").and_then(value_to_text),
        input
            .get("path")
            .or_else(|| input.get("file_path"))
            .and_then(value_to_text),
    ) {
        (Some(pattern), Some(path)) => Some(format!("{pattern} in {path}")),
        (Some(pattern), None) => Some(pattern),
        _ => None,
    }
}

fn command_value_to_text(value: &Value) -> Option<String> {
    if let Some(text) = value_to_text(value) {
        return Some(text);
    }
    let array = value.as_array()?;
    let parts = array.iter().filter_map(value_to_text).collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn argument_value_to_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str().and_then(non_empty) {
        return Some(humanize_argument_text(&text));
    }
    if let Some(object) = value.as_object() {
        if let Some(detail) = tool_detail_text(object) {
            return Some(detail);
        }
        return Some(summarize_json_value(value));
    }
    if let Some(array) = value.as_array() {
        let text = summarize_json_array(array);
        return (!text.trim().is_empty()).then_some(text);
    }
    value_to_text(value)
}

fn humanize_argument_text(text: &str) -> String {
    let trimmed = text.trim();
    if looks_like_json(trimmed) {
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(object) = value.as_object() {
                if let Some(detail) = tool_detail_text(object) {
                    return detail;
                }
            }
            return summarize_json_value(&value);
        }
    }
    trimmed.to_string()
}

fn output_body_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str().and_then(non_empty) {
        return Some(humanize_argument_text(&text));
    }
    if let Some(array) = value.as_array() {
        let text = array
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(value_to_text)
                    .or_else(|| extract_text_from_value(item))
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (!text.trim().is_empty()).then_some(text);
    }
    if value.is_object() {
        return extract_text_from_value(value).or_else(|| Some(summarize_json_value(value)));
    }
    value_to_text(value)
}

fn web_search_action_text(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let action = object
        .get("type")
        .and_then(value_to_text)
        .unwrap_or_else(|| "search".into());
    if let Some(query) = object.get("query").and_then(value_to_text) {
        return Some(format!("{action}: {query}"));
    }
    if let Some(queries) = object.get("queries").and_then(Value::as_array) {
        let queries = queries
            .iter()
            .filter_map(value_to_text)
            .collect::<Vec<_>>()
            .join(", ");
        if !queries.trim().is_empty() {
            return Some(format!("{action}: {queries}"));
        }
    }
    if let Some(url) = object.get("url").and_then(value_to_text) {
        if let Some(pattern) = object.get("pattern").and_then(value_to_text) {
            return Some(format!("{action}: {pattern} in {url}"));
        }
        return Some(format!("{action}: {url}"));
    }
    Some(action)
}

fn value_array_texts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|item| value_to_text(item).or_else(|| extract_text_from_value(item)))
                .map(|text| sanitize_text(text.trim(), 220))
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty(text),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn summarize_json_array(array: &[Value]) -> String {
    let parts = array
        .iter()
        .filter_map(extract_text_from_value)
        .take(6)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        format!("{} item(s)", array.len())
    } else {
        parts.join(" | ")
    }
}

fn summarize_json_code_fence(s: &str) -> Option<String> {
    let fence_start = s.find("```")?;
    let after_open = &s[fence_start + 3..];
    let content_start = after_open.find('\n').map(|idx| idx + 1).unwrap_or(0);
    let after_content_start = &after_open[content_start..];
    let fence_end = after_content_start.find("```")?;
    let raw = after_content_start[..fence_end].trim();
    if !looks_like_json(raw) {
        return None;
    }
    serde_json::from_str::<Value>(raw)
        .ok()
        .map(|value| summarize_json_value(&value))
}

fn summarize_angle_role_block(s: &str, max: usize) -> Option<String> {
    let (head, body) = s.split_once('\n')?;
    let role = head.trim().strip_suffix('>')?.trim();
    if !matches!(role, "planner" | "critic" | "reviewer" | "worker" | "tool") {
        return None;
    }
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let summary = humanize_jsonish(body, max);
    Some(format!("{role}: {summary}"))
}

fn summarize_jsonish_lines(s: &str) -> Option<String> {
    let lines = s
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() < 2 {
        return None;
    }

    let mut parsed = 0usize;
    let mut out = Vec::new();
    for line in lines.iter().take(24) {
        if let Some(summary) = summarize_jsonish_line(line) {
            parsed += 1;
            push_unique_summary_line(&mut out, summary);
        } else {
            push_unique_summary_line(&mut out, sanitize_text(line, 240));
        }
    }
    if lines.len() > 24 {
        out.push(format!("… {} more line(s)", lines.len() - 24));
    }

    (parsed > 0).then(|| out.join("\n"))
}

fn summarize_jsonish_line(line: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
        return Some(summarize_json_value(&value));
    }
    if let Some((prefix, raw)) = line.split_once(": ") {
        let raw = raw.trim();
        if looks_like_json(raw) {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                return Some(format!(
                    "{}: {}",
                    prefix.trim(),
                    summarize_json_value(&value)
                ));
            }
        }
    }
    summarize_embedded_json_with_context(line)
}

fn push_unique_summary_line(out: &mut Vec<String>, line: String) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if out.last().map(String::as_str) != Some(line) {
        out.push(line.to_string());
    }
}

fn summarize_embedded_json_with_context(s: &str) -> Option<String> {
    let start = s
        .char_indices()
        .find_map(|(idx, ch)| (ch == '{').then_some(idx))?;
    let mut iter = serde_json::Deserializer::from_str(&s[start..]).into_iter::<Value>();
    let value = iter.next()?.ok()?;
    let end = start + iter.byte_offset();
    let summary = summarize_json_value(&value);
    let prefix = s[..start].trim();
    let suffix = s[end..].trim();
    match (prefix.is_empty(), suffix.is_empty()) {
        (true, true) => Some(summary),
        (false, true) => Some(format!("{prefix} {summary}")),
        (true, false) => Some(format!("{summary} {suffix}")),
        (false, false) => Some(format!("{prefix} {summary} {suffix}")),
    }
}

fn summarize_loose_structured_json(s: &str) -> Option<String> {
    summarize_loose_review_json(s).or_else(|| summarize_loose_plan_json(s))
}

fn summarize_loose_review_json(s: &str) -> Option<String> {
    if !s.contains("approved") || !(s.contains("issues") || s.contains("suggestions")) {
        return None;
    }
    let approved = find_loose_bool_field(s, "approved")?;
    let issues = extract_loose_string_array(s, "issues");
    let suggestions = extract_loose_string_array(s, "suggestions");
    let visible_issue_count = if issues.is_empty() && !suggestions.is_empty() && !approved {
        suggestions.len()
    } else {
        issues.len()
    };
    let mut out = if approved {
        format!("approved: {} issue(s)", issues.len())
    } else {
        format!("needs changes: {visible_issue_count} issue(s)")
    };

    for issue in issues.iter().take(6) {
        out.push_str("\n  - ");
        out.push_str(&sanitize_text(issue, 180));
    }
    if issues.len() > 6 {
        out.push_str(&format!("\n  … {} more issue(s)", issues.len() - 6));
    }
    if !suggestions.is_empty() {
        out.push_str("\n  suggestions:");
        for suggestion in suggestions.iter().take(3) {
            out.push_str("\n  - ");
            out.push_str(&sanitize_text(suggestion, 180));
        }
    }
    Some(out)
}

fn summarize_loose_plan_json(s: &str) -> Option<String> {
    if !s.contains("sub_tasks") {
        return None;
    }
    if !s.contains('{') && !s.contains('[') {
        return None;
    }
    let prompts = extract_loose_repeated_string_field(s, "prompt");
    let agents = extract_loose_repeated_string_field(s, "agent_hint")
        .into_iter()
        .chain(extract_loose_repeated_string_field(s, "agent"))
        .collect::<Vec<_>>();
    if prompts.is_empty() {
        return Some("plan ready: worker run(s)".into());
    }
    let mut out = format!("plan ready: {} worker run(s)", prompts.len());
    for (idx, prompt) in prompts.iter().take(6).enumerate() {
        out.push_str(&format!("\n  {}. {}", idx + 1, sanitize_text(prompt, 140)));
        if let Some(agent) = agents.get(idx).filter(|agent| !agent.trim().is_empty()) {
            out.push_str(" · ");
            out.push_str(&sanitize_text(agent, 48));
        }
    }
    if prompts.len() > 6 {
        out.push_str(&format!("\n  … {} more", prompts.len() - 6));
    }
    Some(out)
}

fn find_loose_bool_field(s: &str, key: &str) -> Option<bool> {
    let idx = find_loose_key(s, key)?;
    let after_key = &s[idx + key.len() + 2..];
    let after_colon = after_key.split_once(':')?.1.trim_start();
    if after_colon.starts_with("true") {
        Some(true)
    } else if after_colon.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_loose_repeated_string_field(s: &str, key: &str) -> Vec<String> {
    let mut rest = s;
    let mut values = Vec::new();
    while let Some(idx) = find_loose_key(rest, key) {
        let after_key = &rest[idx + key.len() + 2..];
        let Some((_, after_colon)) = after_key.split_once(':') else {
            break;
        };
        let trimmed = after_colon.trim_start();
        if let Some((value, consumed)) = extract_loose_string(trimmed) {
            values.push(value);
            rest = &trimmed[consumed..];
        } else {
            rest = trimmed;
        }
    }
    values
}

fn extract_loose_string_array(s: &str, key: &str) -> Vec<String> {
    let Some(idx) = find_loose_key(s, key) else {
        return Vec::new();
    };
    let after_key = &s[idx + key.len() + 2..];
    let Some((_, after_colon)) = after_key.split_once(':') else {
        return Vec::new();
    };
    let Some(array_start) = after_colon.find('[') else {
        return Vec::new();
    };
    let mut rest = &after_colon[array_start + 1..];
    let mut values = Vec::new();
    loop {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(']') {
            break;
        }
        if let Some((value, consumed)) = extract_loose_string(trimmed) {
            values.push(value);
            rest = &trimmed[consumed..];
            continue;
        }
        if let Some(next_quote) = trimmed.find('"') {
            rest = &trimmed[next_quote..];
        } else {
            break;
        }
    }
    values
}

fn extract_loose_string(s: &str) -> Option<(String, usize)> {
    let start = s.find('"')?;
    let mut out = String::new();
    let mut escape = false;
    for (offset, ch) in s[start + 1..].char_indices() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' => escape = true,
            '"' => {
                let consumed = start + 1 + offset + ch.len_utf8();
                return Some((normalize_loose_string(&out), consumed));
            }
            _ => out.push(ch),
        }
    }
    (!out.trim().is_empty()).then(|| (normalize_loose_string(&out), s.len()))
}

fn normalize_loose_string(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_loose_key(s: &str, key: &str) -> Option<usize> {
    s.find(&format!("\"{key}\""))
}

fn looks_like_json(s: &str) -> bool {
    let s = s.trim_start();
    s.starts_with('{') || s.starts_with('[')
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_claude_stream_text_blocks() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "planning step one" }
                ]
            }
        })
        .to_string();

        let summary = summarize_cli_stream_line(&line, 200).unwrap();

        assert_eq!(summary.kind, "assistant");
        assert_eq!(summary.text, "planning step one");
    }

    #[test]
    fn claude_stream_system_model_metadata_is_not_an_answer() {
        let line = serde_json::json!({
            "type": "system",
            "subtype": "init",
            "model": "glm-5.1",
            "session_id": "4546ea8f-efd9-4a1a-a1f5-29d3be5cc61c"
        })
        .to_string();

        let summary = summarize_cli_stream_line(&line, 200).unwrap();

        assert_eq!(classify_json_line(&line), "system");
        assert_eq!(summary.kind, "system");
        assert_eq!(summary.text, "system");
        assert!(!summary.text.contains("glm-5.1"));
    }

    #[test]
    fn claude_stream_assistant_prefers_message_content_over_model_metadata() {
        let payload = serde_json::json!({
            "type": "assistant",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "glm-5.2",
                "content": [
                    { "type": "text", "text": "Tiffany adapter Claude runtime OK" }
                ]
            },
            "session_id": "4546ea8f-efd9-4a1a-a1f5-29d3be5cc61c"
        });

        let summary = summarize_event_value(&payload, 200).unwrap();
        let text = format_runtime_output("claude-code", "assistant", &payload, 200);

        assert_eq!(summary.kind, "assistant");
        assert_eq!(summary.text, "Tiffany adapter Claude runtime OK");
        assert_eq!(
            text,
            "claude-code assistant: Tiffany adapter Claude runtime OK"
        );
        assert!(!text.contains("glm-5.2"));
    }

    #[test]
    fn extracts_all_text_blocks_from_nested_content() {
        let value = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "first paragraph" },
                    { "type": "text", "text": "second paragraph" }
                ]
            }
        });

        let text = extract_text_from_value(&value).expect("text");

        assert_eq!(text, "first paragraph\nsecond paragraph");
    }

    #[test]
    fn summarizes_codex_style_result_lines() {
        let line = serde_json::json!({
            "type": "turn_complete",
            "result": "Implemented the patch"
        })
        .to_string();

        let summary = summarize_cli_stream_line(&line, 200).unwrap();

        assert_eq!(summary.kind, "turn_complete");
        assert_eq!(summary.text, "Implemented the patch");
    }

    #[test]
    fn summarizes_structured_role_json_stream_lines() {
        let plan = summarize_cli_stream_line(
            r#"{"sub_tasks":[{"prompt":"fix display","agent_hint":"worker-cc"}]}"#,
            500,
        )
        .unwrap();
        assert_eq!(plan.kind, "event");
        assert!(plan.text.contains("plan ready: 1 worker run(s)"));
        assert!(plan.text.contains("agent: worker-cc"));

        let review =
            summarize_cli_stream_line(r#"{"approved":false,"issues":["raw JSON leaked"]}"#, 500)
                .unwrap();
        assert_eq!(review.kind, "event");
        assert!(review.text.contains("needs changes: 1 issue(s)"));
        assert!(review.text.contains("raw JSON leaked"));
    }

    #[test]
    fn formats_runtime_payloads_without_raw_json() {
        let payload = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{ "type": "text", "text": "Working on it" }] }
        });

        let text = format_runtime_output("claude-code", "assistant", &payload, 200);

        assert_eq!(text, "claude-code assistant: Working on it");
        assert!(!text.contains('{'));
    }

    #[test]
    fn formats_nested_tool_use_and_result_blocks() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "tool_use", "name": "Bash", "input": { "command": "cargo test" } },
                    { "type": "tool_result", "content": "tests passed", "is_error": false }
                ]
            }
        })
        .to_string();

        let summary = summarize_cli_stream_line(&line, 500).unwrap();

        assert_eq!(summary.kind, "tool_result");
        assert!(summary.text.contains("tool Bash: cargo test"));
        assert!(summary.text.contains("tool result: tests passed"));
        assert!(!summary.text.contains('{'));
    }

    #[test]
    fn classifies_claude_nested_tool_events_as_worker_process_events() {
        let tool_call = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "Bash",
                        "input": {
                            "command": "cargo test",
                            "description": "Run tests"
                        }
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&tool_call), "tool_use");
        let summary = summarize_cli_stream_line(&tool_call, 500).unwrap();
        assert_eq!(summary.kind, "tool_use");
        assert_eq!(summary.text, "tool Bash: cargo test");

        let tool_result = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": "tests passed",
                        "is_error": false
                    }
                ]
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&tool_result), "tool_result");
        let summary = summarize_cli_stream_line(&tool_result, 500).unwrap();
        assert_eq!(summary.kind, "tool_result");
        assert_eq!(summary.text, "tool result: tests passed");
    }

    #[test]
    fn unwraps_codex_event_wrappers_without_raw_json() {
        let agent_message = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": "Working on the fix",
                "phase": "commentary"
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&agent_message), "assistant");
        let summary = summarize_cli_stream_line(&agent_message, 500).unwrap();
        assert_eq!(summary.kind, "assistant");
        assert_eq!(summary.text, "Working on the fix");
        assert!(!summary.text.contains('{'));

        let completed_agent_message = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_0",
                "type": "agent_message",
                "text": "Tiffany adapter Codex runtime OK"
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&completed_agent_message), "assistant");
        let summary = summarize_cli_stream_line(&completed_agent_message, 500).unwrap();
        assert_eq!(summary.kind, "assistant");
        assert_eq!(summary.text, "Tiffany adapter Codex runtime OK");
        assert!(!summary.text.contains("item.completed"));
        assert!(!summary.text.contains('{'));

        let call = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"cargo test -q\"}",
                "call_id": "call_1"
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&call), "tool_use");
        let summary = summarize_cli_stream_line(&call, 500).unwrap();
        assert_eq!(summary.kind, "tool_use");
        assert_eq!(summary.text, "tool exec_command: cargo test -q");

        let output = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "tests passed"
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&output), "tool_result");
        let summary = summarize_cli_stream_line(&output, 500).unwrap();
        assert_eq!(summary.kind, "tool_result");
        assert_eq!(summary.text, "tool result: tests passed");
    }

    #[test]
    fn unwraps_codex_rollout_event_msg_fields_without_raw_json() {
        let answer = serde_json::json!({
            "timestamp": "2026-06-26T00:00:00Z",
            "item": {
                "event_msg": {
                    "agent_message": { "message": "我会检查代码。" }
                }
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&answer), "assistant");
        let summary = summarize_cli_stream_line(&answer, 500).unwrap();
        assert_eq!(summary.kind, "assistant");
        assert_eq!(summary.text, "我会检查代码。");
        assert!(!summary.text.contains("structured output"));
        assert!(!summary.text.contains('{'));

        let command = serde_json::json!({
            "timestamp": "2026-06-26T00:00:01Z",
            "item": {
                "event_msg": {
                    "exec_command_begin": { "command": ["cargo", "test", "--all"] }
                }
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&command), "tool_use");
        let summary = summarize_cli_stream_line(&command, 500).unwrap();
        assert_eq!(summary.kind, "tool_use");
        assert_eq!(summary.text, "tool shell: cargo test --all");
        assert!(!summary.text.contains('{'));

        let result = serde_json::json!({
            "timestamp": "2026-06-26T00:00:02Z",
            "item": {
                "event_msg": {
                    "exec_command_end": {
                        "exit_code": 0,
                        "stdout": "ok"
                    }
                }
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&result), "tool_result");
        let summary = summarize_cli_stream_line(&result, 500).unwrap();
        assert_eq!(summary.kind, "tool_result");
        assert!(summary.text.contains("tool shell result"));
        assert!(summary.text.contains("exit 0"));
        assert!(summary.text.contains("ok"));
        assert!(!summary.text.contains('{'));

        let diff = serde_json::json!({
            "timestamp": "2026-06-26T00:00:03Z",
            "item": {
                "event_msg": {
                    "turn_diff": { "diff": "diff --git a/lib.rs b/lib.rs\n+done" }
                }
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&diff), "diff");
        let summary = summarize_cli_stream_line(&diff, 500).unwrap();
        assert_eq!(summary.kind, "diff");
        assert!(summary.text.contains("diff --git"));
        assert!(!summary.text.contains("turn_diff"));
    }

    #[test]
    fn summarizes_codex_guardian_approval_without_raw_json() {
        let event = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "guardian_assessment",
                "status": "in_progress",
                "action": {
                    "type": "command",
                    "source": "shell",
                    "command": "rm -rf /tmp/guardian"
                }
            }
        })
        .to_string();

        assert_eq!(classify_json_line(&event), "approval");
        let summary = summarize_cli_stream_line(&event, 500).unwrap();
        assert_eq!(summary.kind, "approval");
        assert_eq!(
            summary.text,
            "waiting for permission approval: in_progress · command: rm -rf /tmp/guardian"
        );
        assert!(!summary.text.contains('{'));

        let visible = visible_agent_output(&format!("codex approval: {}", summary.text), 500)
            .expect("visible approval");
        assert_eq!(visible.kind, VisibleAgentOutputKind::Approval);
        assert!(visible.display.contains("waiting for permission approval"));
    }

    #[test]
    fn summarizes_gemini_tool_calls_and_function_responses_without_raw_json() {
        let call = serde_json::json!({
            "type": "gemini",
            "content": "我会检查文件。",
            "toolCalls": [
                {
                    "name": "read_file",
                    "displayName": "Read",
                    "args": { "file_path": "/tmp/repo/README.md" }
                }
            ]
        })
        .to_string();

        assert_eq!(classify_json_line(&call), "tool_use");
        let summary = summarize_cli_stream_line(&call, 500).unwrap();
        assert_eq!(summary.kind, "tool_use");
        assert_eq!(summary.text, "tool Read: /tmp/repo/README.md");
        assert!(!summary.text.contains('{'));

        let result = serde_json::json!({
            "toolResults": [
                {
                    "name": "read_file",
                    "displayName": "Read",
                    "functionResponse": {
                        "response": {
                            "output": "README body"
                        }
                    }
                }
            ]
        })
        .to_string();

        assert_eq!(classify_json_line(&result), "tool_result");
        let summary = summarize_cli_stream_line(&result, 500).unwrap();
        assert_eq!(summary.kind, "tool_result");
        assert_eq!(summary.text, "tool Read result: README body");
        assert!(!summary.text.contains('{'));

        let cancelled = serde_json::json!({
            "toolCalls": [
                {
                    "name": "replace",
                    "displayName": "Edit",
                    "status": "cancelled",
                    "result": [
                        {
                            "functionResponse": {
                                "response": {
                                    "error": "[Operation Cancelled] Reason: User did not allow tool call"
                                }
                            }
                        }
                    ]
                }
            ]
        })
        .to_string();

        assert_eq!(classify_json_line(&cancelled), "tool_result");
        let summary = summarize_cli_stream_line(&cancelled, 500).unwrap();
        assert_eq!(summary.kind, "tool_result");
        assert_eq!(
            summary.text,
            "tool Edit error: [Operation Cancelled] Reason: User did not allow tool call"
        );
        assert!(!summary.text.contains("functionResponse"));
    }

    #[test]
    fn formats_runtime_tool_payloads_without_raw_json() {
        let payload = serde_json::json!({
            "type": "tool_use",
            "name": "Read",
            "input": { "file_path": "README.md" }
        });

        let text = format_runtime_output("claude-code", "tool_use", &payload, 200);

        assert_eq!(text, "claude-code tool_use: tool Read: README.md");
        assert_eq!(normalize_output_summary(&text), "tool Read: README.md");
        assert!(!text.contains('{'));
    }

    #[test]
    fn formats_codex_response_tool_items_without_raw_json() {
        let shell = summarize_cli_stream_line(
            &serde_json::json!({
                "type": "local_shell_call",
                "status": "in_progress",
                "action": {
                    "type": "exec",
                    "command": ["cargo", "test", "--all"]
                }
            })
            .to_string(),
            500,
        )
        .unwrap();

        assert_eq!(shell.kind, "tool_use");
        assert_eq!(shell.text, "tool shell: cargo test --all");
        assert!(!shell.text.contains('{'));

        let call = summarize_cli_stream_line(
            &serde_json::json!({
                "type": "function_call",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"ls -la\"}",
                "call_id": "call_1"
            })
            .to_string(),
            500,
        )
        .unwrap();

        assert_eq!(call.kind, "tool_use");
        assert_eq!(call.text, "tool exec_command: ls -la");
        assert!(!call.text.contains('{'));

        let output = summarize_cli_stream_line(
            &serde_json::json!({
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [{ "type": "input_text", "text": "tests passed" }]
            })
            .to_string(),
            500,
        )
        .unwrap();

        assert_eq!(output.kind, "tool_result");
        assert_eq!(output.text, "tool result: tests passed");
        assert!(!output.text.contains('{'));
    }

    #[test]
    fn formats_codex_search_and_web_tool_items() {
        let search = summarize_cli_stream_line(
            &serde_json::json!({
                "type": "tool_search_call",
                "status": "in_progress",
                "execution": "search available tools",
                "arguments": { "query": "github" }
            })
            .to_string(),
            500,
        )
        .unwrap();

        assert_eq!(search.text, "tool search: search available tools");

        let web = summarize_cli_stream_line(
            &serde_json::json!({
                "type": "web_search_call",
                "status": "in_progress",
                "action": { "type": "search", "query": "Agent Client Protocol" }
            })
            .to_string(),
            500,
        )
        .unwrap();

        assert_eq!(web.text, "tool web_search: search: Agent Client Protocol");
        assert!(!web.text.contains('{'));
    }

    #[test]
    fn extracts_final_output_from_agent_prefixed_result() {
        let result = final_output_candidate(
            "codex turn_complete: {\"result\":\"Done and verified.\"}",
            200,
        )
        .unwrap();

        assert_eq!(result, "Done and verified.");
    }

    #[test]
    fn extracts_final_output_from_plain_final_heading() {
        assert_eq!(
            final_output_candidate("Final result\n你好！\n我可以帮你写代码。", 500).as_deref(),
            Some("你好！\n我可以帮你写代码。")
        );
        assert_eq!(
            final_output_candidate("claude-code assistant: Final answer:\n完成并验证。", 500)
                .as_deref(),
            Some("完成并验证。")
        );
    }

    #[test]
    fn cleans_visible_agent_output_with_shared_runtime_prefix_rules() {
        assert_eq!(
            clean_visible_agent_output(
                "claude-code system: system\nthinking\nclaude-code assistant: Final result\n你好！",
                500
            )
            .as_deref(),
            Some("你好！")
        );
        assert_eq!(
            clean_visible_agent_output("codex turn_complete: {\"result\":\"完成并验证。\"}", 500)
                .as_deref(),
            Some("完成并验证。")
        );
        assert_eq!(
            clean_visible_agent_output(
                "codex local_shell_call: {\"type\":\"local_shell_call\",\"action\":{\"command\":[\"cargo\",\"test\"]}}",
                500
            )
            .as_deref(),
            Some("tool shell: cargo test")
        );
        assert_eq!(
            runtime_output_kind("codex turn_complete: done"),
            Some("turn_complete".to_string())
        );
        assert_eq!(
            runtime_output_kind("worker-codex stderr: [1211] 模型不存在"),
            Some("stderr".to_string())
        );
        assert_eq!(
            runtime_output_kind("claude assistant-message: done"),
            Some("assistant".to_string())
        );
        assert_eq!(
            clean_visible_agent_output(
                "codex patch-apply: *** Begin Patch\n*** Update File: README.md",
                500
            )
            .as_deref(),
            Some("*** Begin Patch\n*** Update File: README.md")
        );
    }

    #[test]
    fn classifies_visible_agent_output_for_ui_surfaces() {
        assert_eq!(
            visible_agent_output_kind_for_event_kind("assistant_message"),
            Some(VisibleAgentOutputKind::Answer)
        );
        assert_eq!(
            visible_agent_output_kind_for_event_kind("exec_command"),
            Some(VisibleAgentOutputKind::ToolCall)
        );
        assert_eq!(
            visible_agent_output_kind_for_event_kind("patch_apply"),
            Some(VisibleAgentOutputKind::Patch)
        );
        assert_eq!(
            visible_agent_output_kind_for_event_kind("file_write"),
            Some(VisibleAgentOutputKind::FileUpdate)
        );

        let final_output =
            visible_agent_output("codex turn_complete: {\"result\":\"完成并验证。\"}", 500)
                .expect("final output");
        assert_eq!(final_output.kind, VisibleAgentOutputKind::Final);
        assert_eq!(final_output.display, "完成并验证。");
        assert_eq!(final_output.dedupe_key, "完成并验证。");

        let error = visible_agent_output(
            "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]",
            500,
        )
        .expect("actionable error");
        assert_eq!(error.kind, VisibleAgentOutputKind::Stderr);
        assert!(error.display.contains("模型不存在"));
        assert!(
            visible_agent_output(
                "codex stderr: WARNING: proceeding, even though we could not create PATH aliases: Operation not permitted (os error 1)",
                500,
            )
            .is_none(),
            "Codex startup alias warning should not pollute worker waterfall"
        );
        assert!(!error.display.contains('{'));

        let worker_error = visible_agent_output(
            "worker-codex stderr: API Error: 400 [1211][模型不存在]",
            500,
        )
        .expect("worker role stderr");
        assert_eq!(worker_error.kind, VisibleAgentOutputKind::Stderr);
        assert!(worker_error.display.contains("模型不存在"));

        let tool_call =
            visible_agent_output("codex local_shell_call: tool shell: cargo test --all", 500)
                .expect("tool call");
        assert_eq!(tool_call.kind, VisibleAgentOutputKind::ToolCall);
        assert_eq!(tool_call.kind.label(), "tool call");
        assert_eq!(tool_call.display, "tool shell: cargo test --all");
        let internal_tool_selection_content =
            "claude-code tool_use: tool ToolSearch: select:WebFetch";
        let internal_tool_selection = visible_agent_output(internal_tool_selection_content, 500);
        assert!(
            internal_tool_selection.is_none(),
            "internal tool selection should not pollute the worker waterfall"
        );

        let tool_result =
            visible_agent_output("claude-code tool_result: tool result: tests passed", 500)
                .expect("tool result");
        assert_eq!(tool_result.kind, VisibleAgentOutputKind::ToolResult);
        assert_eq!(tool_result.kind.label(), "tool result");
        assert_eq!(tool_result.display, "tests passed");

        let user_tool_result =
            visible_agent_output("claude-code user: tool result: tests passed", 500)
                .expect("user tool result");
        assert_eq!(user_tool_result.kind, VisibleAgentOutputKind::ToolResult);
        assert_eq!(user_tool_result.display, "tests passed");
        assert!(visible_agent_output("claude-code user: tool result", 500).is_none());

        let recovery = visible_agent_output(
            "claude-code status: native session busy · waiting for saved session native-busy and retrying once with the same native session",
            500,
        )
        .expect("recovery status");
        assert_eq!(recovery.kind, VisibleAgentOutputKind::Actionable);
        assert!(recovery.display.contains("native session busy"));
        assert!(recovery.display.contains("same native session"));

        let missing_recovery = visible_agent_output(
            "claude-code status: native session missing · cleared saved session native-missing and retrying once with a fresh native session",
            500,
        )
        .expect("missing recovery status");
        assert_eq!(missing_recovery.kind, VisibleAgentOutputKind::Actionable);
        assert!(missing_recovery.display.contains("native session missing"));

        let process_exit = visible_agent_output(
            "claude-code process_exit: claude exited with status exit status: 1",
            500,
        )
        .expect("process exit");
        assert_eq!(process_exit.kind, VisibleAgentOutputKind::Actionable);

        assert!(
            visible_agent_output(
                "claude-code process_exit: claude exited with status exit status: 0",
                500,
            )
            .is_none(),
            "successful native process exits should not add waterfall noise"
        );
        assert!(
            visible_agent_output("codex process_exit: codex exited with status 0", 500).is_none(),
            "successful native process exits should be hidden across runtimes"
        );

        let question_call = visible_agent_output("claude-code tool_use: tool AskUserQuestion", 500)
            .expect("question call");
        assert_eq!(question_call.kind, VisibleAgentOutputKind::Question);
        assert_eq!(question_call.kind.label(), "question");
        assert_eq!(
            question_call.display,
            "waiting for user input: Answer questions?"
        );
        let question_payload = serde_json::json!({
            "type": "tool_use",
            "name": "AskUserQuestion",
            "input": {
                "questions": [
                    { "question": "想了解比赛规则，还是直接写参赛 agent？" }
                ]
            }
        });
        let question_payload_line = format!("claude-code tool_use: {question_payload}");
        let question_with_detail = visible_agent_output(&question_payload_line, 500);
        let question_with_detail = question_with_detail.expect("question payload");
        assert_eq!(question_with_detail.kind, VisibleAgentOutputKind::Question);
        assert_eq!(
            question_with_detail.display,
            "question requested: 想了解比赛规则，还是直接写参赛 agent？"
        );

        let question_error = visible_agent_output(
            "claude-code tool_result: tool error: Answer questions?",
            500,
        )
        .expect("question wait");
        assert_eq!(question_error.kind, VisibleAgentOutputKind::Question);
        assert_eq!(
            question_error.display,
            "waiting for user input: Answer questions?"
        );
        assert_eq!(question_call.dedupe_key, question_error.dedupe_key);

        let permissions = visible_agent_output(
            "codex tool_use: tool request_permissions: network access and write access",
            500,
        )
        .expect("permissions wait");
        assert_eq!(permissions.kind, VisibleAgentOutputKind::Approval);
        assert_eq!(permissions.kind.label(), "approval");
        assert_eq!(
            permissions.display,
            "waiting for permission approval: network access and write access"
        );
        let permission_payload = serde_json::json!({
            "type": "tool_use",
            "name": "request_permissions",
            "input": {
                "permissions": ["network access", "write access"],
                "reason": "fetch Kaggle competition page"
            }
        });
        let permission_payload_line = format!("codex tool_use: {permission_payload}");
        let permission_payload_visible = visible_agent_output(&permission_payload_line, 500);
        let permission_payload_visible =
            permission_payload_visible.expect("permission payload approval");
        assert_eq!(
            permission_payload_visible.display,
            "waiting for permission approval: network access / write access · fetch Kaggle competition page"
        );

        let command_approval = visible_agent_output(
            "claude-code status: command requires approval: rm -rf target",
            500,
        )
        .expect("command approval wait");
        assert_eq!(command_approval.kind, VisibleAgentOutputKind::Approval);
        assert_eq!(
            command_approval.display,
            "waiting for command approval: rm -rf target"
        );

        let diff = visible_agent_output(
            "claude-code diff: files changed:\n  - src/lib.rs\n\ndiff --git a/src/lib.rs b/src/lib.rs",
            500,
        )
        .expect("worker diff");
        assert_eq!(diff.kind, VisibleAgentOutputKind::Diff);
        assert_eq!(diff.kind.label(), "diff");
        assert!(diff.display.contains("files changed:"));
        assert!(diff.display.contains("diff --git"));

        let patch = visible_agent_output(
            "codex patch: *** Begin Patch\n*** Update File: README.md",
            500,
        )
        .expect("worker patch");
        assert_eq!(patch.kind, VisibleAgentOutputKind::Patch);
        assert_eq!(patch.kind.label(), "patch");

        let file_update = visible_agent_output("gemini file_update: file updated: src/lib.rs", 500)
            .expect("file update");
        assert_eq!(file_update.kind, VisibleAgentOutputKind::FileUpdate);
        assert_eq!(file_update.kind.label(), "file update");

        let answer =
            visible_agent_output("claude assistant: useful summary", 500).expect("answer output");
        assert_eq!(answer.kind, VisibleAgentOutputKind::Answer);
        assert_eq!(answer.kind.label(), "answer");
        assert_eq!(answer.display, "useful summary");
        assert_eq!(answer.dedupe_key, "useful summary");

        let assistant_alert = visible_agent_output(
            "claude-code assistant: permission denied while writing file",
            500,
        )
        .expect("assistant alert");
        assert_eq!(assistant_alert.kind, VisibleAgentOutputKind::Actionable);
        assert_eq!(assistant_alert.kind.label(), "alert");

        assert!(visible_agent_output("claude-code assistant: thinking", 500).is_none());
    }

    #[test]
    fn normalizes_claude_tool_status_wrappers() {
        let system_note =
            visible_agent_output("claude-code system: Create venv and install deps", 500)
                .expect("system note");
        assert_eq!(system_note.kind, VisibleAgentOutputKind::Normal);
        assert_eq!(system_note.display, "Create venv and install deps");

        let created = visible_agent_output(
            "claude-code user: tool result: File created successfully at: /tmp/agent.py (file state is current in your context -- no need to Read it back)",
            500,
        )
        .expect("file created");
        assert_eq!(created.kind, VisibleAgentOutputKind::FileUpdate);
        assert_eq!(created.display, "file created: /tmp/agent.py");

        let updated = visible_agent_output(
            "claude-code user: tool result: The file /tmp/requirements.txt has been updated successfully. (file state is current in your context -- no need to Read it back)",
            500,
        )
        .expect("file updated");
        assert_eq!(updated.kind, VisibleAgentOutputKind::FileUpdate);
        assert_eq!(updated.display, "file updated: /tmp/requirements.txt");

        assert!(visible_agent_output(
            "claude-code user: tool result: (Bash completed with no output)",
            500
        )
        .is_none());
    }

    #[test]
    fn normalizes_native_cli_event_kind_aliases() {
        let assistant = summarize_cli_stream_line(
            &serde_json::json!({
                "event": "assistant-message",
                "message": { "content": [{ "type": "text", "text": "native answer" }] }
            })
            .to_string(),
            500,
        )
        .unwrap();

        assert_eq!(assistant.kind, "assistant");
        assert_eq!(assistant.text, "native answer");

        let shell = summarize_cli_stream_line(
            &serde_json::json!({
                "kind": "exec_command",
                "command": "cargo test"
            })
            .to_string(),
            500,
        )
        .unwrap();

        assert_eq!(shell.kind, "tool_use");
        assert_eq!(shell.text, "tool shell: cargo test");
        assert_eq!(
            classify_json_line(r#"{"event_type":"patch-apply"}"#),
            "patch"
        );
    }

    #[test]
    fn preserves_long_tool_result_bodies_for_native_streams() {
        let long = format!("{}END", "x".repeat(512));
        let output = visible_agent_output(
            &format!("claude-code tool_result: tool result: {long}"),
            10_000,
        )
        .expect("tool result");

        assert_eq!(output.kind, VisibleAgentOutputKind::ToolResult);
        assert!(output.display.ends_with("END"));
        assert!(!output.display.contains('…'));
    }

    #[test]
    fn approval_requests_do_not_emit_failure_hints() {
        assert!(agent_failure_hint(
            "codex tool_use: tool request_permissions: network access",
            500
        )
        .is_none());

        let denied = agent_failure_hint("claude-code stderr: permission denied", 500)
            .expect("permission failure");
        assert_eq!(denied.category, AgentFailureCategory::Permission);
    }

    #[test]
    fn humanizes_json_code_fence_without_raw_json() {
        let display = humanize_jsonish(
            "```json\n{\"approved\":false,\"issues\":[\"missing test\"]}\n```",
            200,
        );

        assert_eq!(display, "needs changes: 1 issue(s)\n  - missing test");
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_plan_json_as_waterfall_summary() {
        let display = humanize_jsonish(
            r#"{"sub_tasks":[{"prompt":"read the code and identify the broken TUI path","agent_hint":"worker-claude"},{"prompt":"patch JSON parsing"}]}"#,
            500,
        );

        assert!(display.contains("plan ready: 2 worker run(s)"));
        assert!(display.contains("1. read the code"));
        assert!(display.contains("agent: worker-claude"));
        assert!(display.contains("2. patch JSON parsing"));
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_review_json_with_issues_and_suggestions() {
        let display = humanize_jsonish(
            r#"{"approved":false,"issues":["missing worker output","raw JSON leaked"],"suggestions":["show parsed summaries"]}"#,
            500,
        );

        assert!(display.contains("needs changes: 2 issue(s)"));
        assert!(display.contains("- missing worker output"));
        assert!(display.contains("- raw JSON leaked"));
        assert!(display.contains("suggestions:"));
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_error_json_with_message_and_code() {
        let display = humanize_jsonish(
            r#"{"error":{"message":"模型不存在，请检查模型代码。","code":1211}}"#,
            500,
        );

        assert_eq!(display, "error: 模型不存在，请检查模型代码。 (1211)");
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_jsonl_stream_as_lines() {
        let display = humanize_jsonish(
            "worker: {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"step one\"}]}}\n{\"approved\":true,\"issues\":[]}",
            500,
        );

        assert!(display.contains("worker: step one"));
        assert!(display.contains("approved: 0 issue(s)"));
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_angle_role_block_with_json_body() {
        let display = humanize_jsonish(
            "critic>\n  {\"approved\": false, \"issues\": [\"plan delegates despite direct-answer instruction\"]}",
            500,
        );

        assert_eq!(
            display,
            "critic: needs changes: 1 issue(s)\n  - plan delegates despite direct-answer instruction"
        );
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_loose_truncated_review_json() {
        let display = humanize_jsonish(
            "critic>\n  {\"approved\": false, \"issues\": [\"Self-contradictory: the prompt says 不要再拆解或委派 sub_agent,\n  直接在主对话中用中文回答\", \"Unnecessary delegation",
            800,
        );

        assert!(display.contains("critic: needs changes: 2 issue(s)"));
        assert!(display.contains("Self-contradictory"));
        assert!(display.contains("直接在主对话中用中文回答"));
        assert!(display.contains("Unnecessary delegation"));
        assert!(!display.contains('{'));
        assert!(!display.contains("\"issues\""));
    }

    #[test]
    fn humanizes_loose_review_json_with_only_suggestions() {
        let display = humanize_jsonish(
            "reviewer>\n  {\"approved\": false, \"issues\": [], \"suggestions\": [\"return a direct final answer\", \"avoid repository inspection\"",
            800,
        );

        assert!(display.contains("reviewer: needs changes: 2 issue(s)"));
        assert!(display.contains("suggestions:"));
        assert!(display.contains("return a direct final answer"));
        assert!(display.contains("avoid repository inspection"));
        assert!(!display.contains('{'));
        assert!(!display.contains("\"suggestions\""));
    }

    #[test]
    fn humanizes_loose_plan_json_with_agent_hints() {
        let display = humanize_jsonish(
            "planner>\n  {\"sub_tasks\": [{\"prompt\": \"检查 TUI 输出重复\", \"agent_hint\": \"worker-cc\"}, {\"prompt\": \"修复 Codex worker session\", \"agent_hint\": \"worker-codex\"",
            800,
        );

        assert!(display.contains("planner: plan ready: 2 worker run(s)"));
        assert!(display.contains("1. 检查 TUI 输出重复 · worker-cc"));
        assert!(display.contains("2. 修复 Codex worker session · worker-codex"));
        assert!(!display.contains('{'));
        assert!(!display.contains("agent_hint"));
    }

    #[test]
    fn normalizes_duplicate_assistant_and_result_outputs() {
        let assistant = normalized_output_key("claude assistant: useful summary", 200).unwrap();
        let result = normalized_output_key("claude result: useful summary", 200).unwrap();

        assert_eq!(assistant, result);
        assert_eq!(assistant, "useful summary");
    }

    #[test]
    fn normalizes_duplicate_tool_result_outputs() {
        let direct =
            normalized_output_key("claude-code tool_result: tool result: tests passed", 200)
                .unwrap();
        let user =
            normalized_output_key("claude-code user: tool result: tests passed", 200).unwrap();
        let output =
            normalized_output_key("claude-code output: tool result: tests passed", 200).unwrap();

        assert_eq!(direct, user);
        assert_eq!(direct, output);
        assert_eq!(direct, "tests passed");
    }

    #[test]
    fn filters_cli_noise_and_redundant_role_summaries() {
        assert!(is_low_value_output("claude-code system: system"));
        assert!(is_low_value_output("claude assistant: assistant"));
        assert!(is_low_value_output("claude-code assistant: thinking"));
        assert!(is_redundant_role_output(
            "planner",
            r#"{"sub_tasks":[{"prompt":"say hello"}]}"#,
            200
        ));
        assert!(!is_redundant_role_output(
            "reviewer",
            r#"{"approved":false,"issues":["missing output"]}"#,
            200
        ));
        assert!(is_redundant_role_output(
            "reviewer",
            "needs changes: 1 issue(s)",
            200
        ));
        assert!(!is_redundant_role_output(
            "critic",
            "needs changes: 1 issue(s)\n  - missing output",
            200
        ));
        assert!(is_redundant_role_output(
            "reviewer",
            r#"{"approved":true,"issues":[]}"#,
            200
        ));
        assert!(!is_redundant_role_output(
            "worker",
            "plan ready: not a planner status",
            200
        ));
    }

    #[test]
    fn formats_control_fallbacks_for_user_visible_surfaces() {
        let planner = control_fallback_display(
            "planner",
            "planning unavailable; using original task",
            "planner unavailable",
            200,
        );
        assert_eq!(planner.role_label, "planner");
        assert_eq!(
            planner.summary,
            "single-worker fallback · planner unavailable"
        );

        let replan = control_fallback_display(
            "planner",
            "replan fell back to original task",
            "replanner returned no usable worker runs; using original task",
            200,
        );
        assert_eq!(
            replan.summary,
            "single-worker fallback · replanner returned no usable worker runs; using original task"
        );

        let critic = control_fallback_display(
            "critic",
            "critique unavailable; continuing with current plan",
            "codex exec --cd unsupported",
            200,
        );
        assert_eq!(critic.role_label, "critic");
        assert_eq!(
            critic.summary,
            "critique unavailable; continuing with current plan · codex exec --cd unsupported"
        );
    }

    #[test]
    fn keeps_actionable_stderr_visible() {
        assert!(!is_low_value_output(
            "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]"
        ));
        assert!(humanize_jsonish(
            "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]",
            200
        )
        .contains("模型不存在"));
        assert!(!is_low_value_output(
            "claude-code stderr: authentication failed: invalid api key"
        ));
        assert!(is_low_value_output(
            "codex stderr: debug transport message without actionable details"
        ));
    }

    #[test]
    fn classifies_common_agent_failures() {
        let model = agent_failure_hint(
            "codex stderr: API Error: 400 [1211][模型不存在，请检查模型代码。]",
            500,
        )
        .expect("model hint");
        assert_eq!(model.category, AgentFailureCategory::Model);
        assert_eq!(model.title(), "model not found");
        assert!(model.evidence.contains("模型不存在"));
        assert!(model.action().contains("/role"));

        let auth = agent_failure_hint(
            "claude-code stderr: authentication failed: invalid api key",
            500,
        )
        .expect("auth hint");
        assert_eq!(auth.category, AgentFailureCategory::Auth);
        assert!(auth.action().contains("/provider"));

        let runtime = agent_failure_hint(
            "spawning claude (is it installed? check `which claude`)",
            500,
        )
        .expect("runtime hint");
        assert_eq!(runtime.category, AgentFailureCategory::Runtime);
        assert!(runtime.action().contains("Install"));

        let trust = agent_failure_hint(
            "codex stderr: Not inside a trusted directory and --skip-git-repo-check was not specified.",
            500,
        )
        .expect("trust hint");
        assert_eq!(trust.category, AgentFailureCategory::Trust);
        assert!(trust.title().contains("project trust"));
        assert!(trust.action().contains("--skip-git-repo-check"));

        let parse = agent_failure_hint(
            "replanning after critique round 1: planner returned no sub_tasks (parse failed)",
            500,
        )
        .expect("parse hint");
        assert_eq!(parse.category, AgentFailureCategory::Parse);
        assert!(parse.evidence.contains("planner returned no worker runs"));
        assert!(!parse.evidence.contains("sub_tasks"));
        assert!(parse.action().contains("/process full"));

        let dependency = agent_failure_hint(
            "task DAG blocked: 1 task(s) remain behind failed dependencies",
            500,
        )
        .expect("dependency hint");
        assert_eq!(dependency.category, AgentFailureCategory::Dependency);
        assert!(dependency.title().contains("dependency blocked"));
        assert!(dependency.action().contains("fix the failed worker"));
    }

    #[test]
    fn humanizes_internal_plan_field_names_in_visible_text() {
        let display = humanize_jsonish(
            "replanning after critique round 1: planner returned no sub_tasks (parse failed)",
            500,
        );

        assert!(display.contains("planner returned no worker runs"));
        assert!(!display.contains("sub_tasks"));
    }

    #[test]
    fn classifies_direct_answer_requests_for_orchestration_surfaces() {
        assert_eq!(
            OrchestrationRoute::from_label("direct-answer"),
            Some(OrchestrationRoute::DirectAnswer)
        );
        assert_eq!(
            OrchestrationRoute::from_label("direct"),
            Some(OrchestrationRoute::DirectAnswer)
        );
        assert_eq!(
            OrchestrationRoute::from_label("single-worker"),
            Some(OrchestrationRoute::SingleWorker)
        );
        assert_eq!(
            OrchestrationRoute::from_label("single"),
            Some(OrchestrationRoute::SingleWorker)
        );
        assert_eq!(
            OrchestrationRoute::from_label("full-pipeline"),
            Some(OrchestrationRoute::FullPipeline)
        );
        assert_eq!(
            OrchestrationRoute::from_label("full"),
            Some(OrchestrationRoute::FullPipeline)
        );
        assert_eq!(OrchestrationRoute::from_label("custom"), None);
        assert_eq!(
            OrchestrationRoute::from_reason(OrchestrationRoute::DirectAnswer.reason()),
            Some(OrchestrationRoute::DirectAnswer)
        );
        assert_eq!(
            OrchestrationRoute::from_reason(OrchestrationRoute::SingleWorker.reason()),
            Some(OrchestrationRoute::SingleWorker)
        );
        assert_eq!(
            OrchestrationRoute::from_reason(OrchestrationRoute::FullPipeline.reason()),
            Some(OrchestrationRoute::FullPipeline)
        );
        assert_eq!(OrchestrationRoute::from_reason("custom reason"), None);
        assert_eq!(
            OrchestrationRoute::from_label_or_reason(
                "single-worker",
                OrchestrationRoute::FullPipeline.reason()
            ),
            Some(OrchestrationRoute::SingleWorker)
        );
        assert_eq!(
            OrchestrationRoute::from_label_or_reason(
                "custom",
                OrchestrationRoute::FullPipeline.reason()
            ),
            Some(OrchestrationRoute::FullPipeline)
        );
        assert_eq!(
            OrchestrationRoute::from_label_or_reason("custom", "custom reason"),
            None
        );
        assert_eq!(
            OrchestrationRoute::DirectAnswer.short_reason_label(),
            "chat/explain"
        );
        assert_eq!(
            OrchestrationRoute::SingleWorker.short_reason_label(),
            "atomic worker"
        );
        assert_eq!(
            OrchestrationRoute::FullPipeline.short_reason_label(),
            "implementation"
        );
        assert_eq!(
            OrchestrationRoute::reason_display_label(OrchestrationRoute::FullPipeline.reason()),
            Some("implementation")
        );
        assert_eq!(
            OrchestrationRoute::reason_display_label("custom reason"),
            None
        );

        assert!(request_looks_direct_answer("你好"));
        assert!(request_looks_direct_answer("你叫啥\n你能干啥"));
        assert!(!request_looks_direct_answer(
            "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle"
        ));
        assert!(!request_looks_direct_answer(
            "看看这个链接 https://example.com"
        ));
        assert_eq!(
            classify_orchestration_route(
                "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle"
            ),
            OrchestrationRoute::SingleWorker
        );
        assert_eq!(
            classify_orchestration_route("看看这个链接 https://example.com"),
            OrchestrationRoute::SingleWorker
        );
        assert_eq!(
            classify_orchestration_route("把 https://example.com 接入当前工程"),
            OrchestrationRoute::FullPipeline
        );
        assert!(!request_looks_direct_answer("分析一下这个比赛"));

        assert!(!request_looks_direct_answer("优化 TUI 显示"));
        assert!(!request_looks_direct_answer("fix the build error"));
        assert!(!request_looks_direct_answer("按照这个计划来做"));
        assert!(!request_looks_direct_answer("写参赛 agent"));
        assert!(!request_looks_direct_answer("创建一个 worker"));
        assert!(!request_looks_direct_answer(
            "在当前目录创建文件 tiffany_smoke.txt，内容写入 hello tiffany，然后运行 ls -la tiffany_smoke.txt 并告诉我结果。"
        ));
        assert!(contains_engineering_action("写参赛 agent"));
        assert_eq!(
            classify_orchestration_route("写参赛 agent"),
            OrchestrationRoute::SingleWorker
        );
        assert_eq!(
            classify_orchestration_route("生成一个 Python 脚手架"),
            OrchestrationRoute::SingleWorker
        );
        assert_eq!(
            classify_orchestration_route(
                "在当前目录创建文件 tiffany_smoke.txt，内容写入 hello tiffany，然后运行 ls -la tiffany_smoke.txt 并告诉我结果。"
            ),
            OrchestrationRoute::SingleWorker
        );
        assert_eq!(
            classify_orchestration_route("当前工程日志报错了"),
            OrchestrationRoute::FullPipeline
        );
    }

    #[test]
    fn classifies_contextual_prompts_by_current_request_only() {
        let contextual = "You are continuing a multi-turn tiffany-loop orchestrator conversation.\n\
Previous turns:\nuser:\n优化 TUI 显示\n\nassistant result:\n已完成提交。\n\n---\nCurrent user request:\n你叫啥";

        assert_eq!(current_user_request(contextual), "你叫啥");
        assert!(request_looks_direct_answer(contextual));

        let engineering_followup = "Previous turns:\nuser:\n你好\n\nassistant result:\n你好。\n\n---\nCurrent user request:\n继续优化 TUI";
        assert_eq!(current_user_request(engineering_followup), "继续优化 TUI");
        assert!(!request_looks_direct_answer(engineering_followup));
        assert_eq!(
            classify_orchestration_route(engineering_followup),
            OrchestrationRoute::FullPipeline
        );

        let external_atomic_followup = "Previous turns:\nuser:\n优化 tiffany-loop 编排流程\n\nassistant result:\n已完成。\n\n---\nCurrent user request:\n写参赛 agent";
        assert_eq!(
            classify_orchestration_route(external_atomic_followup),
            OrchestrationRoute::SingleWorker
        );

        let link_followup = "Previous turns:\nuser:\n你好\n\nassistant result:\n你好。\n\n---\nCurrent user request:\nhttps://www.kaggle.com/competitions/pokemon-tcg-ai-battle";
        assert_eq!(
            classify_orchestration_route(link_followup),
            OrchestrationRoute::SingleWorker
        );

        let continuation_without_project_history = "You are continuing a multi-turn tiffany-loop orchestrator conversation.\n\
Use the previous turns to resolve follow-ups.\n\n\
Previous turns:\nuser:\nhttps://www.kaggle.com/competitions/pokemon-tcg-ai-battle\n\nassistant result:\n这个比赛信息需要进一步查看。\n\n---\nCurrent user request:\n继续";
        assert_eq!(
            current_user_request(continuation_without_project_history),
            "继续"
        );
        assert_eq!(
            classify_orchestration_route(continuation_without_project_history),
            OrchestrationRoute::SingleWorker
        );

        let continuation_with_project_history = "You are continuing a multi-turn tiffany-loop orchestrator conversation.\n\
Conversation context:\nuser:\n优化当前工程编排流程\n\nassistant:\nLast run result:\n已完成第一步。\n\n---\nCurrent user request:\n继续";
        assert_eq!(
            classify_orchestration_route(continuation_with_project_history),
            OrchestrationRoute::FullPipeline
        );

        for confirmation in ["好啊", "可以啊", "行啊", "做吧", "ok", "go ahead"] {
            let prompt = format!(
                "Previous turns:\nuser:\n优化当前工程编排流程\n\nassistant result:\n下一步可以继续收敛流程。\n\n---\nCurrent user request:\n{confirmation}"
            );
            assert_eq!(
                classify_orchestration_route(&prompt),
                OrchestrationRoute::FullPipeline,
                "{confirmation} should continue project work when project context exists"
            );
        }

        let confirmation_without_project_history = "Previous turns:\nuser:\nhttps://www.kaggle.com/competitions/pokemon-tcg-ai-battle\n\nassistant result:\n可以继续研究比赛。\n\n---\nCurrent user request:\n好啊";
        assert_eq!(
            classify_orchestration_route(confirmation_without_project_history),
            OrchestrationRoute::SingleWorker
        );
        assert_eq!(
            classify_orchestration_route("你好"),
            OrchestrationRoute::DirectAnswer
        );

        let crlf = "Previous turns:\r\nuser:\r\nfix the build\r\n\r\n---\r\nCurrent user request:\r\n你能干啥\r\n";
        assert_eq!(current_user_request(crlf), "你能干啥");
        assert!(request_looks_direct_answer(crlf));

        let inline = "Previous turns:\nuser:\ncommit it\n---\nCurrent user request: 你好";
        assert_eq!(current_user_request(inline), "你好");
        assert!(request_looks_direct_answer(inline));

        let indented = "Previous turns:\nuser:\n优化\n---\n  Current user request:\n你是谁";
        assert_eq!(current_user_request(indented), "你是谁");
        assert!(request_looks_direct_answer(indented));
    }

    #[test]
    fn preserves_plain_text_around_embedded_json() {
        let display = humanize_jsonish(
            "stderr: API Error: {\"error\":{\"message\":\"模型不存在\"}} retry stopped",
            200,
        );

        assert!(display.contains("stderr: API Error:"));
        assert!(display.contains("模型不存在"));
        assert!(display.contains("retry stopped"));
        assert!(!display.contains("{\"error\""));
    }
}
