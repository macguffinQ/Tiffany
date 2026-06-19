use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

mod cli;

#[derive(Parser)]
#[command(
    name = "orchestrator",
    version,
    about = "Multi-agent orchestration platform"
)]
struct Cli {
    /// Path to config file
    #[arg(long, global = true, default_value = "~/.orchestrator/config.yaml")]
    config: PathBuf,

    /// Log level
    #[arg(long, global = true)]
    log_level: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize config in ~/.orchestrator/
    Init,

    /// Guided first-run setup for providers, models, and roles
    Setup,

    /// Run a task through the orchestrator
    Run {
        /// Task prompt
        prompt: String,

        /// Tags for routing
        #[arg(long, value_delimiter = ',')]
        tag: Vec<String>,

        /// Override planner model
        #[arg(long)]
        planner: Option<String>,

        /// Override critic model
        #[arg(long)]
        critic: Option<String>,

        /// Force worker route: claude, codex, auto, or any configured worker role
        #[arg(long)]
        worker: Option<String>,

        /// Override reviewer model
        #[arg(long)]
        reviewer: Option<String>,

        /// A/B dual-run mode
        #[arg(long)]
        ab: bool,

        /// Skip critic
        #[arg(long)]
        no_critic: bool,

        /// Skip reviewer
        #[arg(long)]
        no_reviewer: bool,
    },

    /// Stream orchestrator progress events as JSONL for tiffany-loop TUI
    Events {
        /// Task prompt
        prompt: String,

        /// Tags for routing
        #[arg(long, value_delimiter = ',')]
        tag: Vec<String>,

        /// Override planner model
        #[arg(long)]
        planner: Option<String>,

        /// Override critic model
        #[arg(long)]
        critic: Option<String>,

        /// Force worker route: claude, codex, auto, or any configured worker role
        #[arg(long)]
        worker: Option<String>,

        /// Override reviewer model
        #[arg(long)]
        reviewer: Option<String>,

        /// Skip critic
        #[arg(long)]
        no_critic: bool,

        /// Skip reviewer
        #[arg(long)]
        no_reviewer: bool,
    },

    /// Open terminal chat
    Tui {
        /// Run in background outside zellij
        #[arg(long)]
        detach: bool,
        /// Force open a new zellij tab (default: stay in current pane)
        #[arg(long)]
        new_tab: bool,
        /// Compatibility flag; uses the stable tiffany-loop-style scrollback renderer
        #[arg(long)]
        ratatui: bool,
    },

    /// Run an Agent Client Protocol (ACP) server over stdio
    Acp {
        /// Worker route: auto, claude, codex, or a role name from config
        #[arg(long, default_value = "auto")]
        agent: String,

        /// Skip critic
        #[arg(long)]
        no_critic: bool,

        /// Skip reviewer
        #[arg(long)]
        no_reviewer: bool,
    },

    /// Browse past sessions
    Sessions {
        #[command(subcommand)]
        action: SessionsCmd,
    },

    /// Register and inspect orchestrator roles
    Roles {
        #[command(subcommand)]
        action: RolesCmd,
    },

    /// Show orchestrator status
    Status,

    /// Diagnose config, runtime binaries, API keys, and local state
    Doctor,

    /// Show token usage and budget
    Usage {
        /// Window: today | month | week | all (default: today)
        #[arg(long, default_value = "today")]
        window: String,
    },

    /// Inspect and edit tiffany-loop orchestrator configuration
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show all loaded config (default)
    Show,

    /// Set a role's model: orchestrator config set <role> <model_id>
    Set {
        /// Role name: planner, critic, reviewer, worker-cc, worker-codex
        role: String,
        /// Model id (e.g. opus, sonnet, haiku, gpt4o, gpt4o-mini)
        model: String,
    },

    /// Get a role's current model: orchestrator config get <role>
    Get { role: String },

