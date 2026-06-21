//! Real LLM-backed role implementations.
//!
//! Planner / Critic / Reviewer are normally stubs (always approve).
//! These real implementations call a `ModelProvider` (Anthropic / OpenAI / etc.)
//! with role-specific system prompts and parse the JSON response.
//!
//! Each role expects a JSON object back. We use a lenient JSON extractor
//! (find first `{`, find matching `}`) because LLMs often wrap JSON in prose.

use crate::core::provider::{ChatMessage, ChatRequest, ModelProvider};
use crate::core::session_store::SessionStore;
use crate::core::types::{
    CritiqueOutput, PlanOutput, ReviewContext, ReviewOutput, Role, Task, TaskStatus,
};
use crate::roles::critic::Critic;
use crate::roles::planner::Planner;
use crate::roles::reviewer::Reviewer;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

// ── System prompts ─────────────────────────────────────────

const PLANNER_SYSTEM: &str = r#"Decompose the task into 1-5 sub-tasks. Output JSON only, no markdown, no prose. Fields: sub_tasks (array of {prompt, tags}), rationale (1 sentence). If already atomic, return one sub-task with the original prompt."#;

const CRITIC_SYSTEM: &str = r#"Critique the plan. Output JSON only, no markdown, no prose. Fields: approved (bool), issues (array of strings, empty if approved), suggestions (array of strings). Be strict. Approve only if clear, complete, decomposable, likely to succeed."#;

const REVIEWER_SYSTEM: &str = r#"Review the worker's output against the user's intent. Output JSON only, no markdown, no prose. Fields: approved (bool), issues (array of strings). Approve useful conversational answers, greetings, explanations, or diagnostics even when there is no git diff. Reject only if the output is wrong, incomplete, unsafe, or clearly ignores the task."#;

// ── JSON extraction (tolerant of prose wrapping) ───────────

fn extract_json(content: &str) -> Result<serde_json::Value> {
    let trimmed = content.trim();

    // Strategy 1: try direct parse
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Ok(v);
    }

    // Strategy 2: find ```json ... ``` code fence and parse its contents
    if let Some(fence_start) = trimmed.find("```json") {
        let after = &trimmed[fence_start + 7..];
        if let Some(fence_end) = after.find("```") {
            let inner = after[..fence_end].trim();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(inner) {
                return Ok(v);
            }
        }
    }

    // Strategy 3: find any ``` ... ``` code fence
    if let Some(fence_start) = trimmed.find("```") {
        let after = &trimmed[fence_start + 3..];
        // Skip the language tag if present (e.g. ```json)
        let content_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let after_content = &after[content_start..];
        if let Some(fence_end) = after_content.find("```") {
            let inner = after_content[..fence_end].trim();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(inner) {
                return Ok(v);
            }
        }
    }

    // Strategy 4: find first '{' and try brace-matched parse
    if let Some(start) = trimmed.find('{') {
        let mut depth = 0i32;
        let mut end = None;
        for (i, c) in trimmed[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(end) = end {
            return serde_json::from_str(&trimmed[start..end]).map_err(|e| {
                anyhow!(
                    "extracted JSON parse failed: {} -- raw: {}",
                    e,
                    &trimmed[start..end]
                )
            });
        }
    }

    // All strategies failed. Log the raw content so we can see what the LLM
    // actually said (the system prompt says "Output JSON only" — if it
    // didn't, we want to know).
    let preview: String = trimmed.chars().take(500).collect();
    let total_chars = trimmed.chars().count();
    tracing::error!(
        "extract_json: NO JSON OBJECT FOUND in LLM response ({} chars total). First 500 chars:\n--- BEGIN ---\n{}\n--- END ---",
        total_chars, preview
    );
    Err(anyhow!(
        "no JSON object found in LLM response (first 200 chars): {}",
        &trimmed.chars().take(200).collect::<String>()
    ))
}

// ── LLM Planner ────────────────────────────────────────────

pub struct LLMPlanner {
    pub provider: Arc<dyn ModelProvider>,
    pub model: String,
    pub session_store: Arc<SessionStore>,
}

impl LLMPlanner {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        model: String,
        session_store: Arc<SessionStore>,
    ) -> Self {
        Self {
            provider,
            model,
            session_store,
        }
    }
}

