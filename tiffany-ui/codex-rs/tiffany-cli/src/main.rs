use anyhow::Context;
use clap::Args;
use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_config::LoaderOverrides;
use codex_tui::AppExitInfo;
use codex_tui::Cli as TuiCli;
use codex_tui::ExitReason;
use codex_tui::TiffanyOrchestratorLaunch;
use codex_tui::run_main;
use std::env;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use supports_color::Stream;

const TIFFANY_HOME_ENV: &str = "TIFFANY_HOME";
const TIFFANY_SQLITE_HOME_ENV: &str = "TIFFANY_SQLITE_HOME";
const TIFFANY_RUNTIME_ENV: &str = "TIFFANY_LOOP_RUNTIME";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";

/// Tiffany Loop: a lightweight orchestration shell.
#[derive(Debug, Parser)]
#[command(
    author,
    name = "tiffany-loop",
    version,
    bin_name = "tiffany-loop",
    subcommand_negates_reqs = true,
    override_usage = "tiffany-loop [OPTIONS] [PROMPT]\n       tiffany-loop [OPTIONS] orchestrator [ARGS]\n       tiffany-loop <setup|status|doctor|config|roles|sessions|usage|init> [ARGS]"
)]
struct TiffanyCli {
    /// Optional initial prompt to send through the orchestrator.
    #[arg(value_name = "PROMPT", value_hint = clap::ValueHint::Other)]
    prompt: Option<String>,

    #[clap(subcommand)]
    subcommand: Option<Subcommand>,
}

#[derive(Debug, clap::Subcommand)]
enum Subcommand {
    /// Run orchestrator inside the Tiffany Loop TUI.
    #[clap(name = "orchestrator", visible_aliases = ["orch", "team"])]
    Orchestrator(TiffanyOrchestratorCommand),

    /// Guided first-run setup for providers, models, and roles.
    #[command(
        long_about = "Guided first-run setup for providers, models, and roles.\n\nForwards to `orchestrator setup`."
    )]
    Setup(TiffanyPassthroughCommand),

    /// Show installed commands, config roots, roles, providers, and next steps.
    #[command(
        long_about = "Show installed commands, config roots, roles, providers, and next steps.\n\nForwards to `orchestrator status`."
    )]
    Status(TiffanyPassthroughCommand),

    /// Diagnose config, runtime binaries, API keys, roles, and local tools.
    #[command(
        long_about = "Diagnose config, runtime binaries, API keys, roles, and local tools.\n\nForwards to `orchestrator doctor`."
    )]
    Doctor(TiffanyPassthroughCommand),

    /// Inspect and edit orchestrator configuration.
    #[command(
        long_about = "Inspect and edit orchestrator configuration.\n\nForwards to `orchestrator config`."
    )]
    Config(TiffanyPassthroughCommand),

    /// Register and inspect orchestrator roles.
    #[command(
        long_about = "Register and inspect orchestrator roles.\n\nForwards to `orchestrator roles`."
    )]
    Roles(TiffanyPassthroughCommand),

    /// Browse past sessions.
    #[command(long_about = "Browse past sessions.\n\nForwards to `orchestrator sessions`.")]
    Sessions(TiffanyPassthroughCommand),

    /// Show token usage and budget.
    #[command(long_about = "Show token usage and budget.\n\nForwards to `orchestrator usage`.")]
    Usage(TiffanyPassthroughCommand),

    /// Initialize config in ~/.orchestrator/.
    #[command(
        long_about = "Initialize config in ~/.orchestrator/.\n\nForwards to `orchestrator init`."
    )]
    Init(TiffanyPassthroughCommand),
}

#[derive(Debug, Args, Clone, Default)]
struct TiffanyOrchestratorCommand {
    /// Orchestrator executable to invoke.
    #[arg(long = "bin", default_value = "orchestrator")]
    bin: String,

    /// Orchestrator config file. Kept separate from Tiffany Loop UI config.
    #[arg(long = "orchestrator-config", visible_alias = "orch-config")]
    config: Option<String>,