    /// Switch all roles to Claude models (preset)
    UseClaude {
        /// Override planner model (default: sonnet)
        #[arg(long)]
        planner: Option<String>,
        /// Override critic model (default: opus)
        #[arg(long)]
        critic: Option<String>,
        /// Override worker-cc model (default: sonnet)
        #[arg(long)]
        worker_cc: Option<String>,
        /// Override worker-codex model (no change by default)
        #[arg(long)]
        worker_codex: Option<String>,
        /// Override reviewer model (default: haiku)
        #[arg(long)]
        reviewer: Option<String>,
    },

    /// Switch all roles to OpenAI models (preset)
    UseOpenai {
        #[arg(long, default_value = "gpt4o")]
        planner: String,
        #[arg(long, default_value = "gpt4o")]
        critic: String,
        #[arg(long, default_value = "gpt4o")]
        worker_cc: String,
        #[arg(long, default_value = "gpt4o")]
        worker_codex: String,
        #[arg(long, default_value = "gpt4o-mini")]
        reviewer: String,
    },

    /// Routing preset: expensive models for orchestration, cheap for code-writing.
    /// planner/critic/reviewer → expensive (opus/sonnet)
    /// worker-cc/worker-codex → cheap (configurable, default: whatever is set)
    UseExpensiveOrchestrators {
        /// Model for planner + critic (default: opus)
        #[arg(long, default_value = "opus")]
        orchestrator_model: String,
        /// Model for reviewer (default: sonnet)
        #[arg(long, default_value = "sonnet")]
        reviewer_model: String,
        /// Model for workers (default: whatever's in the model list first)
        #[arg(long)]
        worker_model: Option<String>,
    },

    /// Use a named roleset preset: codex, claude, economy, quality
    UseRoleset {
        /// Preset name: codex, claude, economy, quality
        name: String,
    },

    /// OpenClaw-style provider setup helpers; omit subcommand for the selector
    #[command(alias = "providers")]
    Provider {
        #[command(subcommand)]
        action: Option<ProviderConfigCmd>,
    },

    /// Interactive first-run setup wizard
    Wizard,

    /// Set API key for a provider
    SetKey {
        provider: String,
        /// Provider type, e.g. openai, anthropic, google, ollama
        #[arg(long = "type")]
        kind: Option<String>,
        /// API key value, or "$ENV_VAR" to reference an env var (default: prompt)
        #[arg(long)]
        key: Option<String>,
    },

