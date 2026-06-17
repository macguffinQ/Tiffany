//! Capability router: 3-tier priority resolution.
//!
//! 1. CLI flag override (passed in via `cli_override`)
//! 2. Task tag → config override mapping
//! 3. Default role assignment

use crate::config::{ModelConfig, OverrideConfig, RoleConfig};
use crate::core::types::Task;
use crate::runtime;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ResolvedAssignment {
    pub model: String,
    pub provider: Option<String>,
    pub runtime: String,
    pub agent_teams: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelConfig;

    #[test]
    fn new_with_models_resolves_role_model_ids_to_cli_model_names() {
        let roles = HashMap::from([(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".into(),
                runtime: "claude-code".into(),
                agent_teams: true,
            },
        )]);
        let models = vec![ModelConfig {
            id: "sonnet".into(),
            provider: "anthropic".into(),
            name: "claude-sonnet-4-6".into(),
        }];
        let router = CapabilityRouter::new_with_models(&roles, &[], &models);
        let resolved = router.resolve(&Task::new("task")).expect("resolved route");

        assert_eq!(resolved.model, "claude-sonnet-4-6");
        assert_eq!(resolved.provider.as_deref(), Some("anthropic"));
        assert_eq!(resolved.runtime, "claude-code");
        assert!(resolved.agent_teams);
    }

    #[test]
    fn default_route_can_use_named_claude_worker_when_worker_cc_is_absent() {
        let roles = HashMap::from([
            (
                "planner".to_string(),
                RoleConfig {
                    model: "sonnet".into(),
                    runtime: "claude-code".into(),
                    agent_teams: false,
                },
            ),
            (
                "worker-cc-minimax".to_string(),
                RoleConfig {
                    model: "minimax".into(),
                    runtime: "claude-code".into(),
                    agent_teams: true,
                },
            ),
        ]);
        let router = CapabilityRouter::new_with_models(&roles, &[], &[]);
        let resolved = router.resolve(&Task::new("task")).expect("resolved route");

        assert_eq!(resolved.model, "minimax");
        assert_eq!(resolved.runtime, "claude-code");
        assert!(resolved.agent_teams);
    }
}

pub struct CapabilityRouter {
    roles: HashMap<String, RoleConfig>,
    models: HashMap<String, ModelConfig>,
    tag_overrides: HashMap<String, String>,
}

impl CapabilityRouter {
    pub fn new(roles: &HashMap<String, RoleConfig>, overrides: &[OverrideConfig]) -> Self {
        Self::new_inner(roles.clone(), overrides, HashMap::new())
    }

    pub fn new_with_models(
        roles: &HashMap<String, RoleConfig>,
        overrides: &[OverrideConfig],
        models: &[ModelConfig],
    ) -> Self {
        let models = models
            .iter()
            .cloned()
            .map(|model| (model.id.clone(), model))
            .collect::<HashMap<_, _>>();
        Self::new_inner(roles.clone(), overrides, models)
    }

    fn new_inner(
        roles: HashMap<String, RoleConfig>,
        overrides: &[OverrideConfig],
        models: HashMap<String, ModelConfig>,
    ) -> Self {
        let tag_overrides = overrides
            .iter()
            .map(|o| (o.tag.clone(), o.role.clone()))
            .collect();
        Self {
            roles,
            models,
            tag_overrides,
        }
    }

    fn assignment_for(&self, rc: &RoleConfig) -> ResolvedAssignment {
        if let Some(model) = self.models.get(rc.model.as_str()) {
            return ResolvedAssignment {
                model: model.name.clone(),
                provider: Some(model.provider.clone()),
                runtime: rc.runtime.clone(),
                agent_teams: rc.agent_teams,
            };
        }
        ResolvedAssignment {
            model: rc.model.clone(),
            provider: None,
            runtime: rc.runtime.clone(),
            agent_teams: rc.agent_teams,
        }
    }

    /// Resolve an assignment for `task`. The optional `cli_override` is the
    /// highest-priority override (e.g. `--planner opus` from the CLI).
    pub fn resolve(&self, task: &Task) -> Result<ResolvedAssignment> {
        // 1. task.agent_hint (set by CLI flags via main.rs → task.agent_hint)
        if let Some(hint) = &task.agent_hint {
            if let Some(rc) = self.roles.get(hint) {
                return Ok(self.assignment_for(rc));
            }
        }

        // 2. tag-based override
        for tag in &task.tags {
            if let Some(role_name) = self.tag_overrides.get(tag) {
                if let Some(rc) = self.roles.get(role_name) {
                    return Ok(self.assignment_for(rc));
                }
            }
        }

        // 3. default: configured Claude worker, then configured Codex worker.
        let Some(default_key) = runtime::default_worker_role(&self.roles) else {
            return Err(anyhow!("no default worker role in config"));
        };
        let rc = self.roles.get(&default_key).unwrap();
        Ok(self.assignment_for(rc))
    }
}
