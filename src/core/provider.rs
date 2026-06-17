//! The ModelProvider trait — abstracts which LLM API to call.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;

    /// Simple chat completion — used by planner/critic/reviewer roles.
    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse>;
}

/// A provider that always returns an error with a helpful message.
/// Used when an API key is missing — the orchestrator can still start,
/// and the user gets a clear error at the moment they try to run a task.
pub struct FailingProvider {
    pub error_message: String,
}

impl FailingProvider {
    pub fn new(error_message: impl Into<String>) -> Self {
        Self {
            error_message: error_message.into(),
        }
    }
}

#[async_trait]
impl ModelProvider for FailingProvider {
    fn name(&self) -> &str {
        "failing"
    }

    async fn chat(&self, _req: ChatRequest) -> anyhow::Result<ChatResponse> {
        Err(anyhow::anyhow!("{}", self.error_message))
    }
}
