//! Axum-based webhook server. Receives CI events and triggers orchestrator runs.

use anyhow::Result;
use axum::extract::Json;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookPayload {
    pub prompt: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub planner: Option<String>,
    #[serde(default)]
    pub worker: Option<String>,
}

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/run", post(handle_run))
}

async fn health() -> &'static str {
    "ok"
}

async fn handle_run(Json(payload): Json<WebhookPayload>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "accepted": true,
        "prompt": payload.prompt,
        "tags": payload.tags,
    }))
}

pub async fn serve(addr: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router()).await?;
    Ok(())
}
