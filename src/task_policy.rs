//! Lightweight task classification used to keep chat, planning, and review behavior aligned.

use crate::agent_events;
use crate::core::types::{PlanOutput, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRoute {
    DirectAnswer,
    SingleWorker,
    FullPipeline,
}

pub fn apply_conversation_policy(top_task: &Task, plan: &mut PlanOutput) {
    if !is_conversational_task(top_task) {
        return;
    }

    let mut task = Task::new(conversation_worker_prompt(&top_task.prompt));
    task.tags = top_task.tags.clone();
    if !task.tags.iter().any(|tag| tag == "chat") {
        task.tags.push("chat".to_string());
    }
    task.worktree = top_task.worktree.clone();
    task.agent_hint = top_task.agent_hint.clone();
    task.cc_agent_hint = top_task.cc_agent_hint.clone();
    task.model_hint = top_task.model_hint.clone();
    task.model_provider_hint = top_task.model_provider_hint.clone();
    plan.sub_tasks = vec![task];
    plan.rationale = "direct conversational answer".to_string();
}

pub fn single_worker_task(top_task: &Task) -> Task {
    let mut task = Task::new(top_task.prompt.clone());
    task.tags = top_task.tags.clone();
    if !task.tags.iter().any(|tag| tag == "single_worker") {
        task.tags.push("single_worker".to_string());
    }
    task.files_of_interest = top_task.files_of_interest.clone();
    task.worktree = top_task.worktree.clone();
    task.agent_hint = top_task.agent_hint.clone();
    task.cc_agent_hint = top_task.cc_agent_hint.clone();
    task.model_hint = top_task.model_hint.clone();
    task.model_provider_hint = top_task.model_provider_hint.clone();
    task.timeout = top_task.timeout;
    task
}

pub fn review_skip_reason(top_task: &Task, task: &Task) -> Option<&'static str> {
    match classify_task_route(top_task) {
        TaskRoute::DirectAnswer => return Some("conversational answer"),
        TaskRoute::SingleWorker => return Some("single worker route"),
        TaskRoute::FullPipeline => {}
    }
    match classify_task_route(task) {
        TaskRoute::DirectAnswer => Some("conversational answer"),
        TaskRoute::SingleWorker => Some("single worker route"),
        TaskRoute::FullPipeline => None,
    }
}

pub fn conversation_worker_prompt(user_message: &str) -> String {
    format!(
        "Answer the user's message directly in the same language.\n\
         This is a conversational or explanatory request, not a request to alter the project.\n\
         Do not inspect, summarize, or mention repository state, git status, files, branches, \
         commands, or local modifications unless the user explicitly asks for that.\n\n\
         User message:\n{user_message}"
    )
}

pub fn is_conversational_task(task: &Task) -> bool {
    classify_task_route(task) == TaskRoute::DirectAnswer
}

pub fn classify_task_route(task: &Task) -> TaskRoute {
    if has_any_tag(&task.tags, ENGINEERING_TAGS) {
        return TaskRoute::FullPipeline;
    }
    if has_any_tag(&task.tags, DIRECT_TAGS) {
        return TaskRoute::DirectAnswer;
    }
    if has_any_tag(&task.tags, SINGLE_WORKER_TAGS) {
        return TaskRoute::SingleWorker;
    }

    let request = agent_events::current_user_request(&task.prompt);
    let full_prompt = task.prompt.to_lowercase();
    if has_marker(&full_prompt, CONTINUATION_ACTION_MARKERS)
        && has_marker(&full_prompt, FULL_PIPELINE_CONTEXT_MARKERS)
    {
        return TaskRoute::FullPipeline;
    }
    if looks_like_direct_answer_request(request) {
        return TaskRoute::DirectAnswer;
    }
    if looks_like_single_worker_request(request, &full_prompt) {
        return TaskRoute::SingleWorker;
    }
    TaskRoute::FullPipeline
}

const ENGINEERING_TAGS: &[&str] = &[
    "implementation",
    "implement",
    "coding",
    "code",
    "refactor",
    "bug",
    "fix",
    "test",
    "build",
    "release",
    "deploy",
];

const SINGLE_WORKER_TAGS: &[&str] = &[
    "single_worker",
    "worker_only",
    "diagnostic",
    "scaffold",
    "research",
];

const DIRECT_TAGS: &[&str] = &[
    "chat",
    "conversation",
    "conversational",
    "greeting",
    "qa",
    "question",
    "explanation",
    "advice",
];

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
    "hello",
    "hi",
    "你叫",
    "你能",
    "能干啥",
    "干啥",
    "是谁",
    "是什么",
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
    "发布",
    "接入",
    "完善",
    "整合",
    "设置",
    "fix",
    "implement",
    "create",
    "generate",
    "scaffold",
    "change",
    "edit",
    "refactor",
    "test",
    "build",
    "debug",
    "commit",
    "push",
    "release",
    "install",
    "configure",
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

const FULL_PIPELINE_CONTEXT_MARKERS: &[&str] = &[
    "当前工程",
    "这个工程",
    "当前项目",
    "这个项目",
    "项目",
    "仓库",
    "代码",
    "代码库",
    "tiffany",
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

fn looks_like_direct_answer_request(request: &str) -> bool {
    let request = request.trim();
    let lower = request.to_lowercase();
    if DIRECT_CHAT_MARKERS
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return true;
    }
    if agent_events::looks_like_link_reference(request)
        && !has_marker(&lower, IMPERATIVE_ACTION_MARKERS)
    {
        return true;
    }
    if request.ends_with('?') || request.ends_with('？') {
        return !starts_with_imperative_action(&lower);
    }
    DIRECT_QUESTION_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        && !starts_with_imperative_action(&lower)
}