    /// Set the base URL (endpoint) for a provider
    SetEndpoint {
        provider: String,
        /// e.g. https://api.deepseek.com/v1
        url: String,
        /// Provider type, e.g. openai, anthropic, google, ollama
        #[arg(long = "type")]
        kind: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProviderConfigCmd {
    /// Open an interactive provider setup selector
    #[command(alias = "interactive")]
    Ui {
        /// Preview the write without changing the config file
        #[arg(long)]
        dry_run: bool,

        /// Fail when the selected env var is not present in this shell
        #[arg(long)]
        check_env: bool,
    },

    /// List configured providers with redacted credentials
    List,

    /// List built-in provider setup presets
    Presets,

    /// Delete one configured provider
    Delete {
        /// Provider id to remove from providers
        provider: String,

        /// Preview the deletion without changing the config file
        #[arg(long)]
        dry_run: bool,
    },

    /// Configure one provider from presets or explicit values
    Setup {
        /// Provider id, e.g. openai, anthropic, minimax, deepseek, ollama
        provider: String,

        /// Provider type, e.g. openai, anthropic, google, ollama
        #[arg(long = "type")]
        kind: Option<String>,

        /// Literal API key value. Prefer --env for checked-in configs.
        #[arg(long)]
        key: Option<String>,

        /// Environment variable name to store as ${ENV_VAR}
        #[arg(long)]
        env: Option<String>,

        /// Base URL. Use "none" to avoid writing an endpoint.
        #[arg(long)]
        endpoint: Option<String>,

        /// Preview the write without changing the config file
        #[arg(long)]
        dry_run: bool,

        /// Fail when the selected env var is not present in this shell
        #[arg(long)]
        check_env: bool,
    },
}

#[derive(Subcommand)]
enum RolesCmd {
    /// List registered roles
    List,

    /// Show one role, or all roles when omitted
    Show {
        /// Role name
        role: Option<String>,
    },

    /// Register or update a role binding
    #[command(alias = "add", alias = "set")]
    Register {
        /// Role name, for example planner, critic, worker-cc, executor
        role: String,

        /// Model id to bind to this role
        #[arg(long)]
        model: String,

        /// Runtime id, for example claude-code or codex
        #[arg(long)]
        runtime: String,

        /// Provider id when also registering/updating the model
        #[arg(long)]
        provider: Option<String>,

        /// Provider model name when also registering/updating the model
        #[arg(long = "model-name")]
        model_name: Option<String>,

        /// Enable Claude Code agent teams for this role
        #[arg(long, conflicts_with = "no_agent_teams")]
        agent_teams: bool,

        /// Disable agent teams for this role
        #[arg(long)]
        no_agent_teams: bool,
    },
}

#[derive(Subcommand)]
enum SessionsCmd {
    /// List recent sessions
    List {
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// Show one session's events
    Show {
        /// Session UUID, short prefix, last, or .
        id: String,
        /// Print raw JSONL instead of human-readable event summaries
        #[arg(long)]
        raw: bool,
        /// Only show the last N event lines
        #[arg(long)]
        tail: Option<usize>,
        /// Show parent/child session links instead of event log
        #[arg(long, conflicts_with = "flow")]
        tree: bool,
        /// Show a readable orchestration/worker waterfall
        #[arg(long, conflicts_with_all = ["raw", "tree"])]
        flow: bool,
    },
    /// Grep across all session logs
    Grep { pattern: String },
    /// Import Claude Code sessions into orchestrator session log
    ImportCc {
        /// Only import sessions from this project (default: current dir)
        #[arg(long)]
        project: Option<String>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Init tracing: for terminal chat, ACP, and JSONL events, route to a
    // file so stdout stays clean
    // (otherwise WARN/INFO logs interleave with interactive output
    // and corrupt the display). For other commands, log to stderr.
    let level = cli.log_level.as_deref().unwrap_or("info");
    let use_file_logging = matches!(
        cli.cmd,
        crate::Cmd::Tui { .. } | crate::Cmd::Acp { .. } | crate::Cmd::Events { .. }
    );
    if use_file_logging {
        setup_file_logging(level);
    } else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(format!("orchestrator={},tokio=warn", level))
            .with_target(false)
            .init();
    }

    match cli::run(cli.cmd, &cli.config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{:#}", e);
            if use_file_logging {
                eprintln!("Error: {:#}", e);
            }
            ExitCode::FAILURE
        }
    }
}

/// Set up tracing to write to ~/.orchestrator/tui.log instead of stderr.
/// Keeps interactive terminal output clean.
fn setup_file_logging(level: &str) {
    use std::io::Write;
    let home = match home::home_dir() {
        Some(h) => h,
        None => return,
    };
    let dir = home.join(".orchestrator");
    let _ = std::fs::create_dir_all(&dir);
    let log_path = dir.join("tui.log");

    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut file = std::io::LineWriter::new(file);
    let _ = writeln!(
        file,
        "\n───── orchestrator session started at {:?} ─────",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let _ = file.flush();

    // Clone the inner file handle for the writer closure.
    let inner_file = match file.into_inner() {
        Ok(f) => f,
        Err(_) => return,
    };
    let fallback = inner_file
        .try_clone()
        .unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap());
    tracing_subscriber::fmt()
        .with_writer(move || {
            inner_file
                .try_clone()
                .unwrap_or_else(|_| fallback.try_clone().unwrap())
        })
        .with_env_filter(format!("orchestrator={},tokio=warn", level))
        .with_target(false)
        .with_ansi(false)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn top_level_help_points_to_setup_and_tiffany_config() {
        let help = Cli::command().render_long_help().to_string();

        assert!(help.contains("setup"));
        assert!(help.contains("Guided first-run setup"));
        assert!(help.contains("Inspect and edit tiffany-loop orchestrator configuration"));
        assert!(!help.contains("Show loaded Claude Code configuration"));
    }

    #[test]
    fn setup_command_parses() {
        let cli = Cli::parse_from(["orchestrator", "setup"]);

        assert!(matches!(cli.cmd, Cmd::Setup));
    }
}
