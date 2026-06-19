//! Environment diagnostics shared by CLI and terminal chat.

use crate::config::{self, Config, RoleConfig};
use crate::runtime;
use crate::tiffany_install;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

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

        let issue_summary = self.issue_summary();
        if !issue_summary.is_empty() {
            out.push_str("\nIssue summary:\n");
            for item in issue_summary {
                out.push_str(&format!("- {item}\n"));
            }
        }

        let next_steps = self.next_steps();
        if !next_steps.is_empty() {
            out.push_str("\nNext steps:\n");
            for (idx, step) in next_steps.iter().enumerate() {
                out.push_str(&format!("{}. {step}\n", idx + 1));
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

    fn issue_summary(&self) -> Vec<String> {
        let fail_messages = self
            .lines
            .iter()
            .filter(|line| line.level == DoctorLevel::Fail)
            .map(|line| line.message.as_str())
            .collect::<Vec<_>>();
        if fail_messages.is_empty() {
            return Vec::new();
        }

        let mut provider_auth = Vec::new();
        let mut runtime_tools = Vec::new();
        let mut role_wiring = Vec::new();
        let mut config_state = Vec::new();
        let mut other = Vec::new();

        for message in fail_messages {
            if let Some((provider, envs)) = provider_env_unset(message) {
                provider_auth.push(format!("{provider}={envs}"));
            } else if let Some(provider) = provider_key_missing(message) {
                provider_auth.push(provider.to_string());
            } else if message.contains("not found on PATH") {
                runtime_tools.push(message.to_string());
            } else if message.contains("missing role config")
                || message.contains("no worker role configured")
                || message.contains("model `")
                || message.contains("runtime `")
            {
                role_wiring.push(message.to_string());
            } else if message.contains("config missing")
                || message.contains("config parse/load failed")
                || message.contains("provider `")
                || message.contains("no models configured")
                || message.contains("no runtimes configured")
            {
                config_state.push(message.to_string());
            } else {
                other.push(message.to_string());
            }
        }

        let mut summary = Vec::new();
        if !provider_auth.is_empty() {
            provider_auth.sort();
            provider_auth.dedup();
            summary.push(format!(
                "Provider auth missing for {} provider(s): {}",
                provider_auth.len(),
                join_limited(&provider_auth, 4)
            ));
        }
        if !runtime_tools.is_empty() {
            summary.push(format!(
                "Runtime binaries missing: {}",
                join_limited(&runtime_tools, 3)
            ));
        }
        if !role_wiring.is_empty() {
            summary.push(format!(
                "Role/model wiring needs attention: {}",
                join_limited(&role_wiring, 3)
            ));
        }
        if !config_state.is_empty() {
            summary.push(format!(
                "Config needs attention: {}",
                join_limited(&config_state, 3)
            ));
        }
        for item in other.into_iter().take(3) {
            summary.push(item);
        }
        summary
    }

    fn next_steps(&self) -> Vec<String> {
        let mut steps = Vec::new();
        if self.issue_count == 0 {
            steps.push(
                "Start the UI with `tiffany-loop` or `./scripts/tiffany-dev` from source."
                    .to_string(),
            );
            return steps;
        }

        let messages = self
            .lines
            .iter()
            .map(|line| line.message.as_str())
            .collect::<Vec<_>>();

        if messages.iter().any(|message| {
            message.contains("config missing") || message.contains("config parse/load failed")
        }) {
            push_unique(
                &mut steps,
                "Run `orchestrator setup` to create providers, models, roles, and local paths.",
            );
        }
        if messages.iter().any(|message| {
            message.contains("tiffany-loop binary not found")
                || message.contains("tiffany binary not found")
                || message.contains("ORCHESTRATOR_LEGACY_TUI")
                || message.contains("not resolvable")
        }) {
            push_unique(
                &mut steps,
                "Install `tiffany-loop` or run `./scripts/tiffany-dev` from a source checkout.",
            );
        }
        if messages.iter().any(|message| {
            message.contains("api key missing")
                || message.contains("api key env")
                || message.contains("provider `")
                || message.contains("provider auth")
        }) {
            push_unique(
                &mut steps,
                "Configure provider auth: run `tiffany-loop` then `/provider`, or `orchestrator config provider setup <provider> --env <ENV_NAME>`.",
            );
        }
        if messages.iter().any(|message| {
            message.contains("missing role config")
                || message.contains("no worker role")
                || message.contains("model `")
                || message.contains("runtime `")
        }) {
            push_unique(
                &mut steps,
                "Register roles with `/role` in the TUI or `orchestrator roles register <role> --model <model-id> --runtime <runtime-id>`.",
            );
        }
        if messages.iter().any(|message| {
            message.contains("not found on PATH")
                || message.contains("claude: not found")
                || message.contains("codex: not found")
        }) {
            push_unique(
                &mut steps,
                "Install the selected worker CLI (`claude` or `codex`) or update role runtime bindings.",
            );
        }

        if steps.is_empty() {
            steps.push("Fix the ✗ lines above, then rerun `orchestrator doctor`.".to_string());
        } else {
            push_unique(
                &mut steps,
                "Rerun `orchestrator doctor` until required checks pass.",
            );
        }
        steps.truncate(4);
        steps
    }
}

fn provider_env_unset(message: &str) -> Option<(&str, &str)> {
    let (provider, rest) = message.split_once(": api key env ")?;
    let envs = rest.strip_suffix(" unset")?;
    Some((provider, envs))
}

fn provider_key_missing(message: &str) -> Option<&str> {
    let (provider, rest) = message.split_once(": ")?;
    if rest == "api key missing" {
        Some(provider)
    } else {
        None
    }
}

fn join_limited(items: &[String], limit: usize) -> String {
    let shown = items.iter().take(limit).cloned().collect::<Vec<_>>();
    let suffix = if items.len() > shown.len() {
        format!(", +{}", items.len() - shown.len())
    } else {
        String::new()
    };
    shown.join(", ") + &suffix
}

fn push_unique(steps: &mut Vec<String>, step: impl Into<String>) {
    let step = step.into();
    if !steps.iter().any(|existing| existing == &step) {
        steps.push(step);
    }
}

pub fn run(config_path: &Path) -> DoctorReport {
    let mut builder = DoctorReportBuilder::default();
    let expanded_config_path = config::expand_home(config_path);
    let raw_hints = RawConfigHints::load(&expanded_config_path);

    check_tiffany_ui(&mut builder, config_path);
    check_host_environment(&mut builder);
    check_build_cache(&mut builder);

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
        check_providers(&mut builder, cfg, raw_hints.as_ref().ok());
        check_models(&mut builder, cfg);
        check_roles(&mut builder, cfg);
    } else if let Err(err) = raw_hints {
        builder.warn(format!("raw config hints unavailable: {err:#}"));
    }

    check_local_tools(&mut builder);

    builder.finish()
}

