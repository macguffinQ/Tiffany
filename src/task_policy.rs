//! Lightweight task classification used to keep chat, planning, and review behavior aligned.

use crate::core::types::{PlanOutput, Task};

pub type TaskRoute = crate::agent_events::OrchestrationRoute;

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
    if let Some(reason) = classify_task_route(top_task).review_skip_reason() {
        return Some(reason);
    }
    classify_task_route(task).review_skip_reason()
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

    crate::agent_events::classify_orchestration_route(&task.prompt)
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

    #[test]
    fn route_metadata_is_user_visible_and_consistent() {
        assert_eq!(TaskRoute::DirectAnswer.label(), "direct-answer");
        assert_eq!(TaskRoute::SingleWorker.label(), "single-worker");
        assert_eq!(TaskRoute::FullPipeline.label(), "full-pipeline");

        assert!(TaskRoute::DirectAnswer
            .reason()
            .contains("conversational or explanatory"));
        assert!(TaskRoute::SingleWorker
            .reason()
            .contains("planner, critic, and reviewer are not needed"));
        assert!(TaskRoute::FullPipeline
            .reason()
            .contains("planner, critic, worker, and reviewer will run"));

        assert_eq!(TaskRoute::DirectAnswer.flow_steps(), "worker -> answer");
        assert_eq!(TaskRoute::SingleWorker.flow_steps(), "worker -> answer");
        assert_eq!(
            TaskRoute::FullPipeline.flow_steps(),
            "planner -> critic -> worker -> reviewer -> answer"
        );

        assert_eq!(
            TaskRoute::DirectAnswer.review_skip_reason(),
            Some("conversational answer")
        );
        assert_eq!(
            TaskRoute::SingleWorker.review_skip_reason(),
            Some("single worker route")
        );
        assert_eq!(TaskRoute::FullPipeline.review_skip_reason(), None);
    }
}
