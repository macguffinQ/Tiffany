//! Config loading and types.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    pub providers: HashMap<String, ProviderConfig>,
    pub runtimes: HashMap<String, RuntimeConfig>,
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub roles: HashMap<String, RoleConfig>,
    #[serde(default)]
    pub overrides: Vec<OverrideConfig>,
    pub behavior: BehaviorConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub kind: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuntimeConfig {
    #[serde(rename = "type")]
    pub kind: String,
    pub binary: Option<String>,
    #[serde(default)]
    pub supports_mcp: bool,
    #[serde(default)]
    pub supports_agent_teams: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelConfig {
    pub id: String,
    pub provider: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoleConfig {
    pub model: String,
    pub runtime: String,
    #[serde(default)]
    pub agent_teams: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OverrideConfig {
    pub tag: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenPlan {
    /// Master switch
    #[serde(default)]
    pub enabled: bool,
    /// Daily token limit (input + output, summed across all providers)
    pub daily_limit: Option<u64>,
    /// Monthly USD limit
    pub monthly_limit_usd: Option<f64>,
    /// Warn at this percentage of limit (0-100). Default: 80
    #[serde(default = "default_warn_percent")]
    pub warn_at_percent: u8,
    /// Per-provider overrides (in case some are cheaper than others)
    #[serde(default)]
    pub per_provider: HashMap<String, u64>,
}

impl Default for TokenPlan {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_limit: None,
            monthly_limit_usd: None,
            warn_at_percent: default_warn_percent(),
            per_provider: HashMap::new(),
        }
    }
}

fn default_warn_percent() -> u8 {
    80
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BehaviorConfig {
    #[serde(default = "default_worktree_base")]
    pub worktree_base: PathBuf,
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
    #[serde(default = "default_session_log_dir")]
    pub session_log_dir: PathBuf,
    #[serde(default = "default_true")]
    pub enable_critic: bool,
    #[serde(default = "default_true")]
    pub enable_reviewer: bool,
    #[serde(default = "default_max_replan")]
    pub max_replan: u32,
    #[serde(default)]
    pub enable_ab_judge: bool,
    #[serde(default = "default_mux")]
    pub mux: MuxKind,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Pass --permission-mode bypassPermissions to Claude Code so it
    /// can edit files and run bash without prompting. Default: true
    /// (recommended for orchestrator runs in isolated worktrees).
    #[serde(default = "default_true")]
    pub cc_bypass_permissions: bool,
    /// Token plan / budget tracking (daily / monthly limits)
    #[serde(default)]
    pub token_plan: TokenPlan,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            worktree_base: default_worktree_base(),
            db_path: default_db_path(),
            session_log_dir: default_session_log_dir(),
            enable_critic: true,
            enable_reviewer: true,
            max_replan: default_max_replan(),
            enable_ab_judge: false,
            mux: default_mux(),
            log_level: default_log_level(),
            cc_bypass_permissions: true,
            token_plan: TokenPlan::default(),
        }
    }
}

fn default_worktree_base() -> PathBuf {
    PathBuf::from("~/.orchestrator/worktrees")
}

fn default_db_path() -> PathBuf {
    PathBuf::from("~/.orchestrator/state.db")
}

fn default_session_log_dir() -> PathBuf {
    PathBuf::from("~/.orchestrator/sessions")
}

fn default_true() -> bool {
    true
}

fn default_max_replan() -> u32 {
    2
}

fn default_mux() -> MuxKind {
    MuxKind::Zellij
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MuxKind {
    Zellij,
    Tmux,
    #[default]
    None,
}

impl Config {
    /// Load config from a YAML file, expanding ${ENV} placeholders.
    /// Missing env vars are substituted with empty string (not an error),
    /// so the config loads even before API keys are set.
    pub fn load(path: &Path) -> Result<Self> {
        let path = expand_home(path);
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        // Missing env vars → empty string (don't fail).
        let expanded = shellexpand::env_with_context(
            &raw,
            |k| -> Result<Option<std::borrow::Cow<'_, str>>, std::env::VarError> {
                Ok(Some(
                    std::env::var(k)
                        .ok()
                        .map(std::convert::Into::into)
                        .unwrap_or_else(|| std::borrow::Cow::Borrowed("")),
                ))
            },
        )
        .map_err(|e| anyhow::anyhow!("expanding env vars: {}", e))?
        .into_owned();
        let cfg: Self = serde_yaml::from_str(&expanded)
            .with_context(|| format!("parsing config at {}", path.display()))?;
        cfg.into_resolved()
    }

    /// Resolve all `~` and relative paths to absolute paths.
    pub fn into_resolved(mut self) -> Result<Self> {
        self.behavior.worktree_base = expand_home(&self.behavior.worktree_base);
        self.behavior.db_path = expand_home(&self.behavior.db_path);
        self.behavior.session_log_dir = expand_home(&self.behavior.session_log_dir);

        // Ensure dirs exist
        std::fs::create_dir_all(&self.behavior.worktree_base)
            .with_context(|| format!("creating {}", self.behavior.worktree_base.display()))?;
        std::fs::create_dir_all(&self.behavior.session_log_dir)
            .with_context(|| format!("creating {}", self.behavior.session_log_dir.display()))?;
        if let Some(parent) = self.behavior.db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        Ok(self)
    }

    pub fn runtime_config(&self, runtime_id: &str) -> Option<&RuntimeConfig> {
        self.runtimes.get(runtime_id).or_else(|| {
            runtime_aliases(runtime_id)
                .iter()
                .find_map(|alias| self.runtimes.get(*alias))
        })
    }

    /// Derive a stable internal model id from a provider API model name.
    ///
    /// The generated id is meant for user-facing setup flows where asking for
    /// a separate "model id" adds friction. If the preferred id is already
    /// bound to the same provider/model pair, it is reused. If it conflicts,
    /// the provider id and then a numeric suffix are added.
    pub fn derive_model_id(&self, provider: &str, model_name: &str) -> String {
        let provider_slug = slugify_config_id(provider).unwrap_or_else(|| "provider".into());
        let model_slug = slugify_config_id(model_name).unwrap_or_else(|| "model".into());
        let preferred = model_slug.clone();
        if self.model_id_available_or_matching(&preferred, provider, model_name) {
            return preferred;
        }

        let prefixed = format!("{provider_slug}-{model_slug}");
        if self.model_id_available_or_matching(&prefixed, provider, model_name) {
            return prefixed;
        }

        for idx in 2.. {
            let candidate = format!("{prefixed}-{idx}");
            if self.model_id_available_or_matching(&candidate, provider, model_name) {
                return candidate;
            }
        }
        unreachable!("unbounded numeric suffix search should always return")
    }

    fn model_id_available_or_matching(&self, id: &str, provider: &str, model_name: &str) -> bool {
        match self.models.iter().find(|model| model.id == id) {
            None => true,
            Some(existing) => existing.provider == provider && existing.name == model_name,
        }
    }

    /// Write a default config to `~/.orchestrator/config.yaml` if none exists.
    pub fn init_default() -> Result<PathBuf> {
        let home = home::home_dir().context("could not determine home directory")?;
        let dir = home.join(".orchestrator");
        std::fs::create_dir_all(&dir)?;
        let target = dir.join("config.yaml");
        if target.exists() {
            anyhow::bail!("config already exists at {}", target.display());
        }
        let example = include_str!("../config.example.yaml");
        std::fs::write(&target, example)?;
        Ok(target)
    }

    /// Add or replace a model entry in the YAML config without expanding env placeholders.
    pub fn write_model_to_config_file(path: &Path, model_cfg: &ModelConfig) -> Result<()> {
        validate_config_id("model id", &model_cfg.id)?;
        validate_config_id("provider id", &model_cfg.provider)?;
        let (path, mut yaml) = read_raw_config_yaml(path)?;
        let root = yaml_root_mapping_mut(&mut yaml)?;
        let models_key = serde_yaml::Value::String("models".into());
        if !root.contains_key(&models_key) {
            root.insert(models_key.clone(), serde_yaml::Value::Sequence(Vec::new()));
        }
        let models = root
            .get_mut(&models_key)
            .and_then(serde_yaml::Value::as_sequence_mut)
            .ok_or_else(|| anyhow::anyhow!("config field 'models' must be a sequence"))?;

        let model_value = serde_yaml::to_value(model_cfg)?;
        if let Some(existing) = models.iter_mut().find(|item| {
            item.as_mapping()
                .and_then(|mapping| mapping.get(serde_yaml::Value::String("id".into())))
                .and_then(serde_yaml::Value::as_str)
                == Some(model_cfg.id.as_str())
        }) {
            *existing = model_value;
        } else {
            models.push(model_value);
        }

        write_raw_config_yaml(&path, &yaml)
    }

    /// Add or replace a role entry in the YAML config without expanding env placeholders.
    pub fn write_role_to_config_file(path: &Path, role: &str, role_cfg: &RoleConfig) -> Result<()> {
        validate_config_id("role name", role)?;
        validate_config_id("model id", &role_cfg.model)?;
        validate_config_id("runtime id", &role_cfg.runtime)?;
        let (path, mut yaml) = read_raw_config_yaml(path)?;
        let root = yaml_root_mapping_mut(&mut yaml)?;
        let roles_key = serde_yaml::Value::String("roles".into());
        if !root.contains_key(&roles_key) {
            root.insert(
                roles_key.clone(),
                serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
            );
        }
        let roles = root
            .get_mut(&roles_key)
            .and_then(serde_yaml::Value::as_mapping_mut)
            .ok_or_else(|| anyhow::anyhow!("config field 'roles' must be a mapping"))?;

        roles.insert(
            serde_yaml::Value::String(role.to_string()),
            serde_yaml::to_value(role_cfg)?,
        );
        write_raw_config_yaml(&path, &yaml)
    }

    /// Add or replace a provider API key without expanding env placeholders.
    pub fn write_provider_key_to_config_file(
        path: &Path,
        provider: &str,
        kind: &str,
        api_key: &str,
    ) -> Result<()> {
        validate_config_id("provider id", provider)?;
        validate_config_id("provider type", kind)?;
        let (path, mut yaml) = read_raw_config_yaml(path)?;
        let provider_map = raw_provider_mapping_mut(&mut yaml, provider)?;
        provider_map.insert(
            serde_yaml::Value::String("type".into()),
            serde_yaml::Value::String(kind.to_string()),
        );
        provider_map.insert(
            serde_yaml::Value::String("api_key".into()),
            serde_yaml::Value::String(api_key.to_string()),
        );
        write_raw_config_yaml(&path, &yaml)
    }

    /// Add or replace a provider base URL without expanding env placeholders.
    pub fn write_provider_endpoint_to_config_file(
        path: &Path,
        provider: &str,
        kind: &str,
        url: &str,
    ) -> Result<()> {
        validate_config_id("provider id", provider)?;
        validate_config_id("provider type", kind)?;
        if url.trim().is_empty() {
            anyhow::bail!("provider endpoint cannot be empty");
        }
        let (path, mut yaml) = read_raw_config_yaml(path)?;
        let provider_map = raw_provider_mapping_mut(&mut yaml, provider)?;
        provider_map.insert(
            serde_yaml::Value::String("type".into()),
            serde_yaml::Value::String(kind.to_string()),
        );
        provider_map.insert(
            serde_yaml::Value::String("base_url".into()),
            serde_yaml::Value::String(url.to_string()),
        );
        write_raw_config_yaml(&path, &yaml)
    }

    /// Delete a provider entry from the YAML config without expanding env placeholders.
    pub fn delete_provider_from_config_file(path: &Path, provider: &str) -> Result<bool> {
        validate_config_id("provider id", provider)?;
        let (path, mut yaml) = read_raw_config_yaml(path)?;
        let root = yaml_root_mapping_mut(&mut yaml)?;
        let providers_key = serde_yaml::Value::String("providers".into());
        let Some(providers_value) = root.get_mut(&providers_key) else {
            return Ok(false);
        };
        let removed = providers_value
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("config field 'providers' must be a mapping"))?
            .remove(serde_yaml::Value::String(provider.to_string()))
            .is_some();
        if removed {
            write_raw_config_yaml(&path, &yaml)?;
        }
        Ok(removed)
    }
}