fn looks_like_single_worker_request(request: &str, full_prompt_lower: &str) -> bool {
    let lower = request.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    if has_marker(&lower, DIAGNOSTIC_MARKERS)
        && !has_marker(full_prompt_lower, FULL_PIPELINE_CONTEXT_MARKERS)
    {
        return true;
    }
    if !has_marker(&lower, IMPERATIVE_ACTION_MARKERS) {
        return false;
    }
    !has_marker(full_prompt_lower, FULL_PIPELINE_CONTEXT_MARKERS)
}

fn starts_with_imperative_action(lower: &str) -> bool {
    IMPERATIVE_ACTION_MARKERS
        .iter()
        .any(|marker| lower.starts_with(marker))
}

fn has_marker(lower: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| lower.contains(marker))
}

fn has_any_tag(tags: &[String], candidates: &[&str]) -> bool {
    tags.iter()
        .map(|tag| tag.trim().to_ascii_lowercase())
        .any(|tag| candidates.iter().any(|candidate| tag == *candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_plain_chat_as_conversation() {
        assert!(is_conversational_task(&Task::new("你好")));
        assert!(is_conversational_task(&Task::new("你叫啥\n你能干啥")));
        assert!(is_conversational_task(&Task::new(
            "https://www.kaggle.com/competitions/pokemon-tcg-ai-battle"
        )));
        assert!(is_conversational_task(&Task::new(
            "看看这个链接 https://example.com"
        )));
    }

    #[test]
    fn classifies_contextual_prompt_by_current_request() {
        let prompt = "You are continuing a multi-turn terminal chat conversation.\n\
Previous turns:\nuser:\n优化 TUI 显示\n\nassistant result:\n已完成提交。\n\n---\nCurrent user request:\n你叫啥";

        assert!(is_conversational_task(&Task::new(prompt)));

        let prompt = "Previous turns:\nuser:\n你好\n\nassistant result:\n你好。\n\n---\nCurrent user request:\n继续优化 TUI";
        assert!(!is_conversational_task(&Task::new(prompt)));
    }

    #[test]
    fn contextual_continuation_keeps_project_work_in_full_pipeline() {
        let prompt = "Previous turns:\nuser:\n优化 tiffany-loop 编排流程\n\nassistant result:\n已给出计划。\n\n---\nCurrent user request:\n继续做";

        assert_eq!(
            classify_task_route(&Task::new(prompt)),
            TaskRoute::FullPipeline
        );
    }

    #[test]
    fn keeps_engineering_requests_in_full_pipeline() {
        assert!(!is_conversational_task(&Task::new("优化 TUI 显示")));
        assert!(!is_conversational_task(&Task::new("fix the build error")));
        assert!(!is_conversational_task(&Task::new("按照这个计划来做")));
        assert!(!is_conversational_task(&Task::new("写参赛 agent")));
        assert!(!is_conversational_task(&Task::new("创建一个 worker")));
        assert_eq!(
            classify_task_route(&Task::new("优化编排流程")),
            TaskRoute::FullPipeline
        );
        assert_eq!(
            classify_task_route(&Task::new("创建一个 worker")),
            TaskRoute::FullPipeline
        );
    }

    #[test]
    fn classifies_atomic_external_work_as_single_worker() {
        assert_eq!(
            classify_task_route(&Task::new("写参赛 agent")),
            TaskRoute::SingleWorker
        );
        assert_eq!(
            classify_task_route(&Task::new("生成一个 Python 脚手架")),
            TaskRoute::SingleWorker
        );
    }

    #[test]
    fn classifies_questions_about_actions_as_direct_answers() {
        assert_eq!(
            classify_task_route(&Task::new("如何提交呢")),
            TaskRoute::DirectAnswer
        );
        assert_eq!(
            classify_task_route(&Task::new("brew 怎么安装？")),
            TaskRoute::DirectAnswer
        );
    }

    #[test]
    fn classifies_diagnostics_without_project_context_as_single_worker() {
        let mut tagged = Task::new("检查一下报错");
        tagged.tags = vec!["diagnostic".to_string()];
        assert_eq!(classify_task_route(&tagged), TaskRoute::SingleWorker);
        assert_eq!(
            classify_task_route(&Task::new("安装报错了")),
            TaskRoute::SingleWorker
        );
        assert_eq!(
            classify_task_route(&Task::new("当前工程日志报错了")),
            TaskRoute::FullPipeline
        );
    }

    #[test]
    fn conversation_policy_collapses_to_one_direct_worker_task() {
        let top = Task::new("你叫啥");
        let mut plan = PlanOutput {
            sub_tasks: vec![Task::new("inspect repo"), Task::new("answer")],
            rationale: "overplanned".into(),
            estimated_cost_usd: 0.0,
        };

        apply_conversation_policy(&top, &mut plan);

        assert_eq!(plan.sub_tasks.len(), 1);
        assert_eq!(plan.sub_tasks[0].tags, vec!["chat".to_string()]);
        assert!(plan.sub_tasks[0].prompt.contains("User message:\n你叫啥"));
        assert!(plan.sub_tasks[0].prompt.contains("Do not inspect"));
    }

    #[test]
    fn review_skip_reason_names_direct_and_single_worker_routes() {
        let top = Task::new("你好");
        let task = Task::new("answer");
        assert_eq!(
            review_skip_reason(&top, &task),
            Some("conversational answer")
        );

        let top = Task::new("优化当前工程");
        let mut task = Task::new("检查报错");
        task.tags = vec!["single_worker".to_string()];
        assert_eq!(review_skip_reason(&top, &task), Some("single worker route"));

        assert_eq!(
            review_skip_reason(&Task::new("优化当前工程"), &Task::new("改代码")),
            None
        );
    }
}