#[async_trait]
impl Planner for LLMPlanner {
    async fn plan(&self, top_task: &Task) -> Result<PlanOutput> {
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: PLANNER_SYSTEM.into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: format!(
                        "Top-level task:\n\n{}\n\nDecompose into sub-tasks. Output JSON only.",
                        top_task.prompt
                    ),
                },
            ],
            max_tokens: Some(32768),
            temperature: Some(0.3),
        };
        let resp = self.provider.chat(req).await?;
        let json = extract_json(&resp.content)?;
        let rationale = json
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let sub_tasks_json = json
            .get("sub_tasks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut sub_tasks = vec![];
        for st in &sub_tasks_json {
            let prompt = st
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if prompt.is_empty() {
                continue;
            }
            let tags: Vec<String> = st
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let mut t = Task::new(prompt);
            t.tags = tags;
            t.role = Role::Worker;
            t.status = TaskStatus::Pending;
            sub_tasks.push(t);
        }
        if sub_tasks.is_empty() {
            // Fallback: 1:1 mapping of top task
            let mut t = top_task.clone();
            t.id = uuid::Uuid::new_v4();
            sub_tasks.push(t);
        }
        Ok(PlanOutput {
            sub_tasks,
            rationale,
            estimated_cost_usd: 0.0,
        })
    }

    async fn replan(&self, top_task: &Task, critique: &CritiqueOutput) -> Result<PlanOutput> {
        // Naive replan: re-plan with the critique as additional guidance.
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage { role: "system".into(), content: PLANNER_SYSTEM.into() },
                ChatMessage { role: "user".into(), content: format!(
                    "Top-level task:\n\n{}\n\nPrevious plan was rejected. Issues:\n{}\n\nSuggestions:\n{}\n\nRe-plan. Output JSON only.",
                    top_task.prompt,
                    critique.issues.join("\n- "),
                    critique.suggestions.join("\n- "),
                )},
            ],
            max_tokens: Some(32768),
            temperature: Some(0.3),
        };
        let resp = self.provider.chat(req).await?;
        let json = extract_json(&resp.content)?;
        let rationale = json
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let sub_tasks_json = json
            .get("sub_tasks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut sub_tasks = vec![];
        for st in &sub_tasks_json {
            let prompt = st
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if prompt.is_empty() {
                continue;
            }
            let tags: Vec<String> = st
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let mut t = Task::new(prompt);
            t.tags = tags;
            sub_tasks.push(t);
        }
        if sub_tasks.is_empty() {
            let mut t = top_task.clone();
            t.id = uuid::Uuid::new_v4();
            sub_tasks.push(t);
        }
        Ok(PlanOutput {
            sub_tasks,
            rationale,
            estimated_cost_usd: 0.0,
        })
    }
}

// ── LLM Critic ─────────────────────────────────────────────

pub struct LLMCritic {
    pub provider: Arc<dyn ModelProvider>,
    pub model: String,
}

impl LLMCritic {
    pub fn new(provider: Arc<dyn ModelProvider>, model: String) -> Self {
        Self { provider, model }
    }
}

#[async_trait]
impl Critic for LLMCritic {
    async fn critique(&self, top_task: &Task, plan: &PlanOutput) -> Result<CritiqueOutput> {
        let plan_json = serde_json::to_string_pretty(&serde_json::json!({
            "rationale": plan.rationale,
            "sub_tasks": plan.sub_tasks.iter().map(|t| serde_json::json!({
                "prompt": t.prompt,
                "tags": t.tags,
            })).collect::<Vec<_>>(),
        }))?;
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: CRITIC_SYSTEM.into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: format!(
                    "Top-level task:\n{}\n\nProposed plan:\n{}\n\nCritique it. Output JSON only.",
                    top_task.prompt, plan_json
                ),
                },
            ],
            max_tokens: Some(32768),
            temperature: Some(0.2),
        };
        let resp = self.provider.chat(req).await?;
        let json = extract_json(&resp.content)?;
        let approved = json
            .get("approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let issues: Vec<String> = json
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let suggestions: Vec<String> = json
            .get("suggestions")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(CritiqueOutput {
            approved,
            issues,
            suggestions,
        })
    }
}

// ── LLM Reviewer ───────────────────────────────────────────

pub struct LLMReviewer {
    pub provider: Arc<dyn ModelProvider>,
    pub model: String,
}

impl LLMReviewer {
    pub fn new(provider: Arc<dyn ModelProvider>, model: String) -> Self {
        Self { provider, model }
    }
}

