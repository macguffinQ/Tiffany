//! Core domain types: Task, Session, Event, Role, etc.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Orchestrator,
    Planner,
    Critic,
    Router,
    Worker,
    Reviewer,
    AbJudge,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Orchestrator => "orchestrator",
            Role::Planner => "planner",
            Role::Critic => "critic",
            Role::Router => "router",
            Role::Worker => "worker",
            Role::Reviewer => "reviewer",
            Role::AbJudge => "ab_judge",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Planning,
    Critiquing,
    Running,
    Reviewing,
    Completed,
    Failed,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub prompt: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub deps: Vec<Uuid>,
    #[serde(default)]
    pub files_of_interest: Vec<String>,
    pub worktree: Option<PathBuf>,
    /// Worker route hint, for example `worker-cc` or `worker-codex`.
    pub agent_hint: Option<String>,
    /// Claude Code subagent name passed as `claude --agent <name>`.
    #[serde(default)]
    pub cc_agent_hint: Option<String>,
    #[serde(default)]
    pub model_hint: Option<String>,
    #[serde(default)]
    pub model_provider_hint: Option<String>,
    pub role: Role,
    pub timeout: u32,
    pub parent_session_ids: Vec<Uuid>,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl Task {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            prompt: prompt.into(),
            tags: vec![],
            deps: vec![],
            files_of_interest: vec![],
            worktree: None,
            agent_hint: None,
            cc_agent_hint: None,
            model_hint: None,
            model_provider_hint: None,
            role: Role::Worker,
            timeout: 600,
            parent_session_ids: vec![],
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            result: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub session_id: Uuid,
    pub task_id: Uuid,
    pub ts: DateTime<Utc>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub task_id: Uuid,
    pub agent: String,
    pub role: Role,
    pub model: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub parent_session_ids: Vec<Uuid>,
    pub token_in: u64,
    pub token_out: u64,
    pub cost_usd: f64,
    pub files_touched: Vec<String>,
}

impl Session {
    pub fn new(task_id: Uuid, agent: impl Into<String>, role: Role) -> Self {
        Self {
            id: Uuid::new_v4(),
            task_id,
            agent: agent.into(),
            role,
            model: String::new(),
            started_at: Utc::now(),
            ended_at: None,
            parent_session_ids: vec![],
            token_in: 0,
            token_out: 0,
            cost_usd: 0.0,
            files_touched: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanOutput {
    pub sub_tasks: Vec<Task>,
    pub rationale: String,
    pub estimated_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueOutput {
    pub approved: bool,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutput {
    pub approved: bool,
    #[serde(default)]
    pub issues: Vec<String>,
}

/// Context passed to the Reviewer: the session log (raw events) and the
/// worktree path (so the reviewer can `git diff` what the worker actually did).
#[derive(Debug, Clone)]
pub struct ReviewContext {
    pub session_log_path: std::path::PathBuf,
    pub worktree_path: std::path::PathBuf,
}
