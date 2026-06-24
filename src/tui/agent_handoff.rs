use crate::config::Config;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentContinueTarget {
    Claude,
    Codex,
}

impl AgentContinueTarget {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn runtime_name(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex",
        }
    }

    fn default_binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

pub(super) struct ExternalAgentCommand {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

impl ExternalAgentCommand {
    pub(super) fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

pub(super) fn parse_agent_continue_target(args: &[&str]) -> Option<AgentContinueTarget> {
    let raw = match args {
        ["open", target, ..] => *target,
        [target, ..] => *target,
        [] => return None,
    };
    match raw {
        "claude" | "claude-code" | "cc" => Some(AgentContinueTarget::Claude),
        "codex" => Some(AgentContinueTarget::Codex),
        _ => None,
    }
}

pub(super) fn agent_continue_command(
    target: AgentContinueTarget,
    config: &Config,
    handoff_path: &Path,
    native_session_id: Option<&str>,
) -> ExternalAgentCommand {
    let program = config
        .runtime_config(target.runtime_name())
        .and_then(|runtime| runtime.binary.clone())
        .unwrap_or_else(|| target.default_binary().to_string());
    let prompt = format_agent_continue_prompt(handoff_path);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match target {
        AgentContinueTarget::Claude => ExternalAgentCommand {
            program,
            args: claude_continue_args(&cwd, native_session_id, prompt),
        },
        AgentContinueTarget::Codex => ExternalAgentCommand {
            program,
            args: vec![
                "--no-alt-screen".into(),
                "-C".into(),
                cwd.display().to_string(),
                prompt,
            ],
        },
    }
}

fn claude_continue_args(
    cwd: &Path,
    native_session_id: Option<&str>,
    prompt: String,
) -> Vec<String> {
    if let Some(session_id) = native_session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
    {
        return vec![
            "--resume".into(),
            session_id.to_string(),
            "--add-dir".into(),
            cwd.display().to_string(),
            prompt,
        ];
    }

    vec![
        "--add-dir".into(),
        cwd.display().to_string(),
        "--name".into(),
        "orchestrator-handoff".into(),
        prompt,
    ]
}

fn format_agent_continue_prompt(handoff_path: &Path) -> String {
    format!(
        "Continue from this orchestrator handoff: {}\nRead the file first, preserve the current goal, inspect referenced logs/files as needed, and avoid repeating completed work.",
        handoff_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_target_parses_open_aliases() {
        assert_eq!(
            parse_agent_continue_target(&["claude"]),
            Some(AgentContinueTarget::Claude)
        );
        assert_eq!(
            parse_agent_continue_target(&["open", "codex"]),
            Some(AgentContinueTarget::Codex)
        );
        assert_eq!(parse_agent_continue_target(&["status"]), None);
    }

    #[test]
    fn agent_continue_command_uses_inline_terminal_modes() {
        let mut cfg = Config::default();
        cfg.runtimes.insert(
            "claude-code".into(),
            crate::config::RuntimeConfig {
                kind: "subprocess".into(),
                binary: Some("claude-custom".into()),
                supports_mcp: true,
                supports_agent_teams: true,
            },
        );
        cfg.runtimes.insert(
            "codex".into(),
            crate::config::RuntimeConfig {
                kind: "subprocess".into(),
                binary: Some("codex-custom".into()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        let handoff = std::path::Path::new("/tmp/orchestrator-handoff.md");

        let claude = agent_continue_command(AgentContinueTarget::Claude, &cfg, handoff, None);
        assert_eq!(claude.program, "claude-custom");
        assert!(claude.args.contains(&"--add-dir".into()));
        assert!(claude.display().contains("/tmp/orchestrator-handoff.md"));

        let codex = agent_continue_command(AgentContinueTarget::Codex, &cfg, handoff, None);
        assert_eq!(codex.program, "codex-custom");
        assert!(codex.args.contains(&"--no-alt-screen".into()));
        assert!(codex.display().contains("/tmp/orchestrator-handoff.md"));
    }

    #[test]
    fn claude_continue_command_resumes_native_session_when_available() {
        let cfg = Config::default();
        let handoff = std::path::Path::new("/tmp/orchestrator-handoff.md");
        let claude =
            agent_continue_command(AgentContinueTarget::Claude, &cfg, handoff, Some("native-1"));

        assert!(claude
            .args
            .windows(2)
            .any(|pair| pair == ["--resume", "native-1"]));
        assert!(!claude.args.iter().any(|arg| arg == "--name"));
        assert!(claude.display().contains("/tmp/orchestrator-handoff.md"));
    }
}