pub fn expand_home(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = home::home_dir() {
            return home.join(rest);
        }
    } else if s == "~" {
        if let Some(home) = home::home_dir() {
            return home;
        }
    }
    p.to_path_buf()
}

fn read_raw_config_yaml(path: &Path) -> Result<(PathBuf, serde_yaml::Value)> {
    let path = expand_home(path);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok((path, empty_raw_config_yaml()));
        }
        Err(err) => {
            return Err(err).with_context(|| format!("reading config at {}", path.display()));
        }
    };
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw)
        .with_context(|| format!("parsing config at {}", path.display()))?;
    Ok((path, yaml))
}

fn empty_raw_config_yaml() -> serde_yaml::Value {
    let mut root = serde_yaml::Mapping::new();
    root.insert(
        serde_yaml::Value::String("providers".into()),
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
    );
    root.insert(
        serde_yaml::Value::String("runtimes".into()),
        default_raw_runtimes_yaml(),
    );
    root.insert(
        serde_yaml::Value::String("models".into()),
        serde_yaml::Value::Sequence(Vec::new()),
    );
    root.insert(
        serde_yaml::Value::String("roles".into()),
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
    );
    root.insert(
        serde_yaml::Value::String("behavior".into()),
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
    );
    serde_yaml::Value::Mapping(root)
}