fn check_tiffany_ui(builder: &mut DoctorReportBuilder, config_path: &Path) {
    builder.header("Tiffany UI");
    match tiffany_install::current_orchestrator_exe() {
        Some(path) => builder.ok(format!("orchestrator binary: {}", path.display())),
        None => builder.warn("orchestrator binary path could not be resolved"),
    }

    match tiffany_install::resolve_tiffany_binary() {
        Some(binary) if binary.verified => builder.ok(format!(
            "tiffany-loop binary: {} ({})",
            binary.path.display(),
            binary.source_label()
        )),
        Some(binary) => {
            builder.warn(format!(
                "tiffany-loop binary from {} is not resolvable: {}",
                binary.source_label(),
                binary.path.display()
            ));
            builder.hint("unset TIFFANY_BIN or point it at the installed `tiffany-loop` binary");
        }
        None => {
            builder
                .warn("tiffany-loop binary not found; `orchestrator tui` will use legacy fallback");
            builder
                .hint("install tiffany-loop or use `./scripts/tiffany-dev` from a source checkout");
        }
    }

    if tiffany_install::legacy_tui_forced() {
        builder.warn("ORCHESTRATOR_LEGACY_TUI forces the old terminal chat");
        builder.hint("unset ORCHESTRATOR_LEGACY_TUI to use the tiffany-loop UI");
    } else {
        builder.ok("orchestrator tui will prefer `tiffany-loop`");
    }

    if let Some((home, source)) = tiffany_install::resolved_tiffany_home() {
        builder.ok(format!(
            "tiffany home: {} ({})",
            home.display(),
            source.label()
        ));
        let (sqlite_home, sqlite_source) = tiffany_install::resolved_tiffany_sqlite_home(&home);
        builder.ok(format!(
            "tiffany sqlite home: {} ({})",
            sqlite_home.display(),
            sqlite_source.label()
        ));
        let ui_config = home.join("config.toml");
        if ui_config.exists() {
            builder.ok(format!("tiffany config: {}", ui_config.display()));
        } else {
            builder.hint(format!(
                "tiffany config will be created at {}",
                ui_config.display()
            ));
        }
    } else {
        builder.warn("tiffany home could not be resolved");
        builder.hint("set TIFFANY_HOME to isolate UI config and history");
    }

    builder.ok(format!(
        "orchestrator config: {}",
        config::expand_home(config_path).display()
    ));
    builder.hint(format!(
        "launch bridge: {}",
        tiffany_install::launch_command_preview(config_path)
    ));
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RawConfigHints {
    provider_api_keys: HashMap<String, RawSecret>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RawSecret {
    Missing,
    Empty,
    EnvRefs(Vec<String>),
    Literal,
}

impl RawConfigHints {
    fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let yaml: serde_yaml::Value = serde_yaml::from_str(&raw)?;
        let mut out = Self::default();

        let Some(providers) = yaml
            .get("providers")
            .and_then(serde_yaml::Value::as_mapping)
        else {
            return Ok(out);
        };

        for (key, value) in providers {
            let Some(provider) = key.as_str() else {
                continue;
            };
            let raw_secret = value
                .as_mapping()
                .and_then(|mapping| mapping.get(serde_yaml::Value::String("api_key".into())))
                .map(classify_raw_secret)
                .unwrap_or(RawSecret::Missing);
            out.provider_api_keys
                .insert(provider.to_string(), raw_secret);
        }

        Ok(out)
    }
}

fn classify_raw_secret(value: &serde_yaml::Value) -> RawSecret {
    let Some(text) = value.as_str() else {
        return RawSecret::Literal;
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return RawSecret::Empty;
    }
    let refs = env_refs(trimmed);
    if refs.is_empty() {
        RawSecret::Literal
    } else {
        RawSecret::EnvRefs(refs)
    }
}

fn env_refs(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    if let Some(name) = text.strip_prefix('$') {
        let name = name.trim_start_matches('{').trim_end_matches('}');
        if is_env_name(name) {
            refs.push(name.to_string());
        }
    }

    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        let name = &after[..end];
        if is_env_name(name) && !refs.iter().any(|seen| seen == name) {
            refs.push(name.to_string());
        }
        rest = &after[end + 1..];
    }
    refs
}

