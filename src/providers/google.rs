//! Google Gemini provider.

use crate::core::provider::{ChatRequest, ChatResponse, ModelProvider};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct GoogleProvider {
    api_key: String,
    client: Client,
}

impl GoogleProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: Client::new(),
        }
    }
}

#[derive(Serialize)]
struct ApiRequest {
    contents: Vec<Content>,
}

#[derive(Serialize)]
struct Content {
    parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(Serialize)]
struct Part {
    text: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    candidates: Vec<Candidate>,
    #[serde(default)]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
struct Candidate {
    content: CandidateContent,
}

#[derive(Deserialize)]
struct CandidateContent {
    parts: Vec<CandidatePart>,
}

#[derive(Deserialize)]
struct CandidatePart {
    text: String,
}

#[derive(Deserialize)]
struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
}

#[async_trait]
impl ModelProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let mut contents: Vec<Content> = req
            .messages
            .iter()
            .map(|m| Content {
                parts: vec![Part {
                    text: m.content.clone(),
                }],
                role: if m.role == "user" {
                    Some("user".into())
                } else {
                    Some("model".into())
                },
            })
            .collect();

        if contents.is_empty() {
            anyhow::bail!("no messages to send");
        }

        // Gemini doesn't have a "system" role in the same way; merge into first user msg
        if let Some(sys) = req.messages.iter().find(|m| m.role == "system") {
            if let Some(first) = contents.first_mut() {
                first.parts[0].text = format!("{}\n\n{}", sys.content, first.parts[0].text);
            }
        }

        let body = ApiRequest { contents };
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            req.model, self.api_key
        );
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<ApiResponse>()
            .await?;

        let content = resp
            .candidates
            .first()
            .map(|c| {
                c.content
                    .parts
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let (in_t, out_t) = resp
            .usage_metadata
            .map(|u| (u.prompt_token_count, u.candidates_token_count))
            .unwrap_or((0, 0));

        Ok(ChatResponse {
            content,
            input_tokens: in_t,
            output_tokens: out_t,
            cost_usd: 0.0, // Free tier or set per pricing
        })
    }
}
