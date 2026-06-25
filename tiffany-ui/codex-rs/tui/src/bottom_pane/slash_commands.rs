//! Shared helpers for filtering and matching built-in and model service-tier slash commands.
//!
//! The same sandbox- and feature-gating rules are used by both the composer
//! and the command popup. Centralizing them here keeps those call sites small
//! and ensures they stay in sync.
use std::str::FromStr;

use codex_utils_fuzzy_match::fuzzy_match;

use crate::slash_command::SlashCommand;
use crate::slash_command::built_in_slash_commands;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServiceTierCommand {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommandItem {
    Builtin(SlashCommand),
    ServiceTier(ServiceTierCommand),
}

impl SlashCommandItem {
    pub(crate) fn command(&self) -> &str {
        match self {
            Self::Builtin(cmd) => cmd.command(),
            Self::ServiceTier(command) => &command.name,
        }
    }

    pub(crate) fn supports_inline_args(&self) -> bool {
        match self {
            Self::Builtin(cmd) => cmd.supports_inline_args(),
            Self::ServiceTier(_) => false,
        }
    }

    pub(crate) fn available_in_side_conversation(&self) -> bool {
        match self {
            Self::Builtin(cmd) => cmd.available_in_side_conversation(),
            Self::ServiceTier(_) => false,
        }
    }

    pub(crate) fn available_during_task(&self) -> bool {
        match self {
            Self::Builtin(cmd) => cmd.available_during_task(),
            Self::ServiceTier(_) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuiltinCommandFlags {
    pub(crate) collaboration_modes_enabled: bool,
    pub(crate) connectors_enabled: bool,
    pub(crate) plugins_command_enabled: bool,
    pub(crate) token_activity_command_enabled: bool,
    pub(crate) service_tier_commands_enabled: bool,
    pub(crate) goal_command_enabled: bool,
    pub(crate) personality_command_enabled: bool,
    pub(crate) allow_elevate_sandbox: bool,
    pub(crate) side_conversation_active: bool,
    pub(crate) tiffany_orchestrator_shell: bool,
}

pub(crate) const TIFFANY_ORCHESTRATOR_COMMANDS: &[SlashCommand] = &[
    SlashCommand::Provider,
    SlashCommand::Role,
    SlashCommand::Roles,
    SlashCommand::Thread,
    SlashCommand::Doctor,
    SlashCommand::Status,
    SlashCommand::Help,
    SlashCommand::Copy,
    SlashCommand::Raw,
    SlashCommand::Diff,
    SlashCommand::Clear,
    SlashCommand::Exit,
];

/// Return the built-ins that should be visible/usable for the current input.
pub(crate) fn builtins_for_input(flags: BuiltinCommandFlags) -> Vec<(&'static str, SlashCommand)> {
    let commands = if flags.tiffany_orchestrator_shell {
        tiffany_orchestrator_commands()
    } else {
        built_in_slash_commands()
    };

    commands
        .into_iter()
        .filter(|(_, cmd)| {
            !flags.tiffany_orchestrator_shell || tiffany_orchestrator_command_visible(*cmd)
        })
        .filter(|(_, cmd)| flags.allow_elevate_sandbox || *cmd != SlashCommand::ElevateSandbox)
        .filter(|(_, cmd)| flags.collaboration_modes_enabled || *cmd != SlashCommand::Plan)
        .filter(|(_, cmd)| flags.connectors_enabled || *cmd != SlashCommand::Apps)
        .filter(|(_, cmd)| flags.plugins_command_enabled || *cmd != SlashCommand::Plugins)
        .filter(|(_, cmd)| flags.token_activity_command_enabled || *cmd != SlashCommand::Usage)
        .filter(|(_, cmd)| flags.goal_command_enabled || *cmd != SlashCommand::Goal)
        .filter(|(_, cmd)| flags.personality_command_enabled || *cmd != SlashCommand::Personality)
        .filter(|(_, cmd)| !flags.side_conversation_active || cmd.available_in_side_conversation())
        .collect()
}

fn tiffany_orchestrator_commands() -> Vec<(&'static str, SlashCommand)> {
    TIFFANY_ORCHESTRATOR_COMMANDS
        .iter()
        .copied()
        .map(|cmd| (cmd.command(), cmd))
        .collect()
}

pub(crate) fn tiffany_orchestrator_command_visible(cmd: SlashCommand) -> bool {
    TIFFANY_ORCHESTRATOR_COMMANDS.contains(&cmd)
}

pub(crate) fn tiffany_orchestrator_unsupported_command_message(name: &str) -> Option<String> {
    let (command, hint) = if let Ok(cmd) = SlashCommand::from_str(name) {
        if tiffany_orchestrator_command_visible(cmd) {
            return None;
        }

        let hint = match cmd {
            SlashCommand::Model => "Use /provider and /role to configure provider/model routing.",
            SlashCommand::Permissions => {
                "Tiffany runs orchestrator workers through registered runtimes; use /doctor to inspect setup."
            }
            SlashCommand::Init => {
                "Tiffany does not create project instruction files from this shell; add project guidance manually when needed."
            }
            SlashCommand::Compact => {
                "Tiffany keeps orchestration memory separately; use normal follow-up prompts or /doctor for diagnostics."
            }
            SlashCommand::Review => {
                "Ask for review in the chat, or register a reviewer role with /role and /roles."
            }
            SlashCommand::Resume => {
                "Tiffany resumes worker sessions through stable roles; use /roles or /doctor to inspect them."
            }
            SlashCommand::Logout => {
                "Tiffany uses provider settings instead of account login; configure providers with /provider."
            }
            SlashCommand::Quit => "Use /exit to leave Tiffany orchestrator mode.",
            SlashCommand::Agent | SlashCommand::MultiAgents => {
                "Use /role and /roles to register or inspect Tiffany worker roles."
            }
            _ => {
                return Some(format!(
                    "'/{}' is not available in Tiffany orchestrator mode. {}",
                    cmd.command(),
                    tiffany_native_command_hint()
                ));
            }
        };
        (cmd.command(), hint)
    } else if let Some(hint) = tiffany_orchestrator_legacy_command_hint(name) {
        (name, hint)
    } else {
        return None;
    };

    Some(format!(
        "'/{command}' is not available in Tiffany orchestrator mode. {hint}"
    ))
}

fn tiffany_native_command_hint() -> String {
    let commands = tiffany_orchestrator_command_list(", ");
    format!("Use {commands}, or plain chat prompts.")
}

pub(crate) fn tiffany_orchestrator_help_text() -> String {
    let mut text = String::from("Tiffany orchestrator commands:");
    for cmd in tiffany_orchestrator_help_commands() {
        text.push('\n');
        text.push_str(&format!(
            "/{:<10} {}",
            cmd.command(),
            tiffany_orchestrator_command_description(cmd)
        ));
    }
    text
}

pub(crate) fn tiffany_orchestrator_status_command_list() -> String {
    tiffany_orchestrator_command_list("  ")
}

fn tiffany_orchestrator_command_list(separator: &str) -> String {
    tiffany_orchestrator_help_commands()
        .map(|cmd| format!("/{}", cmd.command()))
        .collect::<Vec<_>>()
        .join(separator)
}

fn tiffany_orchestrator_help_commands() -> impl Iterator<Item = SlashCommand> {
    TIFFANY_ORCHESTRATOR_COMMANDS
        .iter()
        .copied()
        .filter(|cmd| *cmd != SlashCommand::Quit)
}

pub(crate) fn tiffany_orchestrator_command_description(cmd: SlashCommand) -> &'static str {
    match cmd {
        SlashCommand::Provider => "configure providers and API keys",
        SlashCommand::Role => "register one role with provider, model, and runtime",
        SlashCommand::Roles => "inspect registered roles",
        SlashCommand::Thread => "inspect or clear stable worker sessions",
        SlashCommand::Doctor => "diagnose setup",
        SlashCommand::Status => "show Tiffany orchestration status",
        SlashCommand::Help => "show this command list",
        SlashCommand::Copy => "copy the last assistant answer",
        SlashCommand::Raw => "toggle copy-friendly raw scrollback",
        SlashCommand::Diff => "show current git changes",
        SlashCommand::Clear => "clear the visible UI",
        SlashCommand::Exit => "exit tiffany-loop",
        _ => "not available in Tiffany orchestrator mode",
    }
}

fn tiffany_orchestrator_legacy_command_hint(name: &str) -> Option<&'static str> {
    let hint = match name {
        "workflow" | "flow" | "process" | "trace" => {
            "The native TUI shows the run waterfall inline. Use /status for current state, or ORCHESTRATOR_LEGACY_TUI=1 orchestrator tui for the legacy process commands."
        }
        "queue" => {
            "Queued follow-up prompts are shown in the bottom pane and run automatically after the active task finishes."
        }
        "result" | "final" => {
            "The worker answer is already rendered as selectable chat text. Use /copy to copy the last assistant answer."
        }
        "o" => {
            "Detail folding is handled by the native run view; /o exists only in the legacy terminal chat fallback."
        }
        "context" | "ctx" | "continue" => {
            "Continue by typing the next prompt directly. Stable worker sessions can be inspected with /thread."
        }
        "sessions" | "history" => {
            "Use /thread for active worker sessions, or run orchestrator sessions list/show from the shell for persisted logs."
        }
        "handoff" => {
            "Ask for a handoff in chat, or use orchestrator sessions show/export from the shell for persisted session output."
        }
        "usage" => "Run orchestrator usage from the shell for token and cost summaries.",
        "tests" | "test" => {
            "Ask Tiffany to run the test command in chat, or run the command directly in your shell."
        }
        "graph" => {
            "Use orchestrator sessions show <id|last> --flow from the shell for a readable orchestration waterfall."
        }
        "acp" => "Run orchestrator acp from the shell to start the Agent Client Protocol server.",
        "checkpoint" | "rollback" => {
            "Patch checkpoint and rollback helpers are only available in the legacy terminal chat fallback."
        }
        "retry" => "Re-submit the prompt directly; native retry controls are not wired yet.",
        "cancel" => "Use Ctrl+C to cancel an active native Tiffany run.",
        _ => return None,
    };
    Some(hint)
}

pub(crate) fn commands_for_input(
    flags: BuiltinCommandFlags,
    service_tier_commands: &[ServiceTierCommand],
) -> Vec<SlashCommandItem> {
    let mut commands = Vec::new();
    let tiers_enabled = flags.service_tier_commands_enabled && !flags.tiffany_orchestrator_shell;
    for (_, cmd) in builtins_for_input(flags) {
        commands.push(SlashCommandItem::Builtin(cmd));
        if cmd == SlashCommand::Model && tiers_enabled {
            commands.extend(
                service_tier_commands
                    .iter()
                    .cloned()
                    .map(SlashCommandItem::ServiceTier),
            );
        }
    }
    commands
        .into_iter()
        .filter(|cmd| !flags.side_conversation_active || cmd.available_in_side_conversation())
        .collect()
}

/// Find a single built-in command by a recognized name or alias, after applying feature gating.
///
/// Side-conversation and token-activity gating are intentionally enforced by dispatch rather than
/// command lookup so a typed command can produce a specific unavailable message while the popup
/// still hides it.
pub(crate) fn find_builtin_command(name: &str, flags: BuiltinCommandFlags) -> Option<SlashCommand> {
    let cmd = SlashCommand::from_str(name).ok().or_else(|| {
        let repeated_os = name.strip_prefix('g')?.strip_suffix("al")?;
        (!repeated_os.is_empty() && repeated_os.bytes().all(|byte| byte == b'o'))
            .then_some(SlashCommand::Goal)
    })?;
    builtins_for_input(BuiltinCommandFlags {
        token_activity_command_enabled: true,
        side_conversation_active: false,
        ..flags
    })
    .into_iter()
    .any(|(_, visible_cmd)| visible_cmd == cmd)
    .then_some(cmd)
}

pub(crate) fn find_slash_command(
    name: &str,
    flags: BuiltinCommandFlags,
    service_tier_commands: &[ServiceTierCommand],
) -> Option<SlashCommandItem> {
    if let Some(cmd) = find_builtin_command(name, flags) {
        return Some(SlashCommandItem::Builtin(cmd));
    }

    let tiers_enabled = flags.service_tier_commands_enabled && !flags.tiffany_orchestrator_shell;
    tiers_enabled
        .then(|| {
            service_tier_commands
                .iter()
                .find(|command| command.name == name)
                .cloned()
                .map(SlashCommandItem::ServiceTier)
        })
        .flatten()
}

pub(crate) fn has_slash_command_prefix(
    name: &str,
    flags: BuiltinCommandFlags,
    service_tier_commands: &[ServiceTierCommand],
) -> bool {
    commands_for_input(flags, service_tier_commands)
        .into_iter()
        .any(|command| fuzzy_match(command.command(), name).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::slice::from_ref;

    fn all_enabled_flags() -> BuiltinCommandFlags {
        BuiltinCommandFlags {
            collaboration_modes_enabled: true,
            connectors_enabled: true,
            plugins_command_enabled: true,
            token_activity_command_enabled: true,
            service_tier_commands_enabled: true,
            goal_command_enabled: true,
            personality_command_enabled: true,
            allow_elevate_sandbox: true,
            side_conversation_active: false,
            tiffany_orchestrator_shell: false,
        }
    }

    #[test]
    fn debug_command_still_resolves_for_dispatch() {
        let cmd = find_builtin_command("debug-config", all_enabled_flags());
        assert_eq!(cmd, Some(SlashCommand::DebugConfig));
    }

    #[test]
    fn clear_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clear", all_enabled_flags()),
            Some(SlashCommand::Clear)
        );
    }

    #[test]
    fn goal_command_allows_extra_os_for_dispatch() {
        assert_eq!(
            find_builtin_command("goooooooooooal", all_enabled_flags()),
            Some(SlashCommand::Goal)
        );
    }

    #[test]
    fn stop_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("stop", all_enabled_flags()),
            Some(SlashCommand::Stop)
        );
    }

    #[test]
    fn clean_command_alias_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clean", all_enabled_flags()),
            Some(SlashCommand::Stop)
        );
    }

    #[test]
    fn service_tier_commands_are_hidden_when_disabled() {
        let mut flags = all_enabled_flags();
        flags.service_tier_commands_enabled = false;
        let commands = vec![ServiceTierCommand {
            id: "priority".to_string(),
            name: "fast".to_string(),
            description: "fastest inference".to_string(),
        }];

        assert_eq!(find_slash_command("fast", flags, &commands), None);
    }

    #[test]
    fn all_service_tiers_are_exposed_as_commands_after_model() {
        let commands = vec![
            ServiceTierCommand {
                id: "priority".to_string(),
                name: "fast".to_string(),
                description: "fastest inference".to_string(),
            },
            ServiceTierCommand {
                id: "batch".to_string(),
                name: "slow".to_string(),
                description: "slower inference with lower priority".to_string(),
            },
        ];

        let items = commands_for_input(all_enabled_flags(), &commands);
        let model_idx = items
            .iter()
            .position(|item| matches!(item, SlashCommandItem::Builtin(SlashCommand::Model)))
            .expect("model command should be visible");
        let inserted = items
            .into_iter()
            .skip(model_idx + 1)
            .take(commands.len())
            .collect::<Vec<_>>();
        let expected = commands
            .into_iter()
            .map(SlashCommandItem::ServiceTier)
            .collect::<Vec<_>>();

        assert_eq!(inserted, expected);
    }

    #[test]
    fn goal_command_is_hidden_when_disabled() {
        let mut flags = all_enabled_flags();
        flags.goal_command_enabled = false;
        assert_eq!(find_builtin_command("goal", flags), None);
    }

    #[test]
    fn usage_command_is_hidden_from_input_when_account_token_activity_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.token_activity_command_enabled = false;
        assert_eq!(
            builtins_for_input(flags)
                .into_iter()
                .find(|(_, command)| *command == SlashCommand::Usage),
            None
        );
    }

    #[test]
    fn usage_command_exact_lookup_still_resolves_when_account_token_activity_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.token_activity_command_enabled = false;
        assert_eq!(
            find_builtin_command("usage", flags),
            Some(SlashCommand::Usage)
        );
    }

    #[test]
    fn tiffany_orchestrator_mode_hides_upstream_only_commands() {
        let commands = builtins_for_input(BuiltinCommandFlags {
            tiffany_orchestrator_shell: true,
            ..all_enabled_flags()
        })
        .into_iter()
        .map(|(_, command)| command)
        .collect::<Vec<_>>();

        assert_eq!(
            commands,
            vec![
                SlashCommand::Provider,
                SlashCommand::Role,
                SlashCommand::Roles,
                SlashCommand::Thread,
                SlashCommand::Doctor,
                SlashCommand::Status,
                SlashCommand::Help,
                SlashCommand::Copy,
                SlashCommand::Raw,
                SlashCommand::Diff,
                SlashCommand::Clear,
                SlashCommand::Exit,
            ]
        );
        assert!(!commands.contains(&SlashCommand::Model));
        assert!(!commands.contains(&SlashCommand::Permissions));
        assert!(!commands.contains(&SlashCommand::Init));
        assert!(!commands.contains(&SlashCommand::Compact));
        assert_eq!(
            find_builtin_command(
                "commands",
                BuiltinCommandFlags {
                    tiffany_orchestrator_shell: true,
                    ..all_enabled_flags()
                },
            ),
            Some(SlashCommand::Help)
        );
    }

    #[test]
    fn tiffany_orchestrator_mode_hides_service_tier_commands() {
        let command = ServiceTierCommand {
            id: "priority".to_string(),
            name: "fast".to_string(),
            description: "fastest inference".to_string(),
        };
        let flags = BuiltinCommandFlags {
            tiffany_orchestrator_shell: true,
            service_tier_commands_enabled: true,
            ..all_enabled_flags()
        };

        assert!(
            !commands_for_input(flags, from_ref(&command))
                .contains(&SlashCommandItem::ServiceTier(command.clone()))
        );
        assert_eq!(find_slash_command("fast", flags, from_ref(&command)), None);
    }

    #[test]
    fn tiffany_orchestrator_mode_explains_known_but_hidden_commands() {
        assert!(
            tiffany_orchestrator_unsupported_command_message("model")
                .expect("model should be known but hidden")
                .contains("not available in Tiffany orchestrator mode")
        );
        assert!(
            tiffany_orchestrator_unsupported_command_message("init")
                .expect("init should be known but hidden")
                .contains("project instruction files")
        );
        assert!(
            tiffany_orchestrator_unsupported_command_message("process")
                .expect("legacy terminal chat command should be explained")
                .contains("run waterfall")
        );
        assert!(
            tiffany_orchestrator_unsupported_command_message("result")
                .expect("legacy result command should be explained")
                .contains("/copy")
        );
        assert!(tiffany_orchestrator_unsupported_command_message("raw").is_none());
        assert!(tiffany_orchestrator_unsupported_command_message("diff").is_none());
        assert!(
            tiffany_orchestrator_unsupported_command_message("o")
                .expect("legacy folding command should be explained")
                .contains("legacy terminal chat fallback")
        );
        assert!(tiffany_orchestrator_unsupported_command_message("provider").is_none());
        assert!(tiffany_orchestrator_unsupported_command_message("does-not-exist").is_none());
    }

    #[test]
    fn tiffany_orchestrator_help_matches_visible_commands() {
        let commands = builtins_for_input(BuiltinCommandFlags {
            tiffany_orchestrator_shell: true,
            ..all_enabled_flags()
        })
        .into_iter()
        .map(|(_, command)| command)
        .collect::<Vec<_>>();
        let help = tiffany_orchestrator_help_text();

        for command in commands {
            assert!(
                help.contains(&format!("/{}", command.command())),
                "expected /{} in Tiffany help: {help}",
                command.command()
            );
        }
        assert!(!help.contains("/quit"));
        assert!(help.contains("/raw"));
        assert!(help.contains("/diff"));
        assert!(!help.contains("/model"));
        assert!(!help.contains("/permissions"));
    }

    #[test]
    fn side_conversation_hides_commands_without_side_flag() {
        let commands = builtins_for_input(BuiltinCommandFlags {
            side_conversation_active: true,
            ..all_enabled_flags()
        })
        .into_iter()
        .map(|(_, command)| command)
        .collect::<Vec<_>>();

        assert_eq!(
            commands,
            vec![
                SlashCommand::Ide,
                SlashCommand::Copy,
                SlashCommand::Raw,
                SlashCommand::Diff,
                SlashCommand::Mention,
                SlashCommand::Status,
                SlashCommand::Usage,
            ]
        );
    }

    #[test]
    fn side_conversation_exact_lookup_still_resolves_hidden_commands_for_dispatch_error() {
        assert_eq!(
            find_builtin_command(
                "review",
                BuiltinCommandFlags {
                    side_conversation_active: true,
                    ..all_enabled_flags()
                },
            ),
            Some(SlashCommand::Review)
        );
    }

    #[test]
    fn side_conversation_exact_lookup_still_resolves_service_tier_commands_for_dispatch_error() {
        let command = ServiceTierCommand {
            id: "priority".to_string(),
            name: "fast".to_string(),
            description: "fastest inference".to_string(),
        };
        let flags = BuiltinCommandFlags {
            side_conversation_active: true,
            ..all_enabled_flags()
        };

        assert_eq!(
            find_slash_command("fast", flags, from_ref(&command)),
            Some(SlashCommandItem::ServiceTier(command))
        );
    }
}
