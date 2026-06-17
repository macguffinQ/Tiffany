//! Shared runtime route metadata for Claude, Codex, and future ACP runtimes.

use crate::config::RoleConfig;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentRuntimeRoute {
    pub label: &'static str,
    pub aliases: &'static [&'static str],
    pub role_hint: Option<&'static str>,
    pub runtime: Option<&'static str>,
    pub description: &'static str,
}

pub const AGENT_RUNTIME_ROUTES: &[AgentRuntimeRoute] = &[
    AgentRuntimeRoute {
        label: "auto",
        aliases: &["", "auto", "default"],
        role_hint: None,
        runtime: None,
        description: "Use configured orchestrator routing",
    },
    AgentRuntimeRoute {
        label: "claude",
        aliases: &["claude", "claude-code", "cc", "worker-cc"],
        role_hint: Some("worker-cc"),
        runtime: Some("claude-code"),
        description: "Run worker tasks with Claude Code",
    },
    AgentRuntimeRoute {
        label: "codex",
        aliases: &["codex", "worker-codex"],
        role_hint: Some("worker-codex"),
        runtime: Some("codex"),
        description: "Run worker tasks with Codex CLI",
    },
];

pub fn normalize_agent_hint(agent: &str) -> Option<String> {
    let normalized = agent.trim().to_ascii_lowercase();
    AGENT_RUNTIME_ROUTES
        .iter()
        .find(|route| route.aliases.iter().any(|alias| *alias == normalized))
        .map(|route| route.role_hint.map(str::to_string))
        .unwrap_or_else(|| (!normalized.is_empty()).then_some(normalized))
}

pub fn normalize_agent_hint_with_roles(
    agent: &str,
    roles: &HashMap<String, RoleConfig>,
) -> Option<String> {
    if roles.is_empty() {
        return normalize_agent_hint(agent);
    }

    let trimmed = agent.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.to_ascii_lowercase();
    let Some(route) = AGENT_RUNTIME_ROUTES
        .iter()
        .find(|route| route.aliases.iter().any(|alias| *alias == normalized))
    else {
        if roles.contains_key(trimmed) {
            return Some(trimmed.to_string());
        }
        if roles.contains_key(&normalized) {
            return Some(normalized);
        }
        return Some(normalized);
    };

    if let Some(runtime) = route.runtime {
        return default_worker_role_for_runtime(roles, runtime)
            .or_else(|| route.role_hint.map(str::to_string));
    }
    route.role_hint.map(str::to_string)
}

pub fn agent_label(agent_hint: Option<&str>) -> String {
    AGENT_RUNTIME_ROUTES
        .iter()
        .find(|route| {
            route.role_hint == agent_hint
                || agent_hint.is_some_and(|hint| route.aliases.iter().any(|alias| *alias == hint))
        })
        .map(|route| route.label.to_string())
        .unwrap_or_else(|| agent_hint.unwrap_or("auto").to_string())
}

pub fn is_claude_runtime(runtime: &str) -> bool {
    matches!(runtime, "claude-code" | "claude" | "cc")
}

pub fn is_codex_runtime(runtime: &str) -> bool {
    runtime == "codex"
}

pub fn default_worker_role(roles: &HashMap<String, RoleConfig>) -> Option<String> {
    default_worker_role_for_runtime(roles, "claude-code")
        .or_else(|| default_worker_role_for_runtime(roles, "codex"))
}

pub fn default_worker_role_for_runtime(
    roles: &HashMap<String, RoleConfig>,
    runtime: &str,
) -> Option<String> {
    if is_claude_runtime(runtime) {
        if roles.contains_key("worker-cc") {
            return Some("worker-cc".into());
        }
        return first_worker_role_with_runtime(roles, is_claude_runtime);
    }
    if is_codex_runtime(runtime) {
        if roles.contains_key("worker-codex") {
            return Some("worker-codex".into());
        }
        return first_worker_role_with_runtime(roles, is_codex_runtime);
    }
    first_worker_role_with_runtime(roles, |role_runtime| role_runtime == runtime)
}

pub fn route_by_label(label: &str) -> Option<&'static AgentRuntimeRoute> {
    let normalized = label.trim().to_ascii_lowercase();
    AGENT_RUNTIME_ROUTES.iter().find(|route| {
        route.label == normalized || route.aliases.iter().any(|alias| *alias == normalized)
    })
}

fn first_worker_role_with_runtime(
    roles: &HashMap<String, RoleConfig>,
    runtime_matches: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut candidates = roles
        .iter()
        .filter(|(name, role)| is_worker_role_name(name) && runtime_matches(&role.runtime))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

fn is_worker_role_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    (name.contains("worker") || name.contains("executor")) && !name.contains("reviewer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_builtin_runtime_routes() {
        assert_eq!(normalize_agent_hint("auto"), None);
        assert_eq!(normalize_agent_hint("claude"), Some("worker-cc".into()));
        assert_eq!(normalize_agent_hint("cc"), Some("worker-cc".into()));
        assert_eq!(normalize_agent_hint("codex"), Some("worker-codex".into()));
        assert_eq!(
            normalize_agent_hint("worker-codex"),
            Some("worker-codex".into())
        );
        assert_eq!(normalize_agent_hint("worker-x"), Some("worker-x".into()));
    }

    #[test]
    fn labels_builtin_runtime_routes() {
        assert_eq!(agent_label(None), "auto");
        assert_eq!(agent_label(Some("worker-cc")), "claude");
        assert_eq!(agent_label(Some("worker-codex")), "codex");
        assert_eq!(agent_label(Some("worker-x")), "worker-x");
    }

    #[test]
    fn resolves_claude_alias_to_configured_claude_worker() {
        let roles = HashMap::from([
            (
                "planner".into(),
                RoleConfig {
                    model: "sonnet".into(),
                    runtime: "claude-code".into(),
                    agent_teams: false,
                },
            ),
            (
                "worker-cc-minimax".into(),
                RoleConfig {
                    model: "minimax".into(),
                    runtime: "claude-code".into(),
                    agent_teams: true,
                },
            ),
            (
                "worker-codex".into(),
                RoleConfig {
                    model: "gpt4o".into(),
                    runtime: "codex".into(),
                    agent_teams: false,
                },
            ),
        ]);

        assert_eq!(
            normalize_agent_hint_with_roles("claude", &roles),
            Some("worker-cc-minimax".into())
        );
        assert_eq!(
            default_worker_role(&roles),
            Some("worker-cc-minimax".into())
        );
    }
}
