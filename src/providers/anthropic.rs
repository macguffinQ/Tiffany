//! Anthropic Messages API provider.

use crate::core::provider::{ChatRequest, ChatResponse, ModelProvider};
use crate::retry::{with_retry, RetryConfig};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            // 60s read timeout + 10s connect timeout. Was 5min — too
            // long; users perceived terminal chat as "stuck" on slow networks.
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

#[derive(Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    messages: Vec<ApiMessage>,
    system: Option<String>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    usage: Usage,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let url = if self.base_url.ends_with("/v1") {
            format!("{}/messages", self.base_url)
        } else {
            format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
        };
        let api_key = self.api_key.clone();
        let client = self.client.clone();

        with_retry(
            move || {
                let url = url.clone();
                let api_key = api_key.clone();
                let client = client.clone();
                let req = req.clone();
                async move {
                    let system = req
                        .messages
                        .iter()
                        .find(|m| m.role == "system")
                        .map(|m| m.content.clone());
                    let messages: Vec<ApiMessage> = req
                        .messages
                        .iter()
                        .filter(|m| m.role != "system")
                        .map(|m| ApiMessage {
                            role: m.role.clone(),
                            content: m.content.clone(),
                        })
                        .collect();

                    let body = ApiRequest {
                        model: &req.model,
                        max_tokens: req.max_tokens.unwrap_or(2048),
                        temperature: req.temperature,
                        messages,
                        system,
                    };

                    let resp = client
                        .post(&url)
                        .header("x-api-key", &api_key)
                        .header("anthropic-version", "2023-06-01")
                        .header("content-type", "application/json")
                        .json(&body)
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<ApiResponse>()
                        .await?;

                    let content = resp
                        .content
                        .iter()
                        .filter(|c| c.kind == "text")
                        .map(|c| c.text.as_str())
                        .collect::<Vec<_>>()
                        .join("");

                    // Rough cost estimate: $3/MTok input, $15/MTok output for sonnet-class
                    let cost = (resp.usage.input_tokens as f64 * 3.0
                        + resp.usage.output_tokens as f64 * 15.0)
                        / 1_000_000.0;

                    Ok(ChatResponse {
                        content,
                        input_tokens: resp.usage.input_tokens,
                        output_tokens: resp.usage.output_tokens,
                        cost_usd: cost,
                    })
                }
            },
            RetryConfig::default(),
        )
        .await
    }
}