#[async_trait]
impl Reviewer for LLMReviewer {
    async fn review(&self, task: &Task, ctx: &ReviewContext) -> Result<ReviewOutput> {
        let worker_text = extract_worker_text(&ctx.session_log_path)
            .unwrap_or_else(|err| format!("(worker output unavailable: {err})"));
        let worker_text = truncate_review_text(&worker_text, 8_000);
        let diff = read_diff_capped(&ctx.worktree_path, 8000);
        let diff = if diff.trim().is_empty() {
            "(no file changes; this is acceptable for conversational, diagnostic, or explanation tasks)".to_string()
        } else {
            diff
        };

        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage { role: "system".into(), content: REVIEWER_SYSTEM.into() },
                ChatMessage { role: "user".into(), content: format!(
                    "Task that was attempted:\n{}\n\n\
                     ## What the worker said (assistant text from session log):\n```\n{}\n```\n\n\
                     ## What the worker actually changed (git diff, capped to 8KB):\n```diff\n{}\n```\n\n\
                     Review against the user's intent. For conversational questions, greetings, explanations, or diagnostics, \
                     approve a useful textual answer even when there is no diff. Only reject if the output is wrong, incomplete, \
                     unsafe, or clearly ignores the task. Output JSON only.",
                    task.prompt, worker_text, diff
                )},
            ],
            max_tokens: Some(32768),
            temperature: Some(0.2),
        };
        let resp = self.provider.chat(req).await?;
        let json = extract_json(&resp.content)?;
        let approved = json
            .get("approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let issues: Vec<String> = json
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(ReviewOutput { approved, issues })
    }
}

/// Extract human-readable worker text from a session JSONL file.
fn extract_worker_text(path: &std::path::Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading worker session log {}", path.display()))?;
    let mut out = String::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        append_event_text(&mut out, &value);
    }
    if out.trim().is_empty() {
        return Err(anyhow!("no worker text found"));
    }
    Ok(out)
}

fn append_event_text(out: &mut String, value: &serde_json::Value) {
    if let Some(text) = value.as_str() {
        out.push_str(text);
        out.push('\n');
        return;
    }

    if let Some(text) = value
        .get("result")
        .or_else(|| value.get("text"))
        .or_else(|| value.get("line"))
        .and_then(|v| v.as_str())
    {
        out.push_str(text);
        out.push('\n');
    }

    if let Some(arr) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    out.push_str(text);
                    out.push('\n');
                }
            }
        }
    }

    if let Some(obj) = value.as_object() {
        for key in ["payload", "event", "message"] {
            if let Some(nested) = obj.get(key) {
                if !nested.is_string() {
                    append_event_text(out, nested);
                }
            }
        }
    }
}

/// Read `git diff` from a worktree, capped to N bytes.
fn read_diff_capped(worktree: &std::path::Path, cap: usize) -> String {
    let output = std::process::Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(worktree)
        .output();
    let raw = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return String::new(),
    };
    truncate_review_text(&raw, cap)
}

fn truncate_review_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…(truncated)", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_json() {
        let v = extract_json(r#"{"approved": true, "issues": []}"#).unwrap();
        assert_eq!(v["approved"], true);
    }

    #[test]
    fn parses_json_inside_markdown_fence() {
        let input = r#"Here's my review:
```json
{"approved": false, "issues": ["x"]}
```
That's my verdict."#;
        let v = extract_json(input).unwrap();
        assert_eq!(v["approved"], false);
        assert_eq!(v["issues"][0], "x");
    }

    #[test]
    fn parses_json_inside_unlabelled_fence() {
        let input = "```\n{\"approved\": true}\n```";
        let v = extract_json(input).unwrap();
        assert_eq!(v["approved"], true);
    }

    #[test]
    fn parses_json_with_surrounding_prose() {
        let input = r#"Sure, here you go: {"approved": false, "issues": ["a","b"]}."#;
        let v = extract_json(input).unwrap();
        assert_eq!(v["approved"], false);
        assert_eq!(v["issues"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn reviewer_context_extracts_worker_text_from_session_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "content": [
                            { "type": "text", "text": "你好，我可以帮你写代码。" }
                        ]
                    }
                }),
                serde_json::json!({
                    "type": "result",
                    "result": "最终回答"
                })
            ),
        )
        .unwrap();

        let text = extract_worker_text(&path).unwrap();

        assert!(text.contains("你好，我可以帮你写代码。"));
        assert!(text.contains("最终回答"));
    }

    #[test]
    fn reviewer_context_truncates_on_char_boundary() {
        let text = truncate_review_text("你好abcdef", 7);

        assert_eq!(text, "你好a…(truncated)");
    }
}
