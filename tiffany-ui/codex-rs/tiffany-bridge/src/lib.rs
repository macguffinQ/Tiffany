use serde::Deserialize;
use std::collections::HashMap;

mod command_args;
mod context_prompt;
mod native_session;
mod visible_output;

pub use command_args::{
    ContinueRequest, continue_request, continue_target_role, doctor_command_args,
    jobs_command_args, provider_command_args, roles_command_args, thread_command_args,
};
pub use context_prompt::{ContextPromptTurn, contextual_prompt};
pub use native_session::{
    NativeSessionCommand, NativeSessionPath, NativeSessionRuntime, claude_session_jsonl_path,
    codex_session_jsonl_path, find_codex_rollout_path_by_id, find_gemini_chat_path_by_id,
    gemini_project_hash, gemini_session_json_path_in_home, gemini_session_message_count,
    native_session_command_is_claude, native_session_command_is_codex,
    native_session_command_is_gemini, native_session_path_in_home,
};
pub use visible_output::{
    PendingVisibleOutput, VisibleOutputEvent, format_tiffany_summary_style,
    looks_like_native_session_recovery, normalized_visible_output_key, visible_output_content,
    visible_output_kind_for_event, visible_output_kind_scope_label, visible_output_scope,
    visible_output_seen_key, visible_output_suffix,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSummary {
    pub providers: usize,
    pub models: usize,
    pub roles: usize,
    pub runtimes: usize,
    pub default_worker: Option<DefaultWorkerSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultWorkerSummary {
    pub role: String,
    pub runtime: String,
    pub binary: String,
    pub status: WorkerReadinessStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerReadinessStatus {
    Ready,
    ModelMissing,
    ProviderMissing,
    RuntimeMissing,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyOrchestratorConfig {
    #[serde(default)]
    providers: HashMap<String, RawTiffanyProviderConfig>,
    #[serde(default)]
    runtimes: HashMap<String, RawTiffanyRuntimeConfig>,
    #[serde(default)]
    models: Vec<RawTiffanyModelConfig>,
    #[serde(default)]
    roles: HashMap<String, RawTiffanyRoleConfig>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyProviderConfig {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyRuntimeConfig {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: Option<String>,
    binary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyModelConfig {
    id: String,
    provider: String,
    #[allow(dead_code)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTiffanyRoleConfig {
    model: String,
    runtime: String,
}

pub fn summarize_orchestrator_config_yaml(
    raw: &str,
    runtime_is_ready: impl Fn(&str) -> bool,
) -> Result<ConfigSummary, String> {
    let config =
        serde_yaml::from_str::<RawTiffanyOrchestratorConfig>(raw).map_err(|err| err.to_string())?;
    Ok(summarize_orchestrator_config(&config, runtime_is_ready))
}

fn summarize_orchestrator_config(
    config: &RawTiffanyOrchestratorConfig,
    runtime_is_ready: impl Fn(&str) -> bool,
) -> ConfigSummary {
    ConfigSummary {
        providers: config.providers.len(),
        models: config.models.len(),
        roles: config.roles.len(),
        runtimes: config.runtimes.len(),
        default_worker: default_worker_summary(config, runtime_is_ready),
    }
}

fn default_worker_summary(
    config: &RawTiffanyOrchestratorConfig,
    runtime_is_ready: impl Fn(&str) -> bool,
) -> Option<DefaultWorkerSummary> {
    let role = default_worker_role(&config.roles)?;
    let role_cfg = config.roles.get(&role)?;
    let runtime_cfg = runtime_config(config, &role_cfg.runtime);
    let binary = runtime_cfg
        .and_then(|runtime| runtime.binary.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| default_binary_for_runtime(&role_cfg.runtime));
    let status = default_worker_status(config, role_cfg, runtime_cfg, &binary, runtime_is_ready);
    Some(DefaultWorkerSummary {
        role,
        runtime: role_cfg.runtime.clone(),
        binary,
        status,
    })
}

fn default_worker_status(
    config: &RawTiffanyOrchestratorConfig,
    role: &RawTiffanyRoleConfig,
    runtime: Option<&RawTiffanyRuntimeConfig>,
    binary: &str,
    runtime_is_ready: impl Fn(&str) -> bool,
) -> WorkerReadinessStatus {
    let Some(model) = config.models.iter().find(|model| model.id == role.model) else {
        return WorkerReadinessStatus::ModelMissing;
    };
    if !config.providers.contains_key(&model.provider) {
        return WorkerReadinessStatus::ProviderMissing;
    }
    if runtime.is_none() || !runtime_is_ready(binary) {
        return WorkerReadinessStatus::RuntimeMissing;
    }
    WorkerReadinessStatus::Ready
}

fn runtime_config<'a>(
    config: &'a RawTiffanyOrchestratorConfig,
    runtime: &str,
) -> Option<&'a RawTiffanyRuntimeConfig> {
    config.runtimes.get(runtime).or_else(|| {
        runtime_aliases(runtime)
            .iter()
            .find_map(|alias| config.runtimes.get(*alias))
    })
}

fn runtime_aliases(runtime: &str) -> &'static [&'static str] {
    match runtime {
        "codex" => &["codex"],
        "claude-code" | "claude" | "cc" => &["claude-code", "claude", "cc"],
        "gemini" | "gemini-cli" => &["gemini", "gemini-cli"],
        _ => &[],
    }
}

fn default_binary_for_runtime(runtime: &str) -> String {
    if is_claude_runtime(runtime) {
        "claude".to_string()
    } else if is_codex_runtime(runtime) {
        "codex".to_string()
    } else if is_gemini_runtime(runtime) {
        "gemini".to_string()
    } else {
        runtime.to_string()
    }
}

fn default_worker_role(roles: &HashMap<String, RawTiffanyRoleConfig>) -> Option<String> {
    default_worker_role_for_runtime(roles, is_claude_runtime, "worker-cc")
        .or_else(|| default_worker_role_for_runtime(roles, is_codex_runtime, "worker-codex"))
        .or_else(|| default_worker_role_for_runtime(roles, is_gemini_runtime, "worker-gemini"))
}

fn default_worker_role_for_runtime(
    roles: &HashMap<String, RawTiffanyRoleConfig>,
    runtime_matches: impl Fn(&str) -> bool,
    preferred: &str,
) -> Option<String> {
    if roles.contains_key(preferred) {
        return Some(preferred.to_string());
    }
    let mut candidates = roles
        .iter()
        .filter(|(name, role)| is_worker_role_name(name) && runtime_matches(&role.runtime))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

fn is_claude_runtime(runtime: &str) -> bool {
    matches!(runtime, "claude-code" | "claude" | "cc")
}

fn is_codex_runtime(runtime: &str) -> bool {
    runtime == "codex"
}

fn is_gemini_runtime(runtime: &str) -> bool {
    matches!(runtime, "gemini" | "gemini-cli")
}

fn is_worker_role_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    (name.contains("worker") || name.contains("executor")) && !name.contains("reviewer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_default_worker_from_config_yaml() {
        let raw = r#"
providers:
  anthropic:
    type: anthropic
runtimes:
  claude-code:
    type: claude-code
    binary: claude
models:
  - id: sonnet
    provider: anthropic
    name: claude-sonnet-4-6
roles:
  worker-cc:
    model: sonnet
    runtime: claude-code
"#;

        let summary =
            summarize_orchestrator_config_yaml(raw, |binary| binary == "claude").expect("summary");

        assert_eq!(summary.providers, 1);
        assert_eq!(summary.models, 1);
        assert_eq!(summary.roles, 1);
        assert_eq!(summary.runtimes, 1);
        let worker = summary.default_worker.expect("default worker");
        assert_eq!(worker.role, "worker-cc");
        assert_eq!(worker.runtime, "claude-code");
        assert_eq!(worker.binary, "claude");
        assert_eq!(worker.status, WorkerReadinessStatus::Ready);
    }

    #[test]
    fn reports_missing_provider_before_runtime_missing() {
        let raw = r#"
runtimes:
  codex:
    type: codex
models:
  - id: gpt
    provider: openai
roles:
  worker-codex:
    model: gpt
    runtime: codex
"#;

        let summary = summarize_orchestrator_config_yaml(raw, |_| false).expect("summary");

        assert_eq!(
            summary.default_worker.expect("default worker").status,
            WorkerReadinessStatus::ProviderMissing
        );
    }
}
