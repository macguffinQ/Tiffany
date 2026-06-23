//! Lightweight task classification used to keep chat, planning, and review behavior aligned.

use crate::agent_events;
use crate::core::types::{PlanOutput, Task};

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

pub fn should_skip_review_for_task(top_task: &Task, task: &Task) -> bool {
    is_conversational_task(top_task) || is_conversational_task(task)
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
    if has_any_tag(&task.tags, ENGINEERING_TAGS) {
        return false;
    }
    if has_any_tag(&task.tags, DIRECT_TAGS) {
        return !agent_events::contains_engineering_action(&task.prompt);
    }
    agent_events::request_looks_direct_answer(&task.prompt)
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

const DIRECT_TAGS: &[&str] = &[
    "chat",
    "conversation",
    "conversational",
    "greeting",
    "qa",
    "question",
    "explanation",
    "diagnostic",
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
    fn keeps_engineering_requests_in_full_pipeline() {
        assert!(!is_conversational_task(&Task::new("优化 TUI 显示")));
        assert!(!is_conversational_task(&Task::new("fix the build error")));
        assert!(!is_conversational_task(&Task::new("按照这个计划来做")));
        assert!(!is_conversational_task(&Task::new("写参赛 agent")));
        assert!(!is_conversational_task(&Task::new("创建一个 worker")));
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
}
