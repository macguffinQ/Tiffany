use anyhow::Context;
use clap::Args;
use codex_tui::TiffanyOrchestratorLaunch;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const TIFFANY_HOME_ENV: &str = "TIFFANY_HOME";
const TIFFANY_SQLITE_HOME_ENV: &str = "TIFFANY_SQLITE_HOME";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";

#[derive(Debug, Args, Clone)]
pub(crate) struct TiffanyOrchestratorCommand {
    /// Orchestrator executable to invoke.
    #[arg(long = "bin", default_value = "orchestrator")]
    pub(crate) bin: String,

    /// Orchestrator config file. Kept separate from tiffany-loop UI config.
    #[arg(long = "orchestrator-config", visible_alias = "orch-config")]
    pub(crate) config: Option<String>,

    /// Use the legacy orchestrator CLI bridge instead of tiffany-loop orchestration mode.
    #[arg(long = "legacy", default_value_t = false)]
    pub(crate) legacy: bool,

    /// Override planner model.
    #[arg(long)]
    pub(crate) planner: Option<String>,

    /// Override critic model.
    #[arg(long)]
    pub(crate) critic: Option<String>,

    /// Force worker route. Defaults to Claude Code (`worker-cc`) in native orchestrator mode.
    #[arg(long)]
    pub(crate) worker: Option<String>,

    /// Claude Code subagent name passed through to Claude workers.
    #[arg(long)]
    pub(crate) agent: Option<String>,

    /// Override reviewer model.
    #[arg(long)]
    pub(crate) reviewer: Option<String>,

    /// Skip critic.
    #[arg(long)]
    pub(crate) no_critic: bool,

    /// Skip reviewer.
    #[arg(long)]
    pub(crate) no_reviewer: bool,

    /// Optional initial prompt for native tiffany-loop TUI mode, or arguments forwarded with --legacy.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) args: Vec<String>,
}

impl TiffanyOrchestratorCommand {
    pub(crate) fn native_launch(&self) -> anyhow::Result<TiffanyOrchestratorLaunch> {
        Ok(TiffanyOrchestratorLaunch {
            bin: resolve_bin(&self.bin),
            user_prompt: self.args.join(" "),
            prompt: self.args.join(" "),
            extra_args: self.event_args(),
            config_path: self.config.clone(),
            context_turn_count: 0,
        })
    }

    fn event_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(planner) = &self.planner {
            args.push("--planner".to_string());
            args.push(planner.clone());
        }
        if let Some(critic) = &self.critic {
            args.push("--critic".to_string());
            args.push(critic.clone());
        }
        args.push("--worker".to_string());
        args.push(
            self.worker
                .clone()
                .unwrap_or_else(|| "worker-cc".to_string()),
        );
        if let Some(agent) = self
            .agent
            .as_deref()
            .map(str::trim)
            .filter(|agent| !agent.is_empty())
        {
            args.push("--agent".to_string());
            args.push(agent.to_string());
        }
        if let Some(reviewer) = &self.reviewer {
            args.push("--reviewer".to_string());
            args.push(reviewer.clone());
        }
        if self.no_critic {
            args.push("--no-critic".to_string());
        }
        if self.no_reviewer {
            args.push("--no-reviewer".to_string());
        }
        args
    }
}

pub(crate) fn run_orchestrator_bridge(cmd: TiffanyOrchestratorCommand) -> anyhow::Result<()> {
    let args = if cmd.args.is_empty() {
        vec!["tui".to_string()]
    } else {
        cmd.args
    };
    let bin = resolve_bin(&cmd.bin);
    let status = Command::new(&bin)
        .args(config_args(cmd.config.as_deref()))
        .args(&args)
        .status()
        .with_context(|| format!("failed to run tiffany-loop orchestrator bridge `{bin}`"))?;

    if !status.success() {
        anyhow::bail!("tiffany-loop orchestrator bridge exited with status {status}");
    }
    Ok(())
}

fn config_args(config: Option<&str>) -> Vec<String> {
    match config.filter(|value| !value.trim().is_empty()) {
        Some(config) => vec!["--config".to_string(), config.to_string()],
        None => Vec::new(),
    }
}

fn resolve_bin(bin: &str) -> String {
    resolve_bin_from(bin, env::var_os("TIFFANY_ORCHESTRATOR_BIN"), env::current_exe().ok())
}

