//! Critic role: red-teams the plan before execution.

use crate::core::types::{CritiqueOutput, PlanOutput, Task};
use crate::pipeline::orchestrator::RunProgress;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

#[async_trait]
pub trait Critic: Send + Sync {
    async fn critique(&self, top_task: &Task, plan: &PlanOutput) -> Result<CritiqueOutput>;
    async fn critique_with_progress(
        &self,
        top_task: &Task,
        plan: &PlanOutput,
        _progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<CritiqueOutput> {
        self.critique(top_task, plan).await
    }
}

pub struct CriticRole {
    _name: String,
}

impl CriticRole {
    pub fn new(name: impl Into<String>) -> Self {
        Self { _name: name.into() }
    }
}

#[async_trait]
impl Critic for CriticRole {
    async fn critique(&self, _top_task: &Task, _plan: &PlanOutput) -> Result<CritiqueOutput> {
        // Default critic: always approve. Wire an LLM-backed critic via config
        // to do real adversarial review.
        Ok(CritiqueOutput {
            approved: true,
            issues: vec![],
            suggestions: vec![],
        })
    }
}
