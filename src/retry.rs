//! Smart retry with exponential backoff for LLM calls.
//!
//! Network errors (timeouts, connection refused, 5xx, 429) are retried.
//! Auth errors (401, 403) and bad-request errors (400, 422) fail fast.
//!
//! Borrowed from the production LLM client pattern (e.g. langchain, haystack).

use std::future::Future;
use std::time::Duration;
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            // No retry by default. LLM calls are usually idempotent enough
            // that retrying on a 60s timeout just delays the failure
            // signal to the user (up to 3 minutes of "stuck" appearance).
            // Set max_attempts: 3 in config if you want retries back.
            max_attempts: 1,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(10),
        }
    }
}

/// Determine if an error is worth retrying. Network/timeout/server errors yes;
/// auth/bad-request errors no.
pub fn is_retryable(err: &anyhow::Error) -> bool {
    if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
        // No response (network failure) — always retry
        if req_err.status().is_none() {
            return true;
        }
        // Has a response — only retry server errors or rate limits
        if let Some(status) = req_err.status() {
            return status.is_server_error() || status.as_u16() == 429;
        }
    }
    // For non-reqwest errors: check the error chain for telltale strings
    let s = err.to_string().to_lowercase();
    if s.contains("timeout") || s.contains("connection") || s.contains("dns") {
        return true;
    }
    false
}

/// Run an async operation with retry. Returns the first success, or the last error.
pub async fn with_retry<F, Fut, T>(mut op: F, cfg: RetryConfig) -> Result<T, anyhow::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, anyhow::Error>>,
{
    let mut delay = cfg.initial_backoff;
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..cfg.max_attempts {
        match op().await {
            Ok(v) => {
                if attempt > 0 {
                    debug!("LLM call succeeded on attempt {}", attempt + 1);
                }
                return Ok(v);
            }
            Err(e) => {
                if !is_retryable(&e) || attempt + 1 >= cfg.max_attempts {
                    if !is_retryable(&e) {
                        debug!("non-retryable error, failing: {}", e);
                    }
                    return Err(e);
                }
                warn!(
                    "LLM call failed (attempt {}/{}): {}; retrying in {:?}",
                    attempt + 1,
                    cfg.max_attempts,
                    e,
                    delay
                );
                last_err = Some(e);
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(cfg.max_backoff);
            }
        }
    }
    Err(last_err.expect("retry loop must have at least one error"))
}
