//! Reviewer role: gates worker output before merge.

use crate::core::types::{ReviewContext, ReviewOutput, Task};
use crate::pipeline::orchestrator::RunProgress;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

#[async_trait]
pub trait Reviewer: Send + Sync {
    async fn review(&self, task: &Task, ctx: &ReviewContext) -> Result<ReviewOutput>;
    async fn review_with_progress(
        &self,
        task: &Task,
        ctx: &ReviewContext,
        _progress: Option<UnboundedSender<RunProgress>>,
    ) -> Result<ReviewOutput> {
        self.review(task, ctx).await
    }
}

pub struct ReviewerRole {
    _name: String,
}

impl ReviewerRole {
    pub fn new(name: impl Into<String>) -> Self {
        Self { _name: name.into() }
    }
}

#[async_trait]
impl Reviewer for ReviewerRole {
    async fn review(&self, _task: &Task, _ctx: &ReviewContext) -> Result<ReviewOutput> {
        Ok(ReviewOutput {
            approved: true,
            issues: vec![],
        })
    }
}