fn resolve_bin_from(
    bin: &str,
    configured_bin: Option<OsString>,
    current_exe: Option<PathBuf>,
) -> String {
    if bin != "orchestrator" {
        return bin.to_string();
    }

    if let Some(configured) = configured_bin
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return configured.to_string_lossy().into_owned();
    }

    if let Some(current_exe) = current_exe
        && let Some(parent) = current_exe.parent()
    {
        let adjacent = parent.join(exe_name("orchestrator"));
        if adjacent.is_file() {
            return adjacent.to_string_lossy().into_owned();
        }
    }

    bin.to_string()
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub(crate) fn install_tiffany_config_home() -> anyhow::Result<()> {
    let home = resolve_tiffany_home(
        env::var_os(TIFFANY_HOME_ENV),
        env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")),
    )?;
    let sqlite_home = resolve_tiffany_sqlite_home(env::var_os(TIFFANY_SQLITE_HOME_ENV), &home);
    std::fs::create_dir_all(&home)
        .with_context(|| format!("failed to create tiffany-loop home `{}`", home.display()))?;
    std::fs::create_dir_all(&sqlite_home).with_context(|| {
        format!(
            "failed to create tiffany-loop sqlite home `{}`",
            sqlite_home.display()
        )
    })?;

    // Codex internals still read CODEX_HOME. Set it before arg0 creates
    // aliases, loads dotenv, or starts Tokio threads.
    unsafe {
        env::set_var(TIFFANY_HOME_ENV, &home);
        env::set_var(TIFFANY_SQLITE_HOME_ENV, &sqlite_home);
        env::set_var(CODEX_HOME_ENV, &home);
        env::set_var(CODEX_SQLITE_HOME_ENV, &sqlite_home);
    }
    Ok(())
}

fn resolve_tiffany_home(
    tiffany_home_env: Option<OsString>,
    user_home_env: Option<OsString>,
) -> anyhow::Result<PathBuf> {
    if let Some(value) = tiffany_home_env.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }

    let Some(home) = user_home_env.filter(|value| !value.is_empty()) else {
        anyhow::bail!("could not find home directory; set TIFFANY_HOME");
    };
    Ok(PathBuf::from(home).join(".tiffany"))
}

fn resolve_tiffany_sqlite_home(sqlite_home_env: Option<OsString>, home: &PathBuf) -> PathBuf {
    sqlite_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone())
}

#[cfg(test)]
mod tests {
    use super::{TiffanyOrchestratorCommand, exe_name, resolve_bin_from, resolve_tiffany_home};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn tiffany_home_env_wins_over_default_home() {
        let resolved = resolve_tiffany_home(
            Some(OsString::from("/tmp/custom-tiffany")),
            Some(OsString::from("/home/alice")),
        )
        .expect("resolve tiffany home");

        assert_eq!(resolved, PathBuf::from("/tmp/custom-tiffany"));
    }

    #[test]
    fn default_tiffany_home_is_not_dot_codex() {
        let resolved = resolve_tiffany_home(None, Some(OsString::from("/home/alice")))
            .expect("resolve default tiffany home");

        assert_eq!(resolved, PathBuf::from("/home/alice/.tiffany"));
    }

    #[test]
    fn sqlite_home_defaults_to_tiffany_home() {
        let home = PathBuf::from("/home/alice/.tiffany");

        assert_eq!(super::resolve_tiffany_sqlite_home(None, &home), home);
    }

    #[test]
    fn orchestrator_event_args_forward_worker_and_claude_agent() {
        let command = TiffanyOrchestratorCommand {
            bin: "orchestrator".into(),
            config: None,
            legacy: false,
            planner: None,
            critic: None,
            worker: Some("worker-cc".into()),
            agent: Some("reviewer".into()),
            reviewer: None,
            no_critic: false,
            no_reviewer: false,
            args: vec![],
        };

        assert_eq!(
            command.event_args(),
            vec![
                "--worker".to_string(),
                "worker-cc".to_string(),
                "--agent".to_string(),
                "reviewer".to_string()
            ]
        );
    }

    #[test]
    fn resolve_orchestrator_prefers_env_override() {
        assert_eq!(
            resolve_bin_from(
                "orchestrator",
                Some(OsString::from("/tmp/custom-orchestrator")),
                None
            ),
            "/tmp/custom-orchestrator"
        );
    }

    #[test]
    fn resolve_orchestrator_prefers_sibling_binary() -> std::io::Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "tiffany-cli-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp)?;
        let tiffany = temp.join(exe_name("tiffany-loop"));
        let orchestrator = temp.join(exe_name("orchestrator"));
        std::fs::write(&tiffany, "")?;
        std::fs::write(&orchestrator, "")?;

        assert_eq!(
            resolve_bin_from("orchestrator", None, Some(tiffany)),
            orchestrator.to_string_lossy()
        );

        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[test]
    fn resolve_non_default_bin_keeps_user_value() {
        assert_eq!(resolve_bin_from("/tmp/orch", None, None), "/tmp/orch");
    }
}