    /// Use the legacy orchestrator CLI bridge instead of Tiffany orchestration mode.
    #[arg(long = "legacy", default_value_t = false)]
    legacy: bool,

    /// Override planner model.
    #[arg(long)]
    planner: Option<String>,

    /// Override critic model.
    #[arg(long)]
    critic: Option<String>,

    /// Force worker route. Defaults to Claude Code (`worker-cc`) in native mode.
    #[arg(long)]
    worker: Option<String>,

    /// Override reviewer model.
    #[arg(long)]
    reviewer: Option<String>,

    /// Skip critic.
    #[arg(long)]
    no_critic: bool,

    /// Skip reviewer.
    #[arg(long)]
    no_reviewer: bool,

    /// Optional initial prompt for native TUI mode, or arguments forwarded with --legacy.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Args, Clone, Default)]
struct TiffanyPassthroughCommand {
    /// Orchestrator executable to invoke.
    #[arg(long = "bin", default_value = "orchestrator")]
    bin: String,

    /// Orchestrator config file. Kept separate from Tiffany Loop UI config.
    #[arg(long = "orchestrator-config", visible_alias = "orch-config")]
    config: Option<String>,

    /// Arguments forwarded to the orchestrator subcommand.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

impl TiffanyOrchestratorCommand {
    fn native_launch(&self) -> TiffanyOrchestratorLaunch {
        let prompt = self.args.join(" ");
        TiffanyOrchestratorLaunch {
            bin: resolve_bin(&self.bin),
            user_prompt: prompt.clone(),
            prompt,
            extra_args: self.event_args(),
            config_path: self.config.clone(),
            context_turn_count: 0,
        }
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

fn main() -> anyhow::Result<()> {
    let args = env::args_os().collect::<Vec<_>>();
    if is_early_clap_request(&args) {
        TiffanyCli::parse_from(args);
        return Ok(());
    }

    install_tiffany_config_home()?;
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let cli = TiffanyCli::parse();
        run_tiffany(cli, arg0_paths).await
    })
}

fn is_early_clap_request(args: &[OsString]) -> bool {
    if !is_primary_tiffany_invocation(args.first()) {
        return false;
    }

    let mut positional_index = 0usize;
    for arg in args.iter().skip(1) {
        let Some(arg) = arg.to_str() else {
            continue;
        };
        if arg == "--" {
            return false;
        }
        if matches!(arg, "-h" | "--help" | "-V" | "--version") {
            return true;
        }
        if positional_index == 0 && arg == "help" {
            return true;
        }
        positional_index += 1;
    }
    false
}

fn is_primary_tiffany_invocation(arg0: Option<&OsString>) -> bool {
    let Some(arg0) = arg0 else {
        return false;
    };
    let Some(file_name) = Path::new(arg0).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        file_name.strip_suffix(".exe").unwrap_or(file_name),
        "tiffany-loop" | "tiffany"
    )
}

async fn run_tiffany(cli: TiffanyCli, arg0_paths: Arg0DispatchPaths) -> anyhow::Result<()> {
    let TiffanyCli { prompt, subcommand } = cli;

    let command = match subcommand {
        Some(Subcommand::Orchestrator(command)) => {
            return run_tiffany_orchestrator(command, arg0_paths).await;
        }
        Some(subcommand) => {
            run_orchestrator_passthrough(subcommand)?;
            return Ok(());
        }
        None => command_from_default_prompt(prompt),
    };

    run_tiffany_orchestrator(command, arg0_paths).await
}

async fn run_tiffany_orchestrator(
    command: TiffanyOrchestratorCommand,
    arg0_paths: Arg0DispatchPaths,
) -> anyhow::Result<()> {
    if command.legacy {
        run_orchestrator_bridge(command)?;
        return Ok(());
    }

    let mut interactive = base_tui_cli();
    interactive.no_alt_screen = true;
    let launch = command.native_launch();
    ensure_orchestrator_launch_ready(&launch.bin)?;
    interactive.tiffany_orchestrator = Some(launch);

    let exit_info = run_main(
        interactive,
        arg0_paths,
        LoaderOverrides::default(),
        /*explicit_remote_endpoint*/ None,
    )
    .await?;
    handle_exit(exit_info)
}

fn run_orchestrator_passthrough(subcommand: Subcommand) -> anyhow::Result<()> {
    let Some((name, command)) = passthrough_command(subcommand) else {
        anyhow::bail!("internal error: unsupported Tiffany passthrough command");
    };
    let bin = resolve_bin(&command.bin);
    ensure_orchestrator_launch_ready(&bin)?;
    let status = Command::new(&bin)
        .args(config_args(command.config.as_deref()))
        .arg(name)
        .args(&command.args)
        .status()
        .with_context(|| format!("failed to run Tiffany passthrough `{bin} {name}`"))?;

    if !status.success() {
        anyhow::bail!("Tiffany passthrough `{name}` exited with status {status}");
    }
    Ok(())
}

fn passthrough_command(
    subcommand: Subcommand,
) -> Option<(&'static str, TiffanyPassthroughCommand)> {
    match subcommand {
        Subcommand::Setup(command) => Some(("setup", command)),
        Subcommand::Status(command) => Some(("status", command)),
        Subcommand::Doctor(command) => Some(("doctor", command)),
        Subcommand::Config(command) => Some(("config", command)),
        Subcommand::Roles(command) => Some(("roles", command)),
        Subcommand::Sessions(command) => Some(("sessions", command)),
        Subcommand::Usage(command) => Some(("usage", command)),
        Subcommand::Init(command) => Some(("init", command)),
        Subcommand::Orchestrator(_) => None,
    }
}

fn command_from_default_prompt(prompt: Option<String>) -> TiffanyOrchestratorCommand {
    TiffanyOrchestratorCommand {
        args: prompt.into_iter().collect(),
        ..TiffanyOrchestratorCommand::default()
    }
}

fn base_tui_cli() -> TuiCli {
    TuiCli::parse_from(["tiffany-loop"])
}

fn run_orchestrator_bridge(cmd: TiffanyOrchestratorCommand) -> anyhow::Result<()> {
    let args = if cmd.args.is_empty() {
        vec!["tui".to_string()]
    } else {
        cmd.args
    };
    let bin = resolve_bin(&cmd.bin);
    ensure_orchestrator_launch_ready(&bin)?;
    let status = Command::new(&bin)
        .args(config_args(cmd.config.as_deref()))
        .args(&args)
        .status()
        .with_context(|| format!("failed to run Tiffany orchestrator bridge `{bin}`"))?;

    if !status.success() {
        anyhow::bail!("Tiffany orchestrator bridge exited with status {status}");
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
    resolve_bin_from(
        bin,
        env::var_os("TIFFANY_ORCHESTRATOR_BIN"),
        env::current_exe().ok(),
    )
}

fn ensure_orchestrator_launch_ready(bin: &str) -> anyhow::Result<()> {
    let resolved = resolve_executable_path(bin);
    if resolved.is_some() {
        return Ok(());
    }

    anyhow::bail!("{}", format_orchestrator_launch_report(bin))
}

fn format_orchestrator_launch_report(bin: &str) -> String {
    format_orchestrator_launch_report_from(
        bin,
        env::current_exe().ok(),
        env::var_os("TIFFANY_ORCHESTRATOR_BIN"),
    )
}

fn format_orchestrator_launch_report_from(
    bin: &str,
    current_exe: Option<PathBuf>,
    env_override: Option<OsString>,
) -> String {
    let requested = display_bin(bin);
    let resolved = resolve_executable_path(bin);
    let adjacent = current_exe
        .as_ref()
        .and_then(|path| path.parent())
        .map(|dir| dir.join(exe_name("orchestrator")));
    let path_match = find_in_path("orchestrator");
    let env_status = env_override
        .as_ref()
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(|value| {
            let path = PathBuf::from(value);
            format!("{} ({})", path.display(), launchable_path_status(&path))
        })
        .unwrap_or_else(|| "unset".to_string());

    let mut out = String::from("tiffany-loop startup check\n");
    out.push_str("status: missing orchestrator runtime\n");
    out.push_str(&format!("requested: {requested}\n"));
    out.push_str(&format!(
        "resolved: {}\n",
        resolved
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found".to_string())
    ));
    out.push_str(&format!("TIFFANY_ORCHESTRATOR_BIN: {env_status}\n"));
    if let Some(current_exe) = current_exe {
        out.push_str(&format!("tiffany-loop binary: {}\n", current_exe.display()));
    }
    if let Some(adjacent) = adjacent {
        out.push_str(&format!(
            "next to tiffany-loop: {} ({})\n",
            adjacent.display(),
            launchable_path_status(&adjacent)
        ));
    }
    out.push_str(&format!(
        "PATH orchestrator: {}\n",
        path_match
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found".to_string())
    ));
    out.push_str("\nTiffany Loop needs the `orchestrator` runtime to execute planner/critic/worker/reviewer flows.\n");
    out.push_str("Fix one of these:\n");
    out.push_str(
        "  - reinstall the package so `orchestrator` is installed next to `tiffany-loop`\n",
    );
    out.push_str("  - run `tiffany-loop orchestrator --bin /path/to/orchestrator`\n");
    out.push_str("  - set `TIFFANY_ORCHESTRATOR_BIN=/path/to/orchestrator`\n");
    out.push_str("  - from source, run `./scripts/tiffany-dev`\n");
    out
}

fn launchable_path_status(path: &Path) -> &'static str {
    if is_launchable_file(path) {
        "ok"
    } else if path.exists() {
        "not executable"
    } else {
        "missing"
    }
}

fn resolve_executable_path(bin: &str) -> Option<PathBuf> {
    let bin = bin.trim();
    if bin.is_empty() {
        return None;
    }
    let path = PathBuf::from(bin);
    if path.components().count() > 1 || path.is_absolute() {
        return is_launchable_file(&path).then_some(path);
    }
    find_in_path(bin)
}

fn display_bin(bin: &str) -> &str {
    let bin = bin.trim();
    if bin.is_empty() { "orchestrator" } else { bin }
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    find_in_path_with(bin, None)
}

fn find_in_path_with(bin: &str, extra_first_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = extra_first_dir {
        if let Some(found) = executable_candidates(dir, bin)
            .into_iter()
            .find(|path| is_launchable_file(path))
        {
            return Some(found);
        }
    }

    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .flat_map(|dir| executable_candidates(&dir, bin))
        .find(|path| is_launchable_file(path))
}

fn executable_candidates(dir: &Path, bin: &str) -> Vec<PathBuf> {
    if cfg!(windows) && Path::new(bin).extension().is_none() {
        let pathext = env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
        pathext
            .split(';')
            .filter(|ext| !ext.trim().is_empty())
            .map(|ext| dir.join(format!("{bin}{ext}")))
            .collect()
    } else {
        vec![dir.join(bin)]
    }
}

#[cfg(unix)]
fn is_launchable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_launchable_file(path: &Path) -> bool {
    path.is_file()
}

fn resolve_bin_from(
    bin: &str,
    configured_bin: Option<OsString>,
    current_exe: Option<PathBuf>,
) -> String {
    let requested = bin.trim();
    let requested = if requested.is_empty() {
        "orchestrator"
    } else {
        requested
    };

    if requested != "orchestrator" {
        return requested.to_string();
    }

    if let Some(configured) = configured_bin
        .filter(|value| !value.to_string_lossy().trim().is_empty())
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

    requested.to_string()
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn handle_exit(exit_info: AppExitInfo) -> anyhow::Result<()> {
    let is_fatal = match &exit_info.exit_reason {
        ExitReason::Fatal(message) => {
            eprintln!("ERROR: {message}");
            true
        }
        ExitReason::UserRequested => false,
    };

    let color_enabled = supports_color::on(Stream::Stdout).is_some();
    for line in format_exit_messages(exit_info, color_enabled) {
        println!("{line}");
    }
    if is_fatal {
        std::io::stdout().flush()?;
        std::process::exit(1);
    }
    Ok(())
}

fn format_exit_messages(exit_info: AppExitInfo, color_enabled: bool) -> Vec<String> {
    let is_fatal = matches!(&exit_info.exit_reason, ExitReason::Fatal(_));
    let AppExitInfo {
        token_usage,
        thread_id,
        resume_hint,
        ..
    } = exit_info;

    let mut lines = Vec::new();
    if !token_usage.is_zero() {
        lines.push(token_usage.to_string());
    }

    if let Some(resume_cmd) = resume_hint {
        let command = if color_enabled {
            format!("\u{1b}[36m{resume_cmd}\u{1b}[39m")
        } else {
            resume_cmd
        };
        lines.push(format!("To continue this session, run {command}"));
    } else if is_fatal && let Some(thread_id) = thread_id {
        lines.push(format!("Session ID: {thread_id}"));
    }

    lines
}

fn install_tiffany_config_home() -> anyhow::Result<()> {
    let home = resolve_tiffany_home(
        env::var_os(TIFFANY_HOME_ENV),
        env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")),
    )?;
    let sqlite_home = resolve_tiffany_sqlite_home(env::var_os(TIFFANY_SQLITE_HOME_ENV), &home);
    std::fs::create_dir_all(&home)
        .with_context(|| format!("failed to create Tiffany home `{}`", home.display()))?;
    std::fs::create_dir_all(&sqlite_home).with_context(|| {
        format!(
            "failed to create Tiffany sqlite home `{}`",
            sqlite_home.display()
        )
    })?;

    // Codex TUI internals still read CODEX_HOME/CODEX_SQLITE_HOME.
    unsafe {
        env::set_var(TIFFANY_HOME_ENV, &home);
        env::set_var(TIFFANY_SQLITE_HOME_ENV, &sqlite_home);
        env::set_var(TIFFANY_RUNTIME_ENV, "1");
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

fn resolve_tiffany_sqlite_home(sqlite_home_env: Option<OsString>, home: &Path) -> PathBuf {
    sqlite_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn default_prompt_becomes_orchestrator_prompt() {
        let command = command_from_default_prompt(Some("hello".to_string()));

        assert_eq!(command.args, vec!["hello"]);
        assert_eq!(command.event_args(), vec!["--worker", "worker-cc"]);
    }

    #[test]
    fn top_level_config_is_orchestrator_passthrough() {
        let cli = TiffanyCli::parse_from([
            "tiffany-loop",
            "config",
            "--orchestrator-config",
            "/tmp/orch.yaml",
            "provider",
            "list",
        ]);
        let Some(Subcommand::Config(command)) = cli.subcommand else {
            panic!("expected config passthrough");
        };

        assert_eq!(command.config.as_deref(), Some("/tmp/orch.yaml"));
        assert_eq!(command.args, vec!["provider", "list"]);
    }

    #[test]
    fn passthrough_command_maps_top_level_names() {
        let Some((name, command)) =
            passthrough_command(Subcommand::Doctor(TiffanyPassthroughCommand {
                args: vec!["--format".to_string(), "json".to_string()],
                ..TiffanyPassthroughCommand::default()
            }))
        else {
            panic!("expected doctor passthrough");
        };

        assert_eq!(name, "doctor");
        assert_eq!(command.args, vec!["--format", "json"]);
    }

    #[test]
    fn help_and_version_skip_arg0_initialization_for_tiffany() {
        assert!(is_early_clap_request(&[
            OsString::from("tiffany-loop"),
            OsString::from("--help")
        ]));
        assert!(is_early_clap_request(&[
            OsString::from("/opt/homebrew/bin/tiffany"),
            OsString::from("orchestrator"),
            OsString::from("-h")
        ]));
        assert!(is_early_clap_request(&[
            OsString::from("tiffany.exe"),
            OsString::from("--version")
        ]));
        assert!(is_early_clap_request(&[
            OsString::from("tiffany-loop.exe"),
            OsString::from("--version")
        ]));
        assert!(is_early_clap_request(&[
            OsString::from("tiffany-loop"),
            OsString::from("help"),
            OsString::from("orchestrator")
        ]));
    }

    #[test]
    fn early_metadata_detection_does_not_intercept_helpers_or_prompt_literals() {
        assert!(!is_early_clap_request(&[
            OsString::from("apply_patch"),
            OsString::from("--help")
        ]));
        assert!(!is_early_clap_request(&[
            OsString::from("tiffany"),
            OsString::from("--"),
            OsString::from("--help")
        ]));
    }

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

        assert_eq!(resolve_tiffany_sqlite_home(None, &home), home);
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
    fn resolve_empty_bin_uses_default_orchestrator() {
        assert_eq!(resolve_bin_from("", None, None), "orchestrator");
        assert_eq!(resolve_bin_from("   ", None, None), "orchestrator");
    }

    #[test]
    fn resolve_orchestrator_ignores_blank_env_override() {
        assert_eq!(
            resolve_bin_from("orchestrator", Some(OsString::from("   ")), None),
            "orchestrator"
        );
    }

    #[test]
    fn resolve_orchestrator_prefers_sibling_binary() -> std::io::Result<()> {
        let temp = std::env::temp_dir().join(format!("tiffany-loop-test-{}", std::process::id()));
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

    #[test]
    fn launch_readiness_rejects_missing_path() {
        let missing = std::env::temp_dir().join(format!(
            "definitely-missing-orchestrator-{}",
            std::process::id()
        ));

        let err = ensure_orchestrator_launch_ready(&missing.to_string_lossy()).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("status: missing orchestrator runtime"));
        assert!(message.contains("TIFFANY_ORCHESTRATOR_BIN"));
        assert!(message.contains("./scripts/tiffany-dev"));
    }

    #[test]
    fn launch_report_shows_runtime_probe_details() -> std::io::Result<()> {
        let temp =
            std::env::temp_dir().join(format!("tiffany-loop-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp)?;
        let tiffany = temp.join(exe_name("tiffany-loop"));
        let adjacent = temp.join(exe_name("orchestrator"));
        let env_bin = temp.join("env-orchestrator");
        std::fs::write(&tiffany, "")?;
        std::fs::write(&adjacent, "")?;
        std::fs::write(&env_bin, "")?;

        let report = format_orchestrator_launch_report_from(
            &temp.join("missing-orchestrator").to_string_lossy(),
            Some(tiffany),
            Some(env_bin.clone().into_os_string()),
        );

        assert!(report.contains("tiffany-loop startup check"));
        assert!(report.contains("status: missing orchestrator runtime"));
        assert!(report.contains("requested:"));
        assert!(report.contains("resolved: not found"));
        assert!(report.contains("TIFFANY_ORCHESTRATOR_BIN:"));
        assert!(report.contains(&env_bin.display().to_string()));
        assert!(report.contains("not executable"));
        assert!(report.contains("next to tiffany-loop:"));
        assert!(report.contains(&adjacent.display().to_string()));
        assert!(report.contains("PATH orchestrator:"));
        assert!(report.contains("reinstall the package"));

        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[test]
    fn launch_readiness_accepts_existing_path() -> std::io::Result<()> {
        let temp =
            std::env::temp_dir().join(format!("tiffany-loop-ready-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp)?;
        let orchestrator = temp.join(exe_name("orchestrator"));
        std::fs::write(&orchestrator, "")?;
        make_executable(&orchestrator)?;

        assert!(ensure_orchestrator_launch_ready(&orchestrator.to_string_lossy()).is_ok());

        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[test]
    fn path_lookup_accepts_launchable_binary() -> std::io::Result<()> {
        let temp =
            std::env::temp_dir().join(format!("tiffany-loop-path-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp)?;
        let orchestrator = temp.join(exe_name("orchestrator"));
        std::fs::write(&orchestrator, "")?;
        make_executable(&orchestrator)?;

        assert_eq!(
            find_in_path_with("orchestrator", Some(temp.as_path())),
            Some(orchestrator)
        );

        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o700);
        std::fs::set_permissions(path, perms)
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}
