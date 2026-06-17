//! Planner role: decomposes a high-level task into a DAG of sub-tasks.

use crate::core::session_store::SessionStore;
use crate::core::types::{PlanOutput, Task};
use crate::pipeline::orchestrator::RunProgress;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

#[async_trait]
pub trait Planner: Send + Sync {
    async fn plan(&self, top_task: &Task) -> Result<PlanOutput>;
    async fn plan_with_progress(
        &self,
        top_task: &Task,
        _progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<PlanOutput> {
        self.plan(top_task).await
    }
    async fn replan(
        &self,
        top_task: &Task,
        critique: &crate::core::types::CritiqueOutput,
    ) -> Result<PlanOutput>;
    async fn replan_with_progress(
        &self,
        top_task: &Task,
        critique: &crate::core::types::CritiqueOutput,
        _progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<PlanOutput> {
        self.replan(top_task, critique).await
    }
}

/// Default planner: in v0.1 we don't run an LLM as the planner. We treat
/// the top task as a single sub-task. The user explicitly fans out by
/// submitting multiple prompts, or wires an LLM planner later via config.
///
/// When wired to a real LLM (e.g. via `DirectAPIAdapter`), this will
/// prompt it to produce a structured DAG.
pub struct PlannerRole {
    _name: String,
    _store: Arc<SessionStore>,
}

impl PlannerRole {
    pub fn new(name: impl Into<String>, store: Arc<SessionStore>) -> Self {
        Self {
            _name: name.into(),
            _store: store,
        }
    }
}

#[async_trait]
impl Planner for PlannerRole {
    async fn plan(&self, top_task: &Task) -> Result<PlanOutput> {
        // Minimal default: the top task IS the only sub-task.
        // TODO: integrate with a ModelProvider to do real LLM-based planning.
        let mut sub = top_task.clone();
        sub.id = uuid::Uuid::new_v4();
        Ok(PlanOutput {
            sub_tasks: vec![sub],
            rationale: format!(
                "default planner: 1:1 mapping of top task '{}'",
                top_task.prompt
            ),
            estimated_cost_usd: 0.0,
        })
    }

    async fn replan(
        &self,
        top_task: &Task,
        _critique: &crate::core::types::CritiqueOutput,
    ) -> Result<PlanOutput> {
        // Naive replan: just keep the same plan.
        self.plan(top_task).await
    }
}