fn default_raw_runtimes_yaml() -> serde_yaml::Value {
    let mut runtimes = serde_yaml::Mapping::new();
    runtimes.insert(
        serde_yaml::Value::String("claude-code".into()),
        raw_runtime_yaml("subprocess", Some("claude"), true, true),
    );
    runtimes.insert(
        serde_yaml::Value::String("codex".into()),
        raw_runtime_yaml("subprocess", Some("codex"), false, false),
    );
    runtimes.insert(
        serde_yaml::Value::String("direct".into()),
        raw_runtime_yaml("sdk", None, false, false),
    );
    serde_yaml::Value::Mapping(runtimes)
}

fn raw_runtime_yaml(
    kind: &str,
    binary: Option<&str>,
    supports_mcp: bool,
    supports_agent_teams: bool,
) -> serde_yaml::Value {
    let mut runtime = serde_yaml::Mapping::new();
    runtime.insert(
        serde_yaml::Value::String("type".into()),
        serde_yaml::Value::String(kind.to_string()),
    );
    if let Some(binary) = binary {
        runtime.insert(
            serde_yaml::Value::String("binary".into()),
            serde_yaml::Value::String(binary.to_string()),
        );
    }
    runtime.insert(
        serde_yaml::Value::String("supports_mcp".into()),
        serde_yaml::Value::Bool(supports_mcp),
    );
    runtime.insert(
        serde_yaml::Value::String("supports_agent_teams".into()),
        serde_yaml::Value::Bool(supports_agent_teams),
    );
    serde_yaml::Value::Mapping(runtime)
}