fn is_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
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

fn check_providers(
    builder: &mut DoctorReportBuilder,
    cfg: &Config,
    raw_hints: Option<&RawConfigHints>,
) {
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

        if let Some(raw_secret) = raw_hints.and_then(|hints| hints.provider_api_keys.get(name)) {
            match raw_secret {
                RawSecret::EnvRefs(refs) => {
                    check_provider_env_refs(builder, name, refs);
                    continue;
                }
                RawSecret::Literal => {
                    if provider
                        .api_key
                        .as_deref()
                        .is_some_and(|key| !key.trim().is_empty())
                    {
                        builder.ok(format!("{name}: api key present"));
                        builder.warn(format!("{name}: api key is stored literally in config"));
                        builder.hint(format!(
                            "prefer `orchestrator config provider setup {name} --env ENV_NAME` for shared configs"
                        ));
                        continue;
                    }
                }
                RawSecret::Missing | RawSecret::Empty => {}
            }
        }

        match provider.api_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => builder.ok(format!("{name}: api key present")),
            _ => {
                builder.fail(format!("{name}: api key missing"));
                builder.hint(format!(
                    "set it with `/provider`, `orchestrator config provider setup {name}`, or an env var reference"
                ));
            }
        }
    }
}

fn check_provider_env_refs(builder: &mut DoctorReportBuilder, provider: &str, refs: &[String]) {
    let missing = refs
        .iter()
        .filter(|name| std::env::var(name.as_str()).map_or(true, |value| value.trim().is_empty()))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        builder.ok(format!("{provider}: api key env {} set", refs.join(", ")));
    } else {
        builder.fail(format!(
            "{provider}: api key env {} unset",
            missing.join(", ")
        ));
        let exports = missing
            .iter()
            .map(|name| format!("export {name}=..."))
            .collect::<Vec<_>>()
            .join("; ");
        builder.hint(exports);
    }
}

