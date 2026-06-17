//! Ollama local-provider (OpenAI-compatible endpoint).

use crate::core::provider::{ChatRequest, ChatResponse, ModelProvider};
use crate::providers::openai::OpenAIProvider;

pub struct OllamaProvider {
    inner: OpenAIProvider,
}

impl OllamaProvider {
    pub fn new(base_url: String) -> Self {
        let inner = OpenAIProvider::new("ollama".into()).with_base_url(base_url);
        Self { inner }
    }
}

#[async_trait::async_trait]
impl ModelProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.inner.chat(req).await
    }
}