pub fn runtime_aliases(runtime_id: &str) -> &'static [&'static str] {
    match runtime_id {
        "codex" => &["codex"],
        "claude-code" | "claude" | "cc" => &["claude-code", "claude", "cc"],
        _ => &[],
    }
}

fn yaml_root_mapping_mut(yaml: &mut serde_yaml::Value) -> Result<&mut serde_yaml::Mapping> {
    yaml.as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("config root must be a YAML mapping"))
}

fn raw_provider_mapping_mut<'a>(
    yaml: &'a mut serde_yaml::Value,
    provider: &str,
) -> Result<&'a mut serde_yaml::Mapping> {
    let root = yaml_root_mapping_mut(yaml)?;
    let providers_key = serde_yaml::Value::String("providers".into());
    if !root.contains_key(&providers_key) {
        root.insert(
            providers_key.clone(),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    let providers = root
        .get_mut(&providers_key)
        .and_then(serde_yaml::Value::as_mapping_mut)
        .ok_or_else(|| anyhow::anyhow!("config field 'providers' must be a mapping"))?;

    let provider_key = serde_yaml::Value::String(provider.to_string());
    if !providers.contains_key(&provider_key) {
        providers.insert(
            provider_key.clone(),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    providers
        .get_mut(&provider_key)
        .and_then(serde_yaml::Value::as_mapping_mut)
        .ok_or_else(|| anyhow::anyhow!("config field 'providers.{provider}' must be a mapping"))
}

fn write_raw_config_yaml(path: &Path, yaml: &serde_yaml::Value) -> Result<()> {
    let body = serde_yaml::to_string(yaml)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    }
    std::fs::write(path, body).with_context(|| format!("writing config to {}", path.display()))
}

fn validate_config_id(kind: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{} cannot be empty", kind);
    }
    if value.chars().any(char::is_whitespace) {
        anyhow::bail!("{} '{}' cannot contain whitespace", kind, value);
    }
    Ok(())
}

fn slugify_config_id(value: &str) -> Option<String> {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_example_config() {
        // Even without env vars set, parsing should succeed (vars → empty).
        let yaml = include_str!("../config.example.yaml");
        let expanded = shellexpand::env_with_context(
            yaml,
            |k| -> Result<Option<std::borrow::Cow<'_, str>>, std::env::VarError> {
                Ok(Some(
                    std::env::var(k)
                        .ok()
                        .map(std::convert::Into::into)
                        .unwrap_or_else(|| std::borrow::Cow::Borrowed("")),
                ))
            },
        )
        .unwrap()
        .into_owned();
        let _cfg: Config = serde_yaml::from_str(&expanded).unwrap();
    }

    #[test]
    fn token_plan_default_warns_at_eighty_percent() {
        let plan = TokenPlan::default();

        assert!(!plan.enabled);
        assert_eq!(plan.warn_at_percent, 80);
    }

    #[test]
    fn raw_role_write_preserves_env_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "providers:\n  openai:\n    type: openai\n    api_key: ${OPENAI_API_KEY}\nruntimes:\n  codex:\n    type: subprocess\n    binary: codex\nmodels:\n  - id: gpt4o\n    provider: openai\n    name: gpt-4o\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        Config::write_role_to_config_file(
            &path,
            "critic",
            &RoleConfig {
                model: "gpt4o".into(),
                runtime: "codex".into(),
                agent_teams: false,
            },
        )
        .unwrap();

        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("${OPENAI_API_KEY}"));
        assert!(body.contains("critic:"));
        assert!(body.contains("model: gpt4o"));
    }

    #[test]
    fn raw_model_write_updates_existing_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "providers:\n  openai:\n    type: openai\nmodels:\n  - id: glm\n    provider: openai\n    name: old\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        Config::write_model_to_config_file(
            &path,
            &ModelConfig {
                id: "glm".into(),
                provider: "openai".into(),
                name: "glm-5.1".into(),
            },
        )
        .unwrap();

        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("name: glm-5.1"));
        assert_eq!(body.matches("id: glm").count(), 1);
    }

    #[test]
    fn derive_model_id_reuses_matching_and_suffixes_conflicts() {
        let cfg = Config {
            models: vec![
                ModelConfig {
                    id: "minimax-m3".into(),
                    provider: "minimax".into(),
                    name: "MiniMax-M3".into(),
                },
                ModelConfig {
                    id: "glm-5-1".into(),
                    provider: "openai".into(),
                    name: "different".into(),
                },
                ModelConfig {
                    id: "z-ai-glm-5-1".into(),
                    provider: "z-ai".into(),
                    name: "different".into(),
                },
            ],
            ..Default::default()
        };

        assert_eq!(cfg.derive_model_id("minimax", "MiniMax-M3"), "minimax-m3");
        assert_eq!(cfg.derive_model_id("z-ai", "glm-5.1"), "z-ai-glm-5-1-2");
    }

    #[test]
    fn raw_provider_key_write_preserves_other_env_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "providers:\n  openai:\n    type: openai\n    api_key: ${OPENAI_API_KEY}\n  anthropic:\n    type: anthropic\n    api_key: ${ANTHROPIC_API_KEY}\nmodels: []\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        Config::write_provider_key_to_config_file(&path, "openai", "openai", "$OPENAI_API_KEY")
            .unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("api_key: $OPENAI_API_KEY"));
        assert!(body.contains("${ANTHROPIC_API_KEY}"));
    }

    #[test]
    fn raw_provider_key_write_creates_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.yaml");

        Config::write_provider_key_to_config_file(&path, "minimax", "openai", "$MINIMAX_API_KEY")
            .unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("providers:"));
        assert!(body.contains("minimax:"));
        assert!(body.contains("type: openai"));
        assert!(body.contains("api_key: $MINIMAX_API_KEY"));
        assert!(body.contains("runtimes:"));
        assert!(body.contains("codex:"));
        assert!(body.contains("models: []"));
        assert!(body.contains("roles: {}"));

        let cfg = Config::load(&path).unwrap();
        assert!(cfg.runtimes.contains_key("codex"));
    }

    #[test]
    fn raw_provider_endpoint_write_keeps_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "providers:\n  openai:\n    type: openai\n    api_key: ${OPENAI_API_KEY}\nmodels: []\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        Config::write_provider_endpoint_to_config_file(
            &path,
            "openai",
            "openai",
            "https://api.openai.com/v1",
        )
        .unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("${OPENAI_API_KEY}"));
        assert!(body.contains("base_url: https://api.openai.com/v1"));
    }

    #[test]
    fn raw_provider_delete_removes_only_selected_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "providers:\n  openai:\n    type: openai\n    api_key: ${OPENAI_API_KEY}\n  minimax:\n    type: openai\n    api_key: ${MINIMAX_API_KEY}\nmodels: []\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        let removed = Config::delete_provider_from_config_file(&path, "minimax").unwrap();

        assert!(removed);
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("openai:"));
        assert!(body.contains("api_key: ${OPENAI_API_KEY}"));
        assert!(!body.contains("minimax:"));
        assert!(!body.contains("MINIMAX_API_KEY"));
    }
}