fn check_models(builder: &mut DoctorReportBuilder, cfg: &Config) {
    builder.header("Models");
    if cfg.models.is_empty() {
        builder.fail("no models configured");
        builder.hint("configure one with `/role` or `orchestrator roles register ... --provider ... --model-name ...`");
        return;
    }

    let mut seen = HashSet::new();
    let mut models = cfg.models.iter().collect::<Vec<_>>();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    for model in models {
        if !seen.insert(model.id.as_str()) {
            builder.fail(format!("model `{}` is defined more than once", model.id));
            continue;
        }
        if cfg.providers.contains_key(&model.provider) {
            builder.ok(format!(
                "{}: {} via {}",
                model.id, model.name, model.provider
            ));
        } else {
            builder.fail(format!(
                "{}: provider `{}` is not configured",
                model.id, model.provider
            ));
            builder.hint(format!(
                "configure provider `{}` with `/provider` or `orchestrator config provider setup {}`",
                model.provider, model.provider
            ));
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
            builder.fail("no worker role configured (add a role containing worker or executor)");
            builder.hint(format!("configured roles: {}", available_role_names(cfg)));
            builder.hint(
                "add one with `orchestrator roles register worker-cc --model <model-id> --runtime claude-code --agent-teams`",
            );
            builder.hint(
                "or add a Codex worker with `orchestrator roles register worker-codex --model <model-id> --runtime codex`",
            );
        }
    }

    let mut roles = cfg.roles.iter().collect::<Vec<_>>();
    roles.sort_by(|a, b| a.0.cmp(b.0));
    for (role_name, role) in roles {
        check_role_binding(builder, cfg, role_name, role);
    }

    if cfg
        .roles
        .values()
        .any(|role| runtime::agent_label(Some(&role.runtime)) == "claude")
        && !cfg.behavior.cc_bypass_permissions
    {
        builder.warn("Claude Code workers may pause for manual permission prompts");
        builder.hint("set behavior.cc_bypass_permissions: true for unattended tiffany-loop runs");
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
    let model_cfg = cfg.models.iter().find(|model| model.id == role.model);
    let model_ok = model_cfg.is_some();
    let runtime_ok = cfg.runtime_config(&role.runtime).is_some();
    if model_ok && runtime_ok {
        let model = model_cfg.expect("checked above");
        let provider_status = if cfg.providers.contains_key(&model.provider) {
            format!("provider={}", model.provider)
        } else {
            format!("provider={} missing", model.provider)
        };
        builder.ok(format!(
            "{role_name}: model={} ({}, {}) runtime={} teams={}",
            role.model, model.name, provider_status, role.runtime, role.agent_teams
        ));
        return;
    }
    if !model_ok {
        builder.fail(format!(
            "{role_name}: model `{}` is not defined in models",
            role.model
        ));
        builder.hint(format!(
            "register it with `orchestrator roles register {role_name} --model {} --provider <provider> --model-name <api-model> --runtime {}`",
            role.model, role.runtime
        ));
    }
    if !runtime_ok {
        builder.fail(format!(
            "{role_name}: runtime `{}` is not defined in runtimes",
            role.runtime
        ));
        builder.hint(format!(
            "available runtimes: {}",
            available_runtime_names(cfg)
        ));
    }
}

fn available_runtime_names(cfg: &Config) -> String {
    let mut names = cfg.runtimes.keys().map(String::as_str).collect::<Vec<_>>();
    names.sort();
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

fn available_role_names(cfg: &Config) -> String {
    let mut names = cfg.roles.keys().map(String::as_str).collect::<Vec<_>>();
    names.sort();
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

fn check_host_environment(builder: &mut DoctorReportBuilder) {
    builder.header("Host environment");
    builder.ok(format!(
        "platform: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));

    check_rust_tool(builder, "rustc", "RUSTC", &["--version"], "source builds");
    check_rust_tool(builder, "cargo", "CARGO", &["--version"], "source builds");

    if cfg!(target_os = "macos") {
        check_homebrew(builder);
        check_xcode(builder);
    }
}

const LARGE_TARGET_CACHE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

fn check_build_cache(builder: &mut DoctorReportBuilder) {
    let Some(repo_dir) = find_source_checkout_dir() else {
        return;
    };
    check_build_cache_at(builder, &repo_dir, LARGE_TARGET_CACHE_BYTES);
}

fn find_source_checkout_dir() -> Option<PathBuf> {
    let current = std::env::current_dir().ok();
    if let Some(dir) = current.as_deref().and_then(find_source_checkout_ancestor) {
        return Some(dir);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if is_source_checkout_dir(&manifest_dir) {
        Some(manifest_dir)
    } else {
        None
    }
}

fn find_source_checkout_ancestor(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|path| is_source_checkout_dir(path))
        .map(Path::to_path_buf)
}

fn is_source_checkout_dir(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join("scripts/tiffany-clean-targets").is_file()
}

fn check_build_cache_at(
    builder: &mut DoctorReportBuilder,
    repo_dir: &Path,
    large_threshold_bytes: u64,
) {
    let shared_target = repo_dir.join("target");
    let legacy_target = repo_dir.join("tiffany-ui/codex-rs/target");
    let shared_size = path_size_bytes(&shared_target);
    let legacy_size = path_size_bytes(&legacy_target);
    if shared_size.is_none() && legacy_size.is_none() {
        return;
    }

    builder.header("Build cache");
    if let Some(size) = shared_size {
        let message = format!(
            "shared target: {} ({})",
            format_bytes(size),
            shared_target.display()
        );
        if size >= large_threshold_bytes {
            builder.warn(message);
        } else {
            builder.ok(message);
        }
    } else {
        builder.hint(format!(
            "shared target not found: {}",
            shared_target.display()
        ));
    }

    if let Some(size) = legacy_size {
        builder.warn(format!(
            "legacy fork target: {} ({})",
            format_bytes(size),
            legacy_target.display()
        ));
        builder.hint("legacy fork-local target is rebuildable; run `./scripts/tiffany-clean-targets` to remove it");
    }

    if shared_size.unwrap_or(0) >= large_threshold_bytes || legacy_size.is_some() {
        builder.hint("inspect build cache with `./scripts/tiffany-clean-targets --sizes`");
        builder
            .hint("preview safe cleanup with `./scripts/tiffany-clean-targets --dry-run --trim`");
        builder.hint("trim rebuildable caches with `./scripts/tiffany-clean-targets --trim`");
    }
}

fn path_size_bytes(path: &Path) -> Option<u64> {
    if !path.exists() {
        return None;
    }
    du_size_bytes(path).or_else(|| recursive_size_bytes(path).ok())
}

fn du_size_bytes(path: &Path) -> Option<u64> {
    let output = Command::new("du").arg("-sk").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let kib = stdout.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(kib.saturating_mul(1024))
}

fn recursive_size_bytes(path: &Path) -> std::io::Result<u64> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }

    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        total = total.saturating_add(recursive_size_bytes(&entry.path())?);
    }
    Ok(total)
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.1} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.1} KiB", bytes_f / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn check_rust_tool(
    builder: &mut DoctorReportBuilder,
    binary: &str,
    env_var: &str,
    args: &[&str],
    description: &str,
) {
    let candidates =
        rust_tool_candidates(binary, env_var, std::env::var_os(env_var), home::home_dir());
    let mut failures = Vec::new();

    for candidate in candidates {
        match command_summary(&candidate.command, args) {
            Ok(summary) if summary.success => {
                builder.ok(format!(
                    "{binary}: {} ({description}, {})",
                    summary.first_line,
                    candidate.source_label()
                ));
                return;
            }
            Ok(summary) => failures.push(format!(
                "{}: {}",
                candidate.source_label(),
                summary.short_failure()
            )),
            Err(err) => failures.push(format!("{}: {err}", candidate.source_label())),
        }
    }

    builder.warn(format!("{binary}: not found ({description})"));
    builder.hint(
        "install Rust with rustup, or export PATH=\"$HOME/.cargo/bin:$PATH\" before running tiffany-loop",
    );
    if let Some(last_failure) = failures
        .iter()
        .find(|failure| !failure.contains("No such file") && !failure.contains("not found"))
    {
        builder.hint(format!("{binary} check detail: {last_failure}"));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RustToolCandidate {
    command: String,
    source: String,
}

impl RustToolCandidate {
    fn source_label(&self) -> String {
        match self.source.as_str() {
            "PATH" => "PATH".to_string(),
            source => format!("{source}: {}", self.command),
        }
    }
}

fn rust_tool_candidates(
    binary: &str,
    env_var: &str,
    env_value: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> Vec<RustToolCandidate> {
    let mut candidates = Vec::new();
    if let Some(value) = env_value.filter(|value| !value.is_empty()) {
        candidates.push(RustToolCandidate {
            command: PathBuf::from(value).to_string_lossy().to_string(),
            source: env_var.to_string(),
        });
    }

    candidates.push(RustToolCandidate {
        command: binary.to_string(),
        source: "PATH".to_string(),
    });

    if let Some(home) = home_dir {
        candidates.push(RustToolCandidate {
            command: home
                .join(".cargo")
                .join("bin")
                .join(binary)
                .to_string_lossy()
                .to_string(),
            source: "~/.cargo/bin".to_string(),
        });
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.command.clone()))
        .collect()
}

fn check_homebrew(builder: &mut DoctorReportBuilder) {
    match command_summary("brew", &["--version"]) {
        Ok(summary) if summary.success => builder.ok(format!("homebrew: {}", summary.first_line)),
        Ok(summary) => builder.warn(format!("homebrew: {}", summary.short_failure())),
        Err(_) => {
            builder.warn("homebrew: not found (recommended install path)");
            builder.hint("install Homebrew or use the release archive / cargo install path");
            return;
        }
    }

    match command_summary("brew", &["tap"]) {
        Ok(summary)
            if summary
                .stdout
                .lines()
                .any(|line| line.trim().eq_ignore_ascii_case("macguffinQ/tap")) =>
        {
            builder.ok("homebrew tap: macguffinQ/tap");
        }
        Ok(summary) if summary.success => {
            builder.warn("homebrew tap: macguffinQ/tap not tapped");
            builder.hint("run `brew tap macguffinQ/tap && brew install tiffany-loop`");
        }
        Ok(summary) => builder.warn(format!(
            "homebrew tap check failed: {}",
            summary.short_failure()
        )),
        Err(err) => builder.warn(format!("homebrew tap check failed: {err}")),
    }

    match command_summary("brew", &["list", "--versions", "tiffany-loop"]) {
        Ok(summary) if summary.success && !summary.stdout.trim().is_empty() => {
            let installed = summary.stdout.trim();
            match parse_homebrew_tiffany_version(installed) {
                Some(env!("CARGO_PKG_VERSION")) => {
                    builder.ok(format!("homebrew package: {installed}"));
                }
                Some(version) => {
                    builder.warn(format!(
                        "homebrew package: tiffany-loop {version} (current source/release is {})",
                        env!("CARGO_PKG_VERSION")
                    ));
                    builder.hint("run `brew update && brew upgrade tiffany-loop`");
                    builder.hint("if the local tap has divergent history, run `brew untap macguffinQ/tap && brew tap macguffinQ/tap`");
                }
                None => builder.ok(format!("homebrew package: {installed}")),
            }
        }
        Ok(summary) if summary.success => {
            builder.hint("homebrew package not installed: `brew install tiffany-loop`");
        }
        Ok(summary) => builder.hint(format!(
            "homebrew package status unavailable: {}",
            summary.short_failure()
        )),
        Err(err) => builder.hint(format!("homebrew package status unavailable: {err}")),
    }
}

fn parse_homebrew_tiffany_version(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("tiffany-loop "))
        .and_then(|rest| rest.split_whitespace().next())
}

fn check_xcode(builder: &mut DoctorReportBuilder) {
    match command_summary("xcode-select", &["-p"]) {
        Ok(summary) if summary.success => {
            let developer_dir = summary.stdout.trim();
            builder.ok(format!("xcode developer dir: {developer_dir}"));
            if developer_dir.contains("/Applications/Xcode-beta.app/") {
                builder.hint("using Xcode beta developer dir; if source builds fail, switch back with `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`");
            } else if Path::new("/Applications/Xcode-beta.app/Contents/Developer").exists() {
                builder.hint("Xcode beta is installed; select it with `sudo xcode-select -s /Applications/Xcode-beta.app/Contents/Developer` when you intentionally want the beta toolchain");
            }
        }
        Ok(summary) => {
            builder.warn(format!("xcode-select: {}", summary.short_failure()));
            builder.hint("run `xcode-select --install`, or select Xcode with `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`");
        }
        Err(err) => builder.warn(format!("xcode-select: {err}")),
    }

    match command_summary("xcrun", &["--find", "clang"]) {
        Ok(summary) if summary.success => builder.ok(format!("clang: {}", summary.stdout.trim())),
        Ok(summary) => {
            builder.warn(format!("clang lookup failed: {}", summary.short_failure()));
            builder.hint("install/select Xcode Command Line Tools before building from source");
        }
        Err(err) => builder.warn(format!("clang lookup failed: {err}")),
    }

    match command_summary("xcodebuild", &["-version"]) {
        Ok(summary) if summary.success => builder.ok(format!(
            "xcodebuild: {}",
            summary
                .stdout
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Ok(summary) => {
            builder.warn(format!("xcodebuild: {}", summary.short_failure()));
            builder.hint(
                "if Xcode was just updated, open it once or run `sudo xcodebuild -license accept`",
            );
        }
        Err(err) => builder.warn(format!("xcodebuild: {err}")),
    }
}

fn check_local_tools(builder: &mut DoctorReportBuilder) {
    builder.header("Local tools");
    check_tool(builder, "git", "required for worktree/diff workflows", true);
    check_tool(builder, "claude", "Claude Code worker binary", false);
    check_tool(builder, "codex", "Codex worker binary", false);
    check_tool(builder, "zellij", "optional terminal multiplexer", false);
    check_tool(builder, "tmux", "optional fallback multiplexer", false);

    check_version_command(builder, "claude", &["--version"], "worker version", false);
    check_version_command(builder, "codex", &["--version"], "worker version", false);
}

fn check_tool(builder: &mut DoctorReportBuilder, binary: &str, description: &str, required: bool) {
    match which::which(binary) {
        Ok(path) => builder.ok(format!("{binary}: {} ({description})", path.display())),
        Err(_) if required => builder.fail(format!("{binary}: not found ({description})")),
        Err(_) => builder.warn(format!("{binary}: not found ({description})")),
    }
}

fn check_version_command(
    builder: &mut DoctorReportBuilder,
    binary: &str,
    args: &[&str],
    description: &str,
    required: bool,
) {
    match command_summary(binary, args) {
        Ok(summary) if summary.success => {
            builder.ok(format!("{binary}: {} ({description})", summary.first_line));
        }
        Ok(summary) if required => {
            builder.fail(format!("{binary}: {}", summary.short_failure()));
        }
        Ok(summary) => {
            builder.warn(format!("{binary}: {}", summary.short_failure()));
        }
        Err(_) if required => builder.fail(format!("{binary}: not found ({description})")),
        Err(_) => builder.warn(format!("{binary}: not found ({description})")),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CommandSummary {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
    first_line: String,
}

impl CommandSummary {
    fn short_failure(&self) -> String {
        let detail = self
            .stderr
            .lines()
            .chain(self.stdout.lines())
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .unwrap_or("no output");
        match self.code {
            Some(code) => format!("exited with code {code}: {detail}"),
            None => format!("terminated by signal: {detail}"),
        }
    }
}

fn command_summary(binary: &str, args: &[&str]) -> std::io::Result<CommandSummary> {
    let output = Command::new(binary).args(args).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let first_line = stdout
        .lines()
        .chain(stderr.lines())
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("no output")
        .to_string();

    Ok(CommandSummary {
        success: output.status.success(),
        code: output.status.code(),
        stdout,
        stderr,
        first_line,
    })
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
        assert!(rendered.contains("Issue summary:"));
        assert!(rendered.contains("Provider auth missing for 1 provider(s): openai"));
        assert!(rendered.contains("Next steps:"));
        assert!(rendered.contains("Configure provider auth:"));
        assert!(rendered.contains("1 issue(s) found"));
    }

    #[test]
    fn report_with_no_required_issues_suggests_launching_ui() {
        let report = DoctorReport {
            lines: vec![DoctorLine {
                level: DoctorLevel::Ok,
                message: "config parsed".into(),
            }],
            issue_count: 0,
        };

        let rendered = report.render_text();

        assert!(rendered.contains("Next steps:"));
        assert!(rendered.contains("tiffany-loop"));
        assert!(rendered.contains("all required checks passed"));
    }

    #[test]
    fn report_next_steps_prioritize_config_and_roles() {
        let report = DoctorReport {
            lines: vec![
                DoctorLine {
                    level: DoctorLevel::Fail,
                    message: "config missing: /tmp/config.yaml".into(),
                },
                DoctorLine {
                    level: DoctorLevel::Fail,
                    message: "planner: missing role config".into(),
                },
            ],
            issue_count: 2,
        };

        let rendered = report.render_text();

        assert!(rendered.contains("Run `orchestrator setup`"));
        assert!(rendered.contains("Register roles with `/role`"));
        assert!(rendered.contains("Rerun `orchestrator doctor`"));
    }

    #[test]
    fn tiffany_ui_check_is_informational() {
        let mut builder = DoctorReportBuilder::default();

        check_tiffany_ui(&mut builder, Path::new("~/.orchestrator/config.yaml"));
        let report = builder.finish();
        let rendered = report.render_text();

        assert_eq!(report.issue_count, 0);
        assert!(rendered.contains("Tiffany UI:"));
        assert!(rendered.contains("orchestrator config:"));
        assert!(rendered.contains("launch bridge:"));
    }

    #[test]
    fn parses_homebrew_tiffany_version() {
        assert_eq!(
            parse_homebrew_tiffany_version("tiffany-loop 0.1.6\n"),
            Some("0.1.6")
        );
        assert_eq!(
            parse_homebrew_tiffany_version("other 1.0\ntiffany-loop 0.1.7 0.1.6\n"),
            Some("0.1.7")
        );
        assert_eq!(parse_homebrew_tiffany_version("other 1.0\n"), None);
    }

    #[test]
    fn rust_tool_candidates_include_env_path_and_cargo_home() {
        let candidates = rust_tool_candidates(
            "cargo",
            "CARGO",
            Some(OsString::from("/opt/rust/bin/cargo")),
            Some(PathBuf::from("/home/alice")),
        );

        assert_eq!(
            candidates,
            vec![
                RustToolCandidate {
                    command: "/opt/rust/bin/cargo".into(),
                    source: "CARGO".into(),
                },
                RustToolCandidate {
                    command: "cargo".into(),
                    source: "PATH".into(),
                },
                RustToolCandidate {
                    command: "/home/alice/.cargo/bin/cargo".into(),
                    source: "~/.cargo/bin".into(),
                },
            ]
        );
    }

    #[test]
    fn rust_tool_candidates_deduplicate_env_matching_path() {
        let candidates =
            rust_tool_candidates("cargo", "CARGO", Some(OsString::from("cargo")), None);

        assert_eq!(
            candidates,
            vec![RustToolCandidate {
                command: "cargo".into(),
                source: "CARGO".into(),
            }]
        );
    }

    #[test]
    fn build_cache_check_reports_large_source_targets_without_required_issue() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname='fixture'\n").unwrap();
        std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();
        std::fs::write(tmp.path().join("scripts/tiffany-clean-targets"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("target")).unwrap();
        std::fs::write(tmp.path().join("target/big.bin"), vec![0u8; 8]).unwrap();
        std::fs::create_dir_all(tmp.path().join("tiffany-ui/codex-rs/target")).unwrap();
        std::fs::write(
            tmp.path().join("tiffany-ui/codex-rs/target/legacy.bin"),
            vec![0u8; 4],
        )
        .unwrap();

        let mut builder = DoctorReportBuilder::default();
        check_build_cache_at(&mut builder, tmp.path(), 4);
        let report = builder.finish();
        let rendered = report.render_text();

        assert_eq!(report.issue_count, 0);
        assert!(rendered.contains("Build cache:"));
        assert!(rendered.contains("shared target:"));
        assert!(rendered.contains("legacy fork target:"));
        assert!(rendered.contains("tiffany-clean-targets --sizes"));
        assert!(rendered.contains("tiffany-clean-targets --dry-run --trim"));
        assert!(rendered.contains("tiffany-clean-targets --trim"));
    }

    #[test]
    fn build_cache_check_keeps_small_shared_target_informational() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname='fixture'\n").unwrap();
        std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();
        std::fs::write(tmp.path().join("scripts/tiffany-clean-targets"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("target")).unwrap();
        std::fs::write(tmp.path().join("target/small.bin"), vec![0u8; 4]).unwrap();

        let mut builder = DoctorReportBuilder::default();
        check_build_cache_at(&mut builder, tmp.path(), 1024 * 1024);
        let report = builder.finish();
        let rendered = report.render_text();

        assert_eq!(report.issue_count, 0);
        assert!(rendered.contains("✓ shared target:"));
        assert!(!rendered.contains("legacy fork target"));
        assert!(!rendered.contains("tiffany-clean-targets --trim"));
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
    fn role_check_guides_missing_worker_registration() {
        let mut cfg = Config {
            behavior: BehaviorConfig::default(),
            ..Config::default()
        };
        cfg.roles.insert(
            "planner".into(),
            RoleConfig {
                model: "gpt".into(),
                runtime: "codex".into(),
                agent_teams: false,
            },
        );
        cfg.roles.insert(
            "critic".into(),
            RoleConfig {
                model: "gpt".into(),
                runtime: "codex".into(),
                agent_teams: false,
            },
        );
        cfg.roles.insert(
            "reviewer".into(),
            RoleConfig {
                model: "gpt".into(),
                runtime: "codex".into(),
                agent_teams: false,
            },
        );

        let mut builder = DoctorReportBuilder::default();
        check_roles(&mut builder, &cfg);
        let report = builder.finish();
        let rendered = report.render_text();

        assert!(rendered.contains("no worker role configured"));
        assert!(rendered.contains("configured roles: critic, planner, reviewer"));
        assert!(rendered.contains("orchestrator roles register worker-cc"));
        assert!(rendered.contains("orchestrator roles register worker-codex"));
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
        check_providers(&mut builder, &cfg, None);
        let report = builder.finish();
        let rendered = report.render_text();

        assert!(rendered.contains("local-api: runtime type `direct`"));
        assert!(rendered.contains("openai: api key missing"));
        assert_eq!(report.issue_count, 1);
    }

    #[test]
    fn provider_check_reports_unset_env_refs_without_leaking_keys() {
        let mut providers = HashMap::new();
        providers.insert(
            "minimax".into(),
            ProviderConfig {
                kind: "openai".into(),
                api_key: Some(String::new()),
                base_url: Some("https://api.minimax.io/v1".into()),
            },
        );
        let cfg = Config {
            providers,
            behavior: BehaviorConfig::default(),
            ..Config::default()
        };
        let mut hints = RawConfigHints::default();
        hints.provider_api_keys.insert(
            "minimax".into(),
            RawSecret::EnvRefs(vec!["TIFFANY_TEST_MISSING_KEY".into()]),
        );

        let mut builder = DoctorReportBuilder::default();
        check_providers(&mut builder, &cfg, Some(&hints));
        let report = builder.finish();
        let rendered = report.render_text();

        assert!(rendered.contains("minimax: api key env TIFFANY_TEST_MISSING_KEY unset"));
        assert!(rendered.contains("hint: export TIFFANY_TEST_MISSING_KEY=..."));
        assert!(rendered
            .contains("Provider auth missing for 1 provider(s): minimax=TIFFANY_TEST_MISSING_KEY"));
        assert!(rendered.contains("orchestrator config provider setup <provider> --env <ENV_NAME>"));
        assert_eq!(report.issue_count, 1);
    }

    #[test]
    fn model_check_reports_missing_provider_and_duplicate_ids() {
        let cfg = Config {
            providers: HashMap::new(),
            models: vec![
                ModelConfig {
                    id: "glm51".into(),
                    provider: "openai-compatible".into(),
                    name: "glm-5.1".into(),
                },
                ModelConfig {
                    id: "glm51".into(),
                    provider: "openai-compatible".into(),
                    name: "glm-5.1".into(),
                },
            ],
            behavior: BehaviorConfig::default(),
            ..Config::default()
        };

        let mut builder = DoctorReportBuilder::default();
        check_models(&mut builder, &cfg);
        let report = builder.finish();
        let rendered = report.render_text();

        assert!(rendered.contains("glm51: provider `openai-compatible` is not configured"));
        assert!(rendered.contains("model `glm51` is defined more than once"));
        assert_eq!(report.issue_count, 2);
    }

    #[test]
    fn role_check_accepts_runtime_alias_and_warns_about_claude_permissions() {
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic".into(),
            ProviderConfig {
                kind: "anthropic".into(),
                api_key: Some("set".into()),
                base_url: None,
            },
        );
        let mut runtimes = HashMap::new();
        runtimes.insert(
            "claude-code".into(),
            RuntimeConfig {
                kind: "subprocess".into(),
                binary: Some("claude".into()),
                supports_mcp: true,
                supports_agent_teams: true,
            },
        );
        let mut roles = HashMap::new();
        roles.insert(
            "worker-cc".into(),
            RoleConfig {
                model: "sonnet".into(),
                runtime: "claude".into(),
                agent_teams: true,
            },
        );
        let cfg = Config {
            providers,
            runtimes,
            models: vec![ModelConfig {
                id: "sonnet".into(),
                provider: "anthropic".into(),
                name: "claude-sonnet".into(),
            }],
            roles,
            behavior: BehaviorConfig {
                cc_bypass_permissions: false,
                ..BehaviorConfig::default()
            },
            ..Config::default()
        };

        let mut builder = DoctorReportBuilder::default();
        check_roles(&mut builder, &cfg);
        let report = builder.finish();
        let rendered = report.render_text();

        assert!(rendered.contains("worker-cc: model=sonnet"));
        assert!(rendered.contains("runtime=claude"));
        assert!(rendered.contains("Claude Code workers may pause"));
        assert!(!rendered.contains("worker-cc: runtime `claude` is not defined"));
    }

    #[test]
    fn raw_secret_classification_detects_env_refs() {
        assert_eq!(
            env_refs("${OPENAI_API_KEY}"),
            vec!["OPENAI_API_KEY".to_string()]
        );
        assert_eq!(
            env_refs("$MINIMAX_API_KEY"),
            vec!["MINIMAX_API_KEY".to_string()]
        );
        assert!(env_refs("sk-literal").is_empty());
    }
}
