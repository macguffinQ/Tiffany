//! Environment diagnostics shared by CLI and terminal chat.

use crate::config::{self, Config, RoleConfig};
use crate::runtime;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DoctorLevel {
    Ok,
    Warn,
    Fail,
    Hint,
    Header,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorLine {
    pub level: DoctorLevel,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorReport {
    pub lines: Vec<DoctorLine>,
    pub issue_count: usize,
}

impl DoctorReport {
    pub fn render_text(&self) -> String {
        let mut out = String::from("orchestrator doctor\n");
        for line in &self.lines {
            match line.level {
                DoctorLevel::Ok => out.push_str(&format!("✓ {}\n", line.message)),
                DoctorLevel::Warn => out.push_str(&format!("⚠ {}\n", line.message)),
                DoctorLevel::Fail => out.push_str(&format!("✗ {}\n", line.message)),
                DoctorLevel::Hint => out.push_str(&format!("  hint: {}\n", line.message)),
                DoctorLevel::Header => out.push_str(&format!("\n{}:\n", line.message)),
            }
        }

        out.push('\n');
        if self.issue_count == 0 {
            out.push_str("✓ all required checks passed");
        } else {
            out.push_str(&format!(
                "⚠ {} issue(s) found; fix them before relying on unattended agent runs",
                self.issue_count
            ));
        }
        out
    }
}

pub fn run(config_path: &Path) -> DoctorReport {
    let mut builder = DoctorReportBuilder::default();
    let expanded_config_path = config::expand_home(config_path);

    if expanded_config_path.exists() {
        builder.ok(format!("config found: {}", expanded_config_path.display()));
    } else {
        builder.fail(format!(
            "config missing: {}",
            expanded_config_path.display()
        ));
        builder.hint("run `orchestrator init` to create a starter config");
    }

    let cfg = match Config::load(config_path) {
        Ok(cfg) => {
            builder.ok("config parsed");
            Some(cfg)
        }
        Err(e) => {
            builder.fail(format!("config parse/load failed: {:#}", e));
            None
        }
    };

    if let Some(cfg) = cfg.as_ref() {
        check_paths(&mut builder, cfg);
        check_runtimes(&mut builder, cfg);
        check_providers(&mut builder, cfg);
        check_roles(&mut builder, cfg);
    }

    check_tool(
        &mut builder,
        "git",
        "required for worktree/diff workflows",
        true,
    );
    check_tool(
        &mut builder,
        "zellij",
        "optional terminal multiplexer",
        false,
    );
    check_tool(&mut builder, "tmux", "optional fallback multiplexer", false);

    builder.finish()
}

#[derive(Default)]
struct DoctorReportBuilder {
    lines: Vec<DoctorLine>,
    issue_count: usize,
}

impl DoctorReportBuilder {
    fn ok(&mut self, message: impl Into<String>) {
        self.push(DoctorLevel::Ok, message);
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.push(DoctorLevel::Warn, message);
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.push(DoctorLevel::Fail, message);
        self.issue_count += 1;
    }

    fn hint(&mut self, message: impl Into<String>) {
        self.push(DoctorLevel::Hint, message);
    }

    fn header(&mut self, message: impl Into<String>) {
        self.push(DoctorLevel::Header, message);
    }

    fn push(&mut self, level: DoctorLevel, message: impl Into<String>) {
        self.lines.push(DoctorLine {
            level,
            message: message.into(),
        });
    }

    fn finish(self) -> DoctorReport {
        DoctorReport {
            lines: self.lines,
            issue_count: self.issue_count,
        }
    }
}

fn check_paths(builder: &mut DoctorReportBuilder, cfg: &Config) {
    for (label, path) in [
        ("worktree_base", &cfg.behavior.worktree_base),
        ("session_log_dir", &cfg.behavior.session_log_dir),
    ] {
        if path.is_dir() {
            builder.ok(format!("{label}: {}", path.display()));
        } else {
            builder.fail(format!("{label} is not a directory: {}", path.display()));
        }
    }

    if let Some(parent) = cfg.behavior.db_path.parent() {
        if parent.is_dir() {
            builder.ok(format!("db parent: {}", parent.display()));
        } else {
            builder.fail(format!("db parent missing: {}", parent.display()));
        }
    }
}

fn check_runtimes(builder: &mut DoctorReportBuilder, cfg: &Config) {
    if cfg.runtimes.is_empty() {
        builder.fail("no runtimes configured");
        return;
    }

    builder.header("Runtimes");
    let mut runtimes = cfg.runtimes.iter().collect::<Vec<_>>();
    runtimes.sort_by(|a, b| a.0.cmp(b.0));
    for (name, rt) in runtimes {
        if rt.kind == "subprocess" {
            let binary = rt.binary.as_deref().unwrap_or(name);
            match which::which(binary) {
                Ok(path) => builder.ok(format!("{name}: `{binary}` -> {}", path.display())),
                Err(_) => {
                    builder.fail(format!("{name}: `{binary}` not found on PATH"));
                    builder.hint(format!(
                        "install it or set runtimes.{name}.binary in config.yaml"
                    ));
                }
            }
        } else {
            builder.ok(format!("{name}: runtime type `{}`", rt.kind));
        }
    }
}

fn check_providers(builder: &mut DoctorReportBuilder, cfg: &Config) {
    if cfg.providers.is_empty() {
        builder.warn("no providers configured");
        return;
    }

    builder.header("Providers");
    let mut providers = cfg.providers.iter().collect::<Vec<_>>();
    providers.sort_by(|a, b| a.0.cmp(b.0));
    for (name, provider) in providers {
        if provider.kind == "ollama" {
            builder.ok(format!(
                "{name}: local provider {}",
                provider.base_url.as_deref().unwrap_or("(default)")
            ));
            continue;
        }

        match provider.api_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => builder.ok(format!("{name}: api key present")),
            _ => {
                builder.fail(format!("{name}: api key missing"));
                builder.hint(
                    "set the provider key in config or export the matching environment variable",
                );
            }
        }
    }
}

fn check_roles(builder: &mut DoctorReportBuilder, cfg: &Config) {
    builder.header("Roles");
    for role_name in ["planner", "critic", "reviewer"] {
        check_named_role(builder, cfg, role_name);
    }

    match runtime::default_worker_role(&cfg.roles) {
        Some(role_name) => builder.ok(format!("default worker: {role_name}")),
        None => {
            builder.fail("no worker role configured (add a role containing worker or executor)")
        }
    }

    let mut roles = cfg.roles.iter().collect::<Vec<_>>();
    roles.sort_by(|a, b| a.0.cmp(b.0));
    for (role_name, role) in roles {
        check_role_binding(builder, cfg, role_name, role);
    }
}

fn check_named_role(builder: &mut DoctorReportBuilder, cfg: &Config, role_name: &str) {
    if !cfg.roles.contains_key(role_name) {
        builder.fail(format!("{role_name}: missing role config"));
    }
}

fn check_role_binding(
    builder: &mut DoctorReportBuilder,
    cfg: &Config,
    role_name: &str,
    role: &RoleConfig,
) {
    let model_ok = cfg.models.iter().any(|model| model.id == role.model);
    let runtime_ok = cfg.runtimes.contains_key(&role.runtime);
    if model_ok && runtime_ok {
        builder.ok(format!(
            "{role_name}: model={} runtime={}",
            role.model, role.runtime
        ));
        return;
    }
    if !model_ok {
        builder.fail(format!(
            "{role_name}: model `{}` is not defined in models",
            role.model
        ));
    }
    if !runtime_ok {
        builder.fail(format!(
            "{role_name}: runtime `{}` is not defined in runtimes",
            role.runtime
        ));
    }
}

fn check_tool(builder: &mut DoctorReportBuilder, binary: &str, description: &str, required: bool) {
    match which::which(binary) {
        Ok(path) => builder.ok(format!("{binary}: {} ({description})", path.display())),
        Err(_) if required => builder.fail(format!("{binary}: not found ({description})")),
        Err(_) => builder.warn(format!("{binary}: not found ({description})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BehaviorConfig, Config, ModelConfig, ProviderConfig, RoleConfig, RuntimeConfig,
    };
    use std::collections::HashMap;

    #[test]
    fn report_renders_icons_and_summary() {
        let report = DoctorReport {
            lines: vec![
                DoctorLine {
                    level: DoctorLevel::Ok,
                    message: "config parsed".into(),
                },
                DoctorLine {
                    level: DoctorLevel::Fail,
                    message: "openai: api key missing".into(),
                },
                DoctorLine {
                    level: DoctorLevel::Hint,
                    message: "export OPENAI_API_KEY".into(),
                },
            ],
            issue_count: 1,
        };

        let rendered = report.render_text();

        assert!(rendered.contains("✓ config parsed"));
        assert!(rendered.contains("✗ openai: api key missing"));
        assert!(rendered.contains("hint: export OPENAI_API_KEY"));
        assert!(rendered.contains("1 issue(s) found"));
    }

    #[test]
    fn role_check_reports_missing_model_and_runtime() {
        let mut cfg = Config {
            behavior: BehaviorConfig::default(),
            ..Config::default()
        };
        cfg.roles.insert(
            "planner".into(),
            RoleConfig {
                model: "missing-model".into(),
                runtime: "missing-runtime".into(),
                agent_teams: false,
            },
        );

        let mut builder = DoctorReportBuilder::default();
        check_roles(&mut builder, &cfg);
        let report = builder.finish();
        let rendered = report.render_text();

        assert!(rendered.contains("planner: model `missing-model` is not defined"));
        assert!(rendered.contains("planner: runtime `missing-runtime` is not defined"));
        assert!(report.issue_count >= 2);
    }

    #[test]
    fn runtime_provider_checks_count_actionable_issues() {
        let mut runtimes = HashMap::new();
        runtimes.insert(
            "local-api".into(),
            RuntimeConfig {
                kind: "direct".into(),
                binary: None,
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        let mut providers = HashMap::new();
        providers.insert(
            "openai".into(),
            ProviderConfig {
                kind: "openai".into(),
                api_key: None,
                base_url: None,
            },
        );
        let cfg = Config {
            providers,
            runtimes,
            models: vec![ModelConfig {
                id: "gpt".into(),
                provider: "openai".into(),
                name: "GPT".into(),
            }],
            behavior: BehaviorConfig::default(),
            ..Config::default()
        };

        let mut builder = DoctorReportBuilder::default();
        check_runtimes(&mut builder, &cfg);
        check_providers(&mut builder, &cfg);
        let report = builder.finish();
        let rendered = report.render_text();

        assert!(rendered.contains("local-api: runtime type `direct`"));
        assert!(rendered.contains("openai: api key missing"));
        assert_eq!(report.issue_count, 1);
    }
}
