//! Slash-command dispatch and local-recall handoff for `ChatWidget`.
//!
//! `ChatComposer` parses slash input and stages recognized command text for local
//! Up-arrow recall before returning an input result. This module owns the app-level
//! dispatch step and records the staged entry once the command has been handled, so
//! slash-command recall follows the same submitted-input rule as ordinary text.

use super::*;
use crate::app_event::ThreadGoalSetMode;
use crate::bottom_pane::ProviderSetupDraft;
use crate::bottom_pane::ProviderSetupView;
use crate::bottom_pane::RoleProfileSetupDraft;
use crate::bottom_pane::RoleProfileSetupView;
use crate::bottom_pane::RoleSetupCatalog;
use crate::bottom_pane::RoleSetupChoice;
use crate::bottom_pane::RoleSetupDraft;
use crate::bottom_pane::RoleSetupModelChoice;
use crate::bottom_pane::RoleSetupView;
use crate::bottom_pane::prompt_args::parse_slash_name;
use crate::bottom_pane::role_profile_setup_draft_args;
use crate::bottom_pane::slash_commands::BuiltinCommandFlags;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::bottom_pane::slash_commands::SlashCommandItem;
use crate::bottom_pane::slash_commands::find_slash_command;
use crate::bottom_pane::slash_commands::tiffany_orchestrator_command_visible;
use crate::bottom_pane::slash_commands::tiffany_orchestrator_help_text;
use crate::bottom_pane::slash_commands::tiffany_orchestrator_status_command_list;
use crate::bottom_pane::slash_commands::tiffany_orchestrator_unsupported_command_message;
use crate::goal_display::GOAL_USAGE;
use crate::goal_files::GoalDraft;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlashCommandDispatchSource {
    Live,
    Queued,
}

struct PreparedSlashCommandArgs {
    args: String,
    text_elements: Vec<TextElement>,
    pending_pastes: Vec<(String, String)>,
    local_images: Vec<LocalImageAttachment>,
    remote_image_urls: Vec<String>,
    mention_bindings: Vec<MentionBinding>,
    source: SlashCommandDispatchSource,
}

const SIDE_STARTING_CONTEXT_LABEL: &str = "Side starting...";
const SIDE_SLASH_COMMAND_UNAVAILABLE_HINT: &str =
    "Press Ctrl+C to return to the main thread first.";
const GOAL_USAGE_HINT: &str = "Example: /goal improve benchmark coverage";
const RAW_USAGE: &str = "Usage: /raw [on|off]";
const USAGE_CHATGPT_LOGIN_REQUIRED: &str = "Sign in with ChatGPT to use /usage.";
const PROVIDER_USAGE: &str = "Usage: /provider [setup|list|delete <provider>|key <provider> <key-or-$ENV>|endpoint <provider> <url>]";
const ROLE_USAGE: &str = "Usage: /role [<role>|options|register <role> --provider <provider> --model-name <api-model> --runtime <runtime>|delete <role>]";
const ROLES_USAGE: &str = "Usage: /roles [list|show <role>|register <role> --provider <provider> --model-name <api-model> --runtime <runtime>|delete <role>]";
const THREAD_USAGE: &str = "Usage: /thread [list|show <role>|clear <role>|export <role> [--format markdown|html|--out <path>|--clipboard]]";
const CONTINUE_USAGE: &str = "Usage: /continue [open] [<role>|claude|codex|gemini]";
const DOCTOR_USAGE: &str = "Usage: /doctor [run]";
const QUEUE_USAGE: &str = "Usage: /queue [show|clear|pause|resume|run]";
const PROCESS_USAGE: &str =
    "Usage: /process [summary|full|compact|approvals|questions|tool|tool-call|diff|stderr]";

fn tiffany_approvals_history_args(args: &str) -> String {
    match args.split_whitespace().collect::<Vec<_>>().as_slice() {
        [] | ["compact" | "story" | "summary"] | ["full" | "all"] => "approvals".to_string(),
        ["--turns" | "-n", value] => format!("approvals --turns {value}"),
        _ => "approvals".to_string(),
    }
}

pub(super) fn tiffany_process_history_args(args: &str) -> String {
    match args.split_whitespace().collect::<Vec<_>>().as_slice() {
        [] | ["summary"] | ["compact" | "folded"] => "compact".to_string(),
        ["full" | "all" | "expanded"] => "full".to_string(),
        ["approval" | "approvals"] => "approvals".to_string(),
        ["question" | "questions" | "ask"] => "kind question".to_string(),
        ["answer" | "answers" | "assistant"] => "kind answer".to_string(),
        ["tool" | "tools" | "tool-result" | "tool_result"] => "kind tool_result".to_string(),
        ["call" | "calls" | "tool-call" | "tool-calls" | "tool_use" | "tool-use"] => {
            "kind tool_use".to_string()
        }
        ["diff" | "patch"] => "kind diff".to_string(),
        ["stderr" | "error" | "errors"] => "kind stderr".to_string(),
        ["kind", rest @ ..] if !rest.is_empty() => format!("kind {}", rest.join(" ")),
        [
            "role" | "thread" | "native" | "session" | "search",
            rest @ ..,
        ] if !rest.is_empty() => {
            format!(
                "{} {}",
                args.split_whitespace().next().unwrap_or_default(),
                rest.join(" ")
            )
        }
        [count] if count.parse::<usize>().is_ok() => (*count).to_string(),
        _ => "full".to_string(),
    }
}

fn truncate_tiffany_queue_preview(text: &str) -> String {
    const MAX: usize = 160;
    let trimmed = text.trim();
    let mut out = trimmed.chars().take(MAX).collect::<String>();
    if trimmed.chars().count() > MAX {
        out.push('…');
    }
    out
}

fn push_tiffany_queue_preview_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    detail: &'static str,
    items: &[String],
) {
    const LIMIT: usize = 8;
    if items.is_empty() {
        return;
    }

    lines.push(
        vec![
            "  ".into(),
            title.fg(crate::tiffany_orchestrator::TIFFANY_BLUE).bold(),
            " ".into(),
            detail.dim(),
        ]
        .into(),
    );

    for (idx, item) in items.iter().take(LIMIT).enumerate() {
        lines.push(
            vec![
                format!("{:>2}. ", idx + 1).dim(),
                truncate_tiffany_queue_preview(item).into(),
            ]
            .into(),
        );
    }

    let hidden = items.len().saturating_sub(LIMIT);
    if hidden > 0 {
        lines.push(
            vec![
                "... ".dim(),
                format!("{hidden} more {title} item(s)").into(),
            ]
            .into(),
        );
    }
}

fn tiffany_queue_next_action(
    queued: usize,
    rejected: usize,
    pending: usize,
    paused: bool,
    running: bool,
) -> &'static str {
    if queued + rejected + pending == 0 {
        return "empty";
    }
    if paused {
        return "paused; use /queue resume or /queue run";
    }
    if running {
        return "armed; runs after current orchestration finishes";
    }
    if rejected > 0 {
        return "run retry-first prompt before queued prompts";
    }
    if queued > 1 {
        return "run queued prompts together as one batch";
    }
    if queued == 1 {
        return "run the queued prompt";
    }
    "pending item already submitted"
}

impl ChatWidget {
    /// Dispatch a bare slash command and record its staged local-history entry.
    ///
    /// The composer stages history before returning `InputResult::Command`; this wrapper commits
    /// that staged entry after dispatch so slash-command recall follows the same "submitted input"
    /// rule as normal text.
    pub(super) fn handle_slash_command_dispatch(&mut self, cmd: SlashCommand) {
        if !self.ensure_tiffany_orchestrator_command_visible(cmd) {
            self.bottom_pane.record_pending_slash_command_history();
            return;
        }
        self.dispatch_command(cmd);
        if cmd == SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
        self.bottom_pane.record_pending_slash_command_history();
    }

    pub(super) fn handle_service_tier_command_dispatch(&mut self, command: ServiceTierCommand) {
        if self.active_side_conversation {
            self.add_error_message(format!(
                "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
                command.name
            ));
            self.bottom_pane.drain_pending_submission_state();
            self.bottom_pane.record_pending_slash_command_history();
            return;
        }
        self.toggle_service_tier_from_ui(command);
        self.bottom_pane.record_pending_slash_command_history();
    }

    /// Dispatch an inline slash command and record its staged local-history entry.
    ///
    /// Inline command arguments may later be prepared through the normal submission pipeline, but
    /// local command recall still tracks the original command invocation. Treating this wrapper as
    /// the only input-result entry point avoids double-recording commands with inline args.
    pub(super) fn handle_slash_command_with_args_dispatch(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        if !self.ensure_tiffany_orchestrator_command_visible(cmd) {
            self.bottom_pane.record_pending_slash_command_history();
            return;
        }
        self.dispatch_command_with_args(cmd, args, text_elements);
        self.bottom_pane.record_pending_slash_command_history();
    }

    fn ensure_tiffany_orchestrator_command_visible(&mut self, cmd: SlashCommand) -> bool {
        if !self.tiffany_orchestrator_shell || tiffany_orchestrator_command_visible(cmd) {
            return true;
        }

        let message = tiffany_orchestrator_unsupported_command_message(cmd.command())
            .unwrap_or_else(|| {
                format!(
                    "'/{}' is not available in Tiffany orchestrator mode.",
                    cmd.command()
                )
            });
        self.add_info_message(message, /*hint*/ None);
        self.bottom_pane.drain_pending_submission_state();
        self.request_redraw();
        false
    }

    fn apply_plan_slash_command(&mut self) -> bool {
        if !self.collaboration_modes_enabled() {
            self.add_info_message(
                "Collaboration modes are disabled.".to_string(),
                Some("Enable collaboration modes to use /plan.".to_string()),
            );
            return false;
        }
        if let Some(mask) = collaboration_modes::plan_mask(self.model_catalog.as_ref()) {
            self.set_collaboration_mask_from_user_action(mask);
            true
        } else {
            self.add_info_message(
                "Plan mode unavailable right now.".to_string(),
                /*hint*/ None,
            );
            false
        }
    }

    fn request_side_conversation(
        &mut self,
        parent_thread_id: ThreadId,
        user_message: Option<UserMessage>,
    ) {
        self.set_side_conversation_context_label(Some(SIDE_STARTING_CONTEXT_LABEL.to_string()));
        self.request_redraw();
        self.app_event_tx.send(AppEvent::StartSide {
            parent_thread_id,
            user_message,
        });
    }

    fn request_empty_side_conversation(&mut self, cmd: SlashCommand) {
        let Some(parent_thread_id) = self.thread_id else {
            let command = cmd.command();
            self.add_error_message(format!(
                "'/{command}' is unavailable before the session starts."
            ));
            return;
        };

        self.request_side_conversation(parent_thread_id, /*user_message*/ None);
    }

    fn emit_raw_output_mode_changed(&self, enabled: bool) {
        self.app_event_tx
            .send(AppEvent::RawOutputModeChanged { enabled });
    }

    fn dispatch_tiffany_roles_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(
                format!(
                    "'/roles' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{ROLES_USAGE}"
                ),
            );
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorRolesCommand { args: args.into() });
    }

    fn dispatch_tiffany_role_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/role' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{ROLE_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorRolesCommand { args: args.into() });
    }

    fn dispatch_tiffany_provider_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/provider' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{PROVIDER_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorProviderCommand { args: args.into() });
    }

    fn dispatch_tiffany_doctor_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/doctor' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{DOCTOR_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorDoctorCommand { args: args.into() });
    }

    fn dispatch_tiffany_thread_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/thread' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{THREAD_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorThreadCommand { args: args.into() });
    }

    fn dispatch_tiffany_jobs_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(
                "'/jobs' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`."
                    .to_string(),
            );
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorJobsCommand { args: args.into() });
    }

    fn dispatch_tiffany_process_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/process' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{PROCESS_USAGE}"
            ));
            return;
        }
        let args = tiffany_process_history_args(&args.into());
        self.dispatch_tiffany_history_command(args);
    }

    fn dispatch_tiffany_continue_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/continue' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{CONTINUE_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorContinueCommand { args: args.into() });
    }

    fn dispatch_tiffany_history_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(
                "'/history' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`."
                    .to_string(),
            );
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorHistoryCommand { args: args.into() });
    }

    fn dispatch_tiffany_approvals_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(
                "'/approvals' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`."
                    .to_string(),
            );
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorHistoryCommand {
                args: tiffany_approvals_history_args(&args.into()),
            });
    }

    fn open_tiffany_provider_setup_prompt(&mut self, provider: Option<&str>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/provider' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{PROVIDER_USAGE}"
            ));
            return;
        }

        let provider = provider.unwrap_or("custom");
        let draft =
            provider_setup_initial_draft(provider, self.tiffany_orchestrator_config.as_ref());
        let tx = self.app_event_tx.clone();
        let view = ProviderSetupView::new(
            draft,
            Box::new(
                move |draft: ProviderSetupDraft| match provider_setup_draft_args(&draft) {
                    Ok(args) => {
                        tx.send(AppEvent::TiffanyOrchestratorProviderCommand { args });
                        Ok(())
                    }
                    Err(err) => Err(err),
                },
            ),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    fn open_tiffany_role_setup_prompt(&mut self, role: Option<&str>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/role' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{ROLE_USAGE}"
            ));
            return;
        }

        let config = self.tiffany_orchestrator_config.as_ref();
        let draft = role_setup_initial_draft(role, config);
        let catalog = role_setup_catalog(config);
        let tx = self.app_event_tx.clone();
        let view = RoleSetupView::with_catalog(
            draft,
            catalog,
            Box::new(
                move |draft: RoleSetupDraft| match role_setup_draft_args(&draft) {
                    Ok(args) => {
                        tx.send(AppEvent::TiffanyOrchestratorRolesCommand { args });
                        Ok(())
                    }
                    Err(err) => Err(err),
                },
            ),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    fn open_tiffany_role_profile_setup_prompt(&mut self) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/roles profile' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{ROLES_USAGE}"
            ));
            return;
        }

        let tx = self.app_event_tx.clone();
        let view = RoleProfileSetupView::new(
            role_profile_setup_initial_draft(),
            Box::new(move |draft: RoleProfileSetupDraft| {
                match role_profile_setup_draft_args(&draft) {
                    Ok(args) => {
                        tx.send(AppEvent::TiffanyOrchestratorRolesCommand { args });
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    pub(super) fn dispatch_command(&mut self, cmd: SlashCommand) {
        if !self.ensure_tiffany_orchestrator_command_visible(cmd) {
            return;
        }
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.bottom_pane.drain_pending_submission_state();
            self.request_redraw();
            return;
        }

        match cmd {
            SlashCommand::Feedback => {
                if !self.config.feedback_enabled {
                    let params = crate::bottom_pane::feedback_disabled_params();
                    self.bottom_pane.show_selection_view(params);
                    self.request_redraw();
                    return;
                }
                // Step 1: pick a category (UI built in feedback_view)
                let params =
                    crate::bottom_pane::feedback_selection_params(self.app_event_tx.clone());
                self.bottom_pane.show_selection_view(params);
                self.request_redraw();
            }
            SlashCommand::New => {
                self.app_event_tx.send(AppEvent::NewSession);
            }
            SlashCommand::Archive => {
                self.bottom_pane.show_selection_view(SelectionViewParams {
                    title: Some("Archive this session?".to_string()),
                    subtitle: Some(
                        "Are you sure? This will archive the current session and exit tiffany-loop"
                            .to_string(),
                    ),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: vec![
                        SelectionItem {
                            name: "No, don't archive".to_string(),
                            description: Some("Return to the current session".to_string()),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, archive and exit".to_string(),
                            description: Some("Archive this session now".to_string()),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::ArchiveCurrentThread);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Delete => {
                self.bottom_pane.show_selection_view(SelectionViewParams {
                    title: Some("Delete this session?".to_string()),
                    subtitle: Some(
                        "Cannot be undone. Subagent threads will also be deleted.".to_string(),
                    ),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: vec![
                        SelectionItem {
                            name: "No, keep this session".to_string(),
                            description: Some("Return to the current session".to_string()),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, delete and exit".to_string(),
                            description: Some("Permanently delete this session now".to_string()),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::DeleteCurrentThread);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Clear => {
                self.app_event_tx.send(AppEvent::ClearUi);
            }
            SlashCommand::Resume => {
                self.app_event_tx.send(AppEvent::OpenResumePicker);
            }
            SlashCommand::Fork => {
                self.app_event_tx.send(AppEvent::ForkCurrentSession);
            }
            SlashCommand::App => {
                let Some(thread_id) = self.thread_id else {
                    self.add_error_message(
                        "Session is still starting; try /app again in a moment.".to_string(),
                    );
                    return;
                };
                self.app_event_tx
                    .send(AppEvent::OpenDesktopThread { thread_id });
            }
            SlashCommand::Init => {
                const INIT_PROMPT: &str = include_str!("../../prompt_for_init_command.md");
                self.submit_user_message(INIT_PROMPT.to_string().into());
            }
            SlashCommand::Compact => {
                if self.tiffany_orchestrator_shell {
                    self.dispatch_tiffany_history_command("compact");
                } else {
                    self.clear_token_usage();
                    if !self.bottom_pane.is_task_running() {
                        self.bottom_pane.set_task_running(/*running*/ true);
                    }
                    self.app_event_tx.compact();
                }
            }
            SlashCommand::Review => {
                self.open_review_popup();
            }
            SlashCommand::Rename => {
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                self.show_rename_prompt();
            }
            SlashCommand::Model => {
                self.open_model_popup();
            }
            SlashCommand::Personality => {
                self.open_personality_popup();
            }
            SlashCommand::Plan => {
                self.apply_plan_slash_command();
            }
            SlashCommand::Goal => {
                if !self.config.features.enabled(Feature::Goals) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadGoalMenu { thread_id });
                    self.append_message_history_entry("/goal".to_string());
                } else {
                    self.add_info_message(
                        GOAL_USAGE.to_string(),
                        Some(GOAL_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Roles => {
                self.dispatch_tiffany_roles_command("list".to_string());
            }
            SlashCommand::Queue => {
                self.add_tiffany_queue_output("");
            }
            SlashCommand::Thread => {
                self.dispatch_tiffany_thread_command("list".to_string());
            }
            SlashCommand::Jobs => {
                self.dispatch_tiffany_jobs_command("");
            }
            SlashCommand::History => {
                self.dispatch_tiffany_history_command("");
            }
            SlashCommand::Approvals => {
                self.dispatch_tiffany_approvals_command("");
            }
            SlashCommand::Continue => {
                self.dispatch_tiffany_continue_command("");
            }
            SlashCommand::Process => {
                self.dispatch_tiffany_process_command("");
            }
            SlashCommand::Outline => {
                self.toggle_tiffany_process_detail();
            }
            SlashCommand::Role => {
                self.open_tiffany_role_setup_prompt(None);
            }
            SlashCommand::Provider => {
                self.open_tiffany_provider_setup_prompt(None);
            }
            SlashCommand::Doctor => {
                self.dispatch_tiffany_doctor_command("");
            }
            SlashCommand::Help => {
                self.add_info_message(tiffany_orchestrator_help_text(), /*hint*/ None);
            }
            SlashCommand::Side | SlashCommand::Btw => {
                self.request_empty_side_conversation(cmd);
            }
            SlashCommand::Agent | SlashCommand::MultiAgents => {
                self.app_event_tx.send(AppEvent::OpenAgentPicker);
            }
            SlashCommand::Permissions => {
                self.open_permissions_popup();
            }
            SlashCommand::Vim => {
                self.toggle_vim_mode_and_notify();
            }
            SlashCommand::Keymap => {
                self.open_keymap_picker();
            }
            SlashCommand::ElevateSandbox => {
                #[cfg(target_os = "windows")]
                {
                    let windows_sandbox_level =
                        crate::windows_sandbox::level_from_config(&self.config);
                    let windows_degraded_sandbox_enabled =
                        matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken);
                    if !windows_degraded_sandbox_enabled {
                        // This command should not be visible/recognized outside degraded mode,
                        // but guard anyway in case something dispatches it directly.
                        return;
                    }

                    let Some(preset) = builtin_approval_presets()
                        .into_iter()
                        .find(|preset| preset.id == "auto")
                    else {
                        // Avoid panicking in interactive UI; treat this as a recoverable
                        // internal error.
                        self.add_error_message(
                            "Internal error: missing the 'auto' approval preset.".to_string(),
                        );
                        return;
                    };

                    if let Err(err) = self
                        .config
                        .permissions
                        .approval_policy
                        .can_set(&preset.approval)
                    {
                        self.add_error_message(err.to_string());
                        return;
                    }

                    self.session_telemetry.counter(
                        "codex.windows_sandbox.setup_elevated_sandbox_command",
                        /*inc*/ 1,
                        &[],
                    );
                    self.app_event_tx
                        .send(AppEvent::BeginWindowsSandboxElevatedSetup {
                            preset,
                            profile_selection: None,
                        });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = &self.session_telemetry;
                    // Not supported; on non-Windows this command should never be reachable.
                }
            }
            SlashCommand::SandboxReadRoot => {
                self.add_error_message(
                    "Usage: /sandbox-add-read-dir <absolute-directory-path>".to_string(),
                );
            }
            SlashCommand::Experimental => {
                self.open_experimental_popup();
            }
            SlashCommand::AutoReview => {
                self.open_auto_review_denials_popup();
            }
            SlashCommand::Memories => {
                self.open_memories_popup();
            }
            SlashCommand::Quit | SlashCommand::Exit => {
                self.request_quit_without_confirmation();
            }
            SlashCommand::Logout => {
                self.add_error_message(
                    "tiffany-loop does not use account login. Configure providers with /provider or `orchestrator config provider`."
                        .to_string(),
                );
            }
            SlashCommand::Copy => {
                self.copy_last_agent_markdown();
            }
            SlashCommand::Raw => {
                let enabled = self.toggle_raw_output_mode_and_notify();
                self.emit_raw_output_mode_changed(enabled);
            }
            SlashCommand::Diff => {
                self.add_diff_in_progress();
                let tx = self.app_event_tx.clone();
                let runner = self.workspace_command_runner.clone();
                let cwd = self
                    .current_cwd
                    .clone()
                    .unwrap_or_else(|| self.config.cwd.to_path_buf());
                tokio::spawn(async move {
                    let text = match runner {
                        Some(runner) => match get_git_diff(runner.as_ref(), &cwd).await {
                            Ok((is_git_repo, diff_text)) => {
                                if is_git_repo {
                                    diff_text
                                } else {
                                    "`/diff` — _not inside a git repository_".to_string()
                                }
                            }
                            Err(e) => format!("Failed to compute diff: {e}"),
                        },
                        None => "Failed to compute diff: workspace command runner unavailable"
                            .to_string(),
                    };
                    tx.send(AppEvent::DiffResult(text));
                });
            }
            SlashCommand::Mention => {
                self.insert_str("@");
            }
            SlashCommand::Skills => {
                self.open_skills_menu();
            }
            SlashCommand::Import => {
                self.app_event_tx
                    .send(AppEvent::OpenExternalAgentConfigMigration);
            }
            SlashCommand::Hooks => {
                self.add_hooks_output();
            }
            SlashCommand::Status => {
                if self.tiffany_orchestrator_shell {
                    self.add_tiffany_orchestrator_status_output();
                } else if self.should_prefetch_rate_limits() {
                    let request_id = self.next_status_refresh_request_id;
                    self.next_status_refresh_request_id =
                        self.next_status_refresh_request_id.wrapping_add(1);
                    self.add_status_output(/*refreshing_rate_limits*/ true, Some(request_id));
                    self.app_event_tx.send(AppEvent::RefreshRateLimits {
                        origin: RateLimitRefreshOrigin::StatusCommand { request_id },
                    });
                } else {
                    self.add_status_output(
                        /*refreshing_rate_limits*/ false, /*request_id*/ None,
                    );
                }
            }
            SlashCommand::Usage => {
                if self.ensure_token_activity_command_available() {
                    self.add_token_activity_output(tokens::TokenActivityView::Daily);
                }
            }
            SlashCommand::Ide => {
                self.handle_ide_command();
            }
            SlashCommand::DebugConfig => {
                self.add_debug_config_output();
            }
            SlashCommand::Title => {
                self.open_terminal_title_setup();
            }
            SlashCommand::Statusline => {
                self.open_status_line_setup();
            }
            SlashCommand::Theme => {
                self.open_theme_picker();
            }
            SlashCommand::Pets => {
                self.open_pets_picker();
            }
            SlashCommand::Ps => {
                self.add_ps_output();
            }
            SlashCommand::Stop => {
                self.clean_background_terminals();
            }
            SlashCommand::MemoryDrop => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::MemoryUpdate => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::Mcp => {
                self.add_mcp_output(McpServerStatusDetail::ToolsAndAuthOnly);
            }
            SlashCommand::Apps => {
                self.add_connectors_output();
            }
            SlashCommand::Plugins => {
                self.add_plugins_output();
            }
            SlashCommand::Rollout => {
                if let Some(path) = self.rollout_path() {
                    self.add_info_message(
                        format!("Current rollout path: {}", path.display()),
                        /*hint*/ None,
                    );
                } else {
                    self.add_info_message(
                        "Rollout path is not available yet.".to_string(),
                        /*hint*/ None,
                    );
                }
            }
            SlashCommand::TestApproval => {
                use std::collections::HashMap;

                use crate::approval_events::ApplyPatchApprovalRequestEvent;
                use crate::diff_model::FileChange;

                self.on_apply_patch_approval_request(
                    "1".to_string(),
                    ApplyPatchApprovalRequestEvent {
                        call_id: "1".to_string(),
                        turn_id: "turn-1".to_string(),
                        changes: HashMap::from([
                            (
                                PathBuf::from("/tmp/test.txt"),
                                FileChange::Add {
                                    content: "test".to_string(),
                                },
                            ),
                            (
                                PathBuf::from("/tmp/test2.txt"),
                                FileChange::Update {
                                    unified_diff: "+test\n-test2".to_string(),
                                    move_path: None,
                                },
                            ),
                        ]),
                        reason: None,
                        grant_root: Some(PathBuf::from("/tmp")),
                    },
                );
            }
        }
    }

    fn add_tiffany_orchestrator_status_output(&mut self) {
        let mode = if self.turn_lifecycle.agent_turn_running {
            "running"
        } else {
            "ready"
        };
        let queued = self.input_queue.queued_user_messages.len()
            + self.input_queue.rejected_steers_queue.len();
        let readiness = crate::tiffany_orchestrator::startup_readiness(
            self.tiffany_orchestrator_config.as_ref(),
        );
        let config = self
            .tiffany_orchestrator_config
            .as_ref()
            .and_then(orchestrator_config_path)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "default".to_string());
        let commands = tiffany_orchestrator_status_command_list();

        let mut lines: Vec<Line<'static>> = vec![
            vec![
                "◆ ".fg(crate::tiffany_orchestrator::TIFFANY_BLUE).bold(),
                "T>_ ".bold(),
                "tiffany-loop orchestrator".into(),
            ]
            .into(),
            vec![
                "status  ".dim(),
                mode.into(),
                "  queue ".dim(),
                queued.to_string().into(),
            ]
            .into(),
            vec![
                "flow    ".dim(),
                "direct / single / full  →  selected per prompt".into(),
            ]
            .into(),
            vec![
                "worker  ".dim(),
                "stable roles reuse native Claude Code, Codex, or Gemini sessions".into(),
            ]
            .into(),
            vec!["config  ".dim(), config.into()].into(),
        ];
        lines.extend(crate::tiffany_orchestrator::startup_readiness_lines(
            &readiness,
        ));
        lines.extend(crate::tiffany_orchestrator::startup_health_action_lines(
            &readiness,
        ));
        lines.extend([
            vec![
                "thread  ".dim(),
                "/thread shows role session reuse and native resume state".into(),
            ]
            .into(),
            vec![
                "history ".dim(),
                "/history status compares session-db and local-json".into(),
            ]
            .into(),
            vec![
                "compact ".dim(),
                "/history compact builds a readable handoff storyline".into(),
            ]
            .into(),
            vec!["cmd     ".dim(), commands.into()].into(),
        ]);
        if queued > 0 {
            lines.push(
                vec![
                "next    ".dim(),
                "queued prompts will run one at a time after the current orchestration finishes"
                    .into(),
            ]
                .into(),
            );
        }
        self.add_to_history(history_cell::PlainHistoryCell::new(lines));
    }

    fn add_tiffany_queue_output(&mut self, args: &str) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/queue' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{QUEUE_USAGE}"
            ));
            return;
        }

        let action = args.split_whitespace().next().unwrap_or("show");
        let action = action.to_ascii_lowercase();
        match action.as_str() {
            "show" | "status" | "" => self.show_tiffany_queue_output(),
            "clear" | "reset" => {
                let cleared = self.input_queue.queued_user_messages.len()
                    + self.input_queue.rejected_steers_queue.len();
                self.input_queue.queued_user_messages.clear();
                self.input_queue.queued_user_message_history_records.clear();
                self.input_queue.rejected_steers_queue.clear();
                self.input_queue.rejected_steer_history_records.clear();
                self.refresh_pending_input_preview();
                self.add_info_message(
                    format!("Queue cleared. Removed {cleared} queued item(s)."),
                    /*hint*/ None,
                );
            }
            "pause" => {
                let ready = self.input_queue.queued_user_messages.len()
                    + self.input_queue.rejected_steers_queue.len();
                self.input_queue.suppress_queue_autosend = true;
                self.add_info_message(
                    format!(
                        "Queue paused. {ready} queued item(s) will stay queued until /queue resume or /queue run."
                    ),
                    /*hint*/ None,
                );
            }
            "resume" | "unpause" => {
                self.input_queue.suppress_queue_autosend = false;
                let ready = self.input_queue.queued_user_messages.len()
                    + self.input_queue.rejected_steers_queue.len();
                let running = self.is_user_turn_pending_or_running();
                let started = self.maybe_send_merged_tiffany_queued_input();
                let message = if started {
                    "Queue resumed. Started queued prompt(s).".to_string()
                } else if running && ready > 0 {
                    format!(
                        "Queue resumed. {ready} queued item(s) armed; they will run after the current orchestration finishes."
                    )
                } else {
                    format!("Queue resumed. {ready} queued item(s) ready.")
                };
                self.add_info_message(message, /*hint*/ None);
            }
            "run" | "start" | "go" => {
                self.input_queue.suppress_queue_autosend = false;
                let has_queued = self.input_queue.has_queued_follow_up_messages();
                let running = self.is_user_turn_pending_or_running();
                if self.maybe_send_merged_tiffany_queued_input() {
                    self.add_info_message(
                        "Queue started. Queued prompts are being submitted.".to_string(),
                        /*hint*/ None,
                    );
                } else if running && has_queued {
                    self.add_info_message(
                        "Queue armed. Current orchestration is still running; queued prompts will run after it finishes."
                            .to_string(),
                        /*hint*/ None,
                    );
                } else {
                    self.show_tiffany_queue_output();
                }
            }
            _ => self.add_error_message(QUEUE_USAGE.to_string()),
        }
        self.request_redraw();
    }

    fn show_tiffany_queue_output(&mut self) {
        let preview = self.input_queue.preview();
        let queued = preview.queued_messages.len();
        let rejected = preview.rejected_steers.len();
        let pending = preview.pending_steers.len();
        let paused = self.input_queue.suppress_queue_autosend;
        let running = self.is_user_turn_pending_or_running();
        let status = if paused {
            "paused"
        } else if running {
            "waiting"
        } else {
            "ready"
        };
        let mut lines: Vec<Line<'static>> = vec![
            vec![
                "◆ ".fg(crate::tiffany_orchestrator::TIFFANY_BLUE).bold(),
                "queue ".bold(),
                status.into(),
            ]
            .into(),
            vec![
                "items  ".dim(),
                format!("{queued} queued · {rejected} retry-first · {pending} pending").into(),
            ]
            .into(),
            vec![
                "next   ".dim(),
                tiffany_queue_next_action(queued, rejected, pending, paused, running).into(),
            ]
            .into(),
        ];

        if queued + rejected + pending == 0 {
            lines.push(vec!["state  ".dim(), "empty".into()].into());
        } else {
            push_tiffany_queue_preview_section(
                &mut lines,
                "retry-first",
                "runs before queued prompts",
                &preview.rejected_steers,
            );
            push_tiffany_queue_preview_section(
                &mut lines,
                "queued",
                "normal follow-up prompts",
                &preview.queued_messages,
            );
            push_tiffany_queue_preview_section(
                &mut lines,
                "pending",
                "submitted, waiting for turn start/history",
                &preview.pending_steers,
            );
        }
        lines.push(
            vec![
                "cmd    ".dim(),
                "/queue clear  /queue pause  /queue resume  /queue run".into(),
            ]
            .into(),
        );
        self.add_to_history(history_cell::PlainHistoryCell::new(lines));
    }

    fn toggle_tiffany_process_detail(&mut self) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(
                "'/o' is available in tiffany-loop orchestrator mode.".to_string(),
            );
            return;
        }
        self.tiffany_process_detail_expanded = !self.tiffany_process_detail_expanded;
        let args = if self.tiffany_process_detail_expanded {
            "full"
        } else {
            "compact"
        };
        let label = if self.tiffany_process_detail_expanded {
            "expanded"
        } else {
            "folded"
        };
        self.add_info_message(
            format!("Process detail {label}. Loading /history {args}."),
            /*hint*/ None,
        );
        self.dispatch_tiffany_history_command(args);
    }

    /// Run an inline slash command.
    ///
    /// Branches that prepare arguments should pass `record_history: false` to the composer because
    /// the staged slash-command entry is the recall record; using the normal submission-history
    /// path as well would make a single command appear twice during Up-arrow navigation.
    pub(super) fn dispatch_command_with_args(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        if !self.ensure_tiffany_orchestrator_command_visible(cmd) {
            return;
        }
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.supports_inline_args() {
            self.dispatch_command(cmd);
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.request_redraw();
            return;
        }

        let trimmed = args.trim();
        if trimmed.is_empty() {
            self.dispatch_command(cmd);
            return;
        }

        if cmd == SlashCommand::Goal {
            self.dispatch_prepared_command_with_args(
                cmd,
                PreparedSlashCommandArgs {
                    args,
                    text_elements,
                    pending_pastes: self.bottom_pane.composer_pending_pastes(),
                    local_images: self.bottom_pane.composer_local_images(),
                    remote_image_urls: self.bottom_pane.remote_image_urls(),
                    mention_bindings: Vec::new(),
                    source: SlashCommandDispatchSource::Live,
                },
            );
            return;
        }

        let Some((prepared_args, prepared_elements)) =
            self.prepare_live_inline_args(args, text_elements)
        else {
            return;
        };
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: prepared_args,
                text_elements: prepared_elements,
                pending_pastes: Vec::new(),
                local_images: Vec::new(),
                remote_image_urls: Vec::new(),
                mention_bindings: Vec::new(),
                source: SlashCommandDispatchSource::Live,
            },
        );
    }

    fn prepare_live_inline_args(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
    ) -> Option<(String, Vec<TextElement>)> {
        if self.bottom_pane.composer_text().is_empty() {
            Some((args, text_elements))
        } else {
            self.bottom_pane
                .prepare_inline_args_submission(/*record_history*/ false)
        }
    }

    fn clear_live_goal_submission(&mut self) {
        self.bottom_pane
            .set_composer_text(String::new(), Vec::new(), Vec::new());
        self.bottom_pane.set_composer_pending_pastes(Vec::new());
        self.bottom_pane.drain_pending_submission_state();
    }

    fn prepared_inline_user_message(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
        mut local_images: Vec<LocalImageAttachment>,
        mut remote_image_urls: Vec<String>,
        mut mention_bindings: Vec<MentionBinding>,
        source: SlashCommandDispatchSource,
    ) -> UserMessage {
        if source == SlashCommandDispatchSource::Live {
            local_images = self
                .bottom_pane
                .take_recent_submission_images_with_placeholders();
            remote_image_urls = self.take_remote_image_urls();
            mention_bindings = self.bottom_pane.take_recent_submission_mention_bindings();
        }
        UserMessage {
            text: args,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        }
    }

    fn dispatch_prepared_command_with_args(
        &mut self,
        cmd: SlashCommand,
        prepared: PreparedSlashCommandArgs,
    ) {
        let PreparedSlashCommandArgs {
            args,
            text_elements,
            pending_pastes,
            local_images,
            remote_image_urls,
            mention_bindings,
            source,
        } = prepared;
        let trimmed = args.trim();
        match cmd {
            SlashCommand::Usage => {
                if self.ensure_token_activity_command_available() {
                    match tokens::TokenActivityView::parse(trimmed) {
                        Some(view) => self.add_token_activity_output(view),
                        None => self.add_error_message(
                            "Usage: /usage [daily|weekly|cumulative]".to_string(),
                        ),
                    }
                }
            }
            SlashCommand::Ide => {
                self.handle_ide_command_args(trimmed);
            }
            SlashCommand::Mcp => match trimmed.to_ascii_lowercase().as_str() {
                "verbose" => self.add_mcp_output(McpServerStatusDetail::Full),
                _ => self.add_error_message("Usage: /mcp [verbose]".to_string()),
            },
            SlashCommand::Keymap => match trimmed.to_ascii_lowercase().as_str() {
                "" => self.open_keymap_picker(),
                "debug" => {
                    match crate::keymap::RuntimeKeymap::from_config(&self.config.tui_keymap) {
                        Ok(runtime_keymap) => self.open_keymap_debug(&runtime_keymap),
                        Err(err) => {
                            self.add_error_message(format!(
                                "Invalid `tui.keymap` configuration: {err}"
                            ));
                        }
                    }
                }
                _ => self.add_error_message("Usage: /keymap [debug]".to_string()),
            },
            SlashCommand::Raw => match trimmed.to_ascii_lowercase().as_str() {
                "on" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ true);
                    self.emit_raw_output_mode_changed(/*enabled*/ true);
                }
                "off" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ false);
                    self.emit_raw_output_mode_changed(/*enabled*/ false);
                }
                _ => self.add_error_message(RAW_USAGE.to_string()),
            },
            SlashCommand::Rename if !trimmed.is_empty() => {
                if !self.ensure_thread_rename_allowed() {
                    return;
                }
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                let Some(name) = normalize_thread_name(&args) else {
                    self.add_error_message("Thread name cannot be empty.".to_string());
                    return;
                };
                self.app_event_tx.set_thread_name(name);
            }
            SlashCommand::Plan if !trimmed.is_empty() => {
                if !self.apply_plan_slash_command() {
                    return;
                }
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                if self.is_session_configured() {
                    self.reasoning_buffer.clear();
                    self.full_reasoning_buffer.clear();
                    self.set_status_header(String::from("Working"));
                    self.submit_user_message(user_message);
                } else {
                    self.queue_user_message(user_message);
                }
            }
            SlashCommand::Goal if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::Goals) {
                    if source == SlashCommandDispatchSource::Live {
                        self.clear_live_goal_submission();
                    }
                    return;
                }
                enum GoalControlCommand {
                    Clear,
                    SetStatus(AppThreadGoalStatus),
                }
                let control_command = match trimmed.to_ascii_lowercase().as_str() {
                    "clear" => Some(GoalControlCommand::Clear),
                    "edit" => {
                        self.app_event_tx.send(AppEvent::OpenThreadGoalEditor {
                            thread_id: self.thread_id,
                        });
                        if source == SlashCommandDispatchSource::Live {
                            self.clear_live_goal_submission();
                        }
                        return;
                    }
                    "pause" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Paused)),
                    "resume" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Active)),
                    _ => None,
                };
                if let Some(command) = control_command {
                    let Some(thread_id) = self.thread_id else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some(
                                "The session must start before you can change a goal.".to_string(),
                            ),
                        );
                        if source == SlashCommandDispatchSource::Live {
                            self.clear_live_goal_submission();
                        }
                        return;
                    };
                    match command {
                        GoalControlCommand::Clear => {
                            self.app_event_tx
                                .send(AppEvent::ClearThreadGoal { thread_id });
                        }
                        GoalControlCommand::SetStatus(status) => {
                            self.app_event_tx
                                .send(AppEvent::SetThreadGoalStatus { thread_id, status });
                        }
                    }
                    self.append_message_history_entry(format!("/goal {trimmed}"));
                    if source == SlashCommandDispatchSource::Live {
                        self.clear_live_goal_submission();
                    }
                    return;
                }
                let draft = GoalDraft {
                    objective: args,
                    text_elements,
                    pending_pastes,
                    local_images,
                    remote_image_urls,
                };
                let Some(thread_id) = self.thread_id else {
                    if source == SlashCommandDispatchSource::Live {
                        const GOAL_PREFIX: &str = "/goal ";
                        let text_elements = draft
                            .text_elements
                            .into_iter()
                            .map(|element| {
                                element.map_range(|range| ByteRange {
                                    start: range.start + GOAL_PREFIX.len(),
                                    end: range.end + GOAL_PREFIX.len(),
                                })
                            })
                            .collect();
                        self.queue_user_message_with_options(
                            UserMessage {
                                text: format!("{GOAL_PREFIX}{}", draft.objective),
                                local_images: draft.local_images,
                                remote_image_urls: draft.remote_image_urls,
                                text_elements,
                                mention_bindings: Vec::new(),
                            },
                            QueuedInputAction::ParseSlash,
                            draft.pending_pastes,
                        );
                        self.clear_live_goal_submission();
                    } else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some("The session must start before you can set a goal.".to_string()),
                        );
                    }
                    return;
                };
                let history_objective = draft.objective.clone();
                self.app_event_tx.send(AppEvent::SetThreadGoalDraft {
                    thread_id,
                    draft,
                    mode: ThreadGoalSetMode::ConfirmIfExists,
                });
                self.append_message_history_entry(format!("/goal {history_objective}"));
                if source == SlashCommandDispatchSource::Live {
                    self.clear_live_goal_submission();
                }
            }
            SlashCommand::Roles if !trimmed.is_empty() => {
                if role_profile_setup_prompt(trimmed) {
                    self.open_tiffany_role_profile_setup_prompt();
                } else {
                    self.dispatch_tiffany_roles_command(args);
                }
            }
            SlashCommand::Queue if !trimmed.is_empty() => {
                self.add_tiffany_queue_output(trimmed);
            }
            SlashCommand::Role if !trimmed.is_empty() => {
                if let Some(role) = role_setup_prompt_role(trimmed) {
                    self.open_tiffany_role_setup_prompt(role);
                } else {
                    self.dispatch_tiffany_role_command(args);
                }
            }
            SlashCommand::Provider if !trimmed.is_empty() => {
                if let Some(provider) = provider_setup_prompt_provider(trimmed) {
                    self.open_tiffany_provider_setup_prompt(provider);
                } else {
                    self.dispatch_tiffany_provider_command(args);
                }
            }
            SlashCommand::Doctor if !trimmed.is_empty() => {
                self.dispatch_tiffany_doctor_command(args);
            }
            SlashCommand::Thread if !trimmed.is_empty() => {
                self.dispatch_tiffany_thread_command(args);
            }
            SlashCommand::Jobs if !trimmed.is_empty() => {
                self.dispatch_tiffany_jobs_command(args);
            }
            SlashCommand::History if !trimmed.is_empty() => {
                self.dispatch_tiffany_history_command(args);
            }
            SlashCommand::Approvals if !trimmed.is_empty() => {
                self.dispatch_tiffany_approvals_command(args);
            }
            SlashCommand::Compact if self.tiffany_orchestrator_shell && !trimmed.is_empty() => {
                self.dispatch_tiffany_history_command(format!("compact {args}"));
            }
            SlashCommand::Continue if !trimmed.is_empty() => {
                self.dispatch_tiffany_continue_command(args);
            }
            SlashCommand::Process if !trimmed.is_empty() => {
                self.dispatch_tiffany_process_command(args);
            }
            SlashCommand::Outline if !trimmed.is_empty() => {
                self.toggle_tiffany_process_detail();
            }
            SlashCommand::Side | SlashCommand::Btw if !trimmed.is_empty() => {
                let Some(parent_thread_id) = self.thread_id else {
                    let command = cmd.command();
                    self.add_error_message(format!(
                        "'/{command}' is unavailable before the session starts."
                    ));
                    return;
                };
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                self.request_side_conversation(parent_thread_id, Some(user_message));
            }
            SlashCommand::Review if !trimmed.is_empty() => {
                self.submit_op(AppCommand::review(ReviewTarget::Custom {
                    instructions: args,
                }));
            }
            SlashCommand::Resume if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::ResumeSessionByIdOrName(args));
            }
            SlashCommand::SandboxReadRoot if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::BeginWindowsSandboxGrantReadRoot { path: args });
            }
            SlashCommand::Pets
                if matches!(
                    args.trim().to_ascii_lowercase().as_str(),
                    "disable" | "disabled" | "hide" | "hidden" | "off" | "none"
                ) =>
            {
                self.app_event_tx.send(AppEvent::PetDisabled);
            }
            SlashCommand::Pets if !trimmed.is_empty() => {
                self.select_pet_by_id(args);
            }
            _ => self.dispatch_command(cmd),
        }
        if source == SlashCommandDispatchSource::Live && cmd != SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    pub(super) fn submit_queued_slash_prompt(
        &mut self,
        queued_message: QueuedUserMessage,
    ) -> QueueDrain {
        let QueuedUserMessage {
            user_message,
            pending_pastes,
            ..
        } = queued_message;
        let UserMessage {
            text,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        } = user_message;
        let Some((name, rest, rest_offset)) = parse_slash_name(&text) else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        if name.contains('/') {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }

        let service_tier_commands = self.current_model_service_tier_commands();
        let Some(command) =
            find_slash_command(name, self.builtin_command_flags(), &service_tier_commands)
        else {
            let message = if self.tiffany_orchestrator_shell {
                tiffany_orchestrator_unsupported_command_message(name)
            } else {
                None
            }
            .unwrap_or_else(|| {
                format!(
                    r#"Unrecognized command '/{name}'. Type "/" for a list of supported commands."#
                )
            });
            self.add_info_message(message, /*hint*/ None);
            return QueueDrain::Continue;
        };

        if rest.is_empty() {
            return match command {
                SlashCommandItem::Builtin(cmd) => {
                    self.dispatch_command(cmd);
                    self.queued_command_drain_result(cmd)
                }
                SlashCommandItem::ServiceTier(command) => {
                    self.handle_service_tier_command_dispatch(command);
                    QueueDrain::Continue
                }
            };
        }

        if !command.supports_inline_args() {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }
        let SlashCommandItem::Builtin(cmd) = command else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        let trimmed_start = rest.trim_start();
        let leading_trimmed = rest.len().saturating_sub(trimmed_start.len());
        let trimmed_rest = trimmed_start.trim_end();
        let args_elements = Self::slash_command_args_elements(
            trimmed_rest,
            rest_offset + leading_trimmed,
            &text_elements,
        );
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: trimmed_rest.to_string(),
                text_elements: args_elements,
                pending_pastes,
                local_images,
                remote_image_urls,
                mention_bindings,
                source: SlashCommandDispatchSource::Queued,
            },
        );
        self.queued_command_drain_result(cmd)
    }

    fn builtin_command_flags(&self) -> BuiltinCommandFlags {
        #[cfg(target_os = "windows")]
        let allow_elevate_sandbox = {
            let windows_sandbox_level = crate::windows_sandbox::level_from_config(&self.config);
            matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken)
        };
        #[cfg(not(target_os = "windows"))]
        let allow_elevate_sandbox = false;

        BuiltinCommandFlags {
            collaboration_modes_enabled: self.collaboration_modes_enabled(),
            connectors_enabled: self.connectors_enabled(),
            plugins_command_enabled: self.config.features.enabled(Feature::Plugins),
            token_activity_command_enabled: self.has_codex_backend_auth,
            goal_command_enabled: self.config.features.enabled(Feature::Goals),
            service_tier_commands_enabled: self.fast_mode_enabled(),
            personality_command_enabled: self.config.features.enabled(Feature::Personality),
            allow_elevate_sandbox,
            side_conversation_active: self.active_side_conversation,
            tiffany_orchestrator_shell: self.tiffany_orchestrator_shell,
        }
    }

    fn ensure_token_activity_command_available(&mut self) -> bool {
        if self.has_codex_backend_auth {
            return true;
        }
        self.add_error_message(USAGE_CHATGPT_LOGIN_REQUIRED.to_string());
        false
    }

    fn queued_command_drain_result(&self, cmd: SlashCommand) -> QueueDrain {
        if self.is_user_turn_pending_or_running() || !self.bottom_pane.no_modal_or_popup_active() {
            return QueueDrain::Stop;
        }
        match cmd {
            SlashCommand::Ide
            | SlashCommand::Status
            | SlashCommand::Usage
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Rollout
            | SlashCommand::Copy
            | SlashCommand::Raw
            | SlashCommand::Vim
            | SlashCommand::Diff
            | SlashCommand::App
            | SlashCommand::Rename
            | SlashCommand::Provider
            | SlashCommand::Role
            | SlashCommand::Roles
            | SlashCommand::Queue
            | SlashCommand::Thread
            | SlashCommand::Jobs
            | SlashCommand::History
            | SlashCommand::Approvals
            | SlashCommand::Continue
            | SlashCommand::Process
            | SlashCommand::Outline
            | SlashCommand::Doctor
            | SlashCommand::Help
            | SlashCommand::TestApproval => QueueDrain::Continue,
            SlashCommand::Compact => {
                if self.tiffany_orchestrator_shell {
                    QueueDrain::Continue
                } else {
                    QueueDrain::Stop
                }
            }
            SlashCommand::Feedback
            | SlashCommand::New
            | SlashCommand::Archive
            | SlashCommand::Delete
            | SlashCommand::Clear
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Review
            | SlashCommand::Model
            | SlashCommand::Personality
            | SlashCommand::Plan
            | SlashCommand::Goal
            | SlashCommand::Side
            | SlashCommand::Btw
            | SlashCommand::Keymap
            | SlashCommand::Agent
            | SlashCommand::MultiAgents
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::AutoReview
            | SlashCommand::Memories
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Logout
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Import
            | SlashCommand::Hooks
            | SlashCommand::Title
            | SlashCommand::Statusline
            | SlashCommand::Theme
            | SlashCommand::Pets => QueueDrain::Stop,
        }
    }

    fn slash_command_args_elements(
        rest: &str,
        rest_offset: usize,
        text_elements: &[TextElement],
    ) -> Vec<TextElement> {
        if rest.is_empty() || text_elements.is_empty() {
            return Vec::new();
        }
        text_elements
            .iter()
            .filter_map(|elem| {
                if elem.byte_range.end <= rest_offset {
                    return None;
                }
                let start = elem.byte_range.start.saturating_sub(rest_offset);
                let mut end = elem.byte_range.end.saturating_sub(rest_offset);
                if start >= rest.len() {
                    return None;
                }
                end = end.min(rest.len());
                (start < end).then_some(elem.map_range(|_| ByteRange { start, end }))
            })
            .collect()
    }

    fn ensure_slash_command_allowed_in_side_conversation(&mut self, cmd: SlashCommand) -> bool {
        if !self.active_side_conversation || cmd.available_in_side_conversation() {
            return true;
        }
        self.add_error_message(format!(
            "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
            cmd.command()
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }

    fn ensure_side_command_allowed_outside_review(&mut self, cmd: SlashCommand) -> bool {
        if !matches!(cmd, SlashCommand::Side | SlashCommand::Btw) || !self.review.is_review_mode {
            return true;
        }

        let command = cmd.command();
        self.add_error_message(format!(
            "'/{command}' is unavailable while code review is running."
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }
}

fn provider_setup_prompt_provider(args: &str) -> Option<Option<&str>> {
    let mut parts = args.split_whitespace();
    let first = parts.next()?;
    if first.eq_ignore_ascii_case("setup")
        || first.eq_ignore_ascii_case("input")
        || first.eq_ignore_ascii_case("edit")
        || first.eq_ignore_ascii_case("modify")
    {
        let provider = parts.next();
        if parts.next().is_some() {
            return None;
        }
        return Some(provider);
    }

    if matches!(
        first.to_ascii_lowercase().as_str(),
        "list"
            | "show"
            | "status"
            | "key"
            | "set-key"
            | "env"
            | "set-env"
            | "endpoint"
            | "base-url"
            | "url"
            | "set-endpoint"
            | "delete"
            | "remove"
            | "rm"
    ) || parts.next().is_some()
    {
        return None;
    }

    Some(Some(first))
}

fn role_setup_prompt_role(args: &str) -> Option<Option<&str>> {
    let mut parts = args.split_whitespace();
    let first = parts.next()?;
    if first.eq_ignore_ascii_case("setup")
        || first.eq_ignore_ascii_case("input")
        || first.eq_ignore_ascii_case("edit")
        || first.eq_ignore_ascii_case("modify")
    {
        let role = parts.next();
        if parts.next().is_some() {
            return None;
        }
        return Some(role);
    }

    if matches!(
        first.to_ascii_lowercase().as_str(),
        "list"
            | "show"
            | "route"
            | "use"
            | "snippet"
            | "save"
            | "set"
            | "add"
            | "register"
            | "options"
            | "opts"
            | "models"
            | "delete"
            | "remove"
            | "rm"
    ) || parts.next().is_some()
    {
        return None;
    }

    Some(Some(first))
}

fn role_profile_setup_prompt(args: &str) -> bool {
    let mut parts = args.split_whitespace();
    let Some(first) = parts.next() else {
        return false;
    };
    matches!(
        first.to_ascii_lowercase().as_str(),
        "profile" | "setup-profile" | "profile-setup"
    ) && parts.next().is_none()
}

fn provider_setup_initial_draft(
    provider: &str,
    config: Option<&crate::tiffany_orchestrator::TiffanyOrchestratorConfig>,
) -> ProviderSetupDraft {
    let provider = provider.trim();
    let provider = if provider.is_empty() {
        "custom"
    } else {
        provider
    };
    let existing = config.and_then(|config| read_provider_config(config, provider));
    let existing_key = existing
        .as_ref()
        .and_then(|provider| provider.api_key.as_deref());
    let env = existing_key
        .and_then(env_var_reference)
        .or_else(|| provider_default_env(provider).map(str::to_string))
        .unwrap_or_default();
    let key = match existing_key {
        Some(value) if !value.trim().is_empty() && env_var_reference(value).is_none() => {
            "<unchanged>".to_string()
        }
        _ => String::new(),
    };

    let kind = existing
        .as_ref()
        .and_then(|provider| provider.kind.clone())
        .unwrap_or_else(|| provider_default_kind(provider).to_string());

    ProviderSetupDraft {
        provider: provider.to_string(),
        kind,
        env,
        key,
        endpoint: existing
            .as_ref()
            .and_then(|provider| provider.base_url.clone())
            .or_else(|| provider_default_endpoint(provider).map(str::to_string))
            .unwrap_or_default(),
    }
}

fn role_setup_initial_draft(
    role: Option<&str>,
    config: Option<&crate::tiffany_orchestrator::TiffanyOrchestratorConfig>,
) -> RoleSetupDraft {
    let role = role
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("worker-codex");
    if let Some(existing) = config.and_then(|config| read_role_setup(config, role)) {
        return existing;
    }
    let defaults = role_default_setup(role).unwrap_or(WORKER_CODEX_ROLE_DEFAULTS);
    RoleSetupDraft {
        role: role.to_string(),
        provider: defaults.provider.to_string(),
        model_id: defaults.model_id.to_string(),
        model_name: defaults.model_name.to_string(),
        runtime: defaults.runtime.to_string(),
        teams: defaults.teams.to_string(),
    }
}

fn role_setup_catalog(
    config: Option<&crate::tiffany_orchestrator::TiffanyOrchestratorConfig>,
) -> RoleSetupCatalog {
    config.and_then(read_role_setup_catalog).unwrap_or_default()
}

fn read_role_setup_catalog(
    config: &crate::tiffany_orchestrator::TiffanyOrchestratorConfig,
) -> Option<RoleSetupCatalog> {
    let path = orchestrator_config_path(config)?;
    let raw = std::fs::read_to_string(path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).ok()?;
    Some(role_setup_catalog_from_yaml(&yaml))
}

pub(super) fn role_setup_catalog_from_yaml(yaml: &serde_yaml::Value) -> RoleSetupCatalog {
    let mut catalog = RoleSetupCatalog::default();

    if let Some(providers) = yaml
        .get("providers")
        .and_then(serde_yaml::Value::as_mapping)
    {
        for (key, value) in providers {
            let Some(provider) = key
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let hint = value
                .as_mapping()
                .map(|mapping| {
                    let kind = yaml_mapping_string(mapping, "type")
                        .unwrap_or_else(|| provider_default_kind(provider).to_string());
                    match yaml_mapping_string(mapping, "base_url") {
                        Some(url) if !url.trim().is_empty() => format!("{kind} · {}", url.trim()),
                        _ => kind,
                    }
                })
                .unwrap_or_else(|| provider_default_kind(provider).to_string());
            catalog.providers.push(RoleSetupChoice::new(
                provider.to_string(),
                provider.to_string(),
                hint,
            ));
        }
    }

    if let Some(runtimes) = yaml.get("runtimes").and_then(serde_yaml::Value::as_mapping) {
        for (key, value) in runtimes {
            let Some(runtime) = key
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let hint = value
                .as_mapping()
                .map(|mapping| {
                    let kind = yaml_mapping_string(mapping, "type")
                        .unwrap_or_else(|| "runtime".to_string());
                    match yaml_mapping_string(mapping, "binary") {
                        Some(binary) if !binary.trim().is_empty() => {
                            format!("{kind} · {}", binary.trim())
                        }
                        _ => kind,
                    }
                })
                .unwrap_or_else(|| "runtime".to_string());
            catalog.runtimes.push(RoleSetupChoice::new(
                runtime.to_string(),
                runtime.to_string(),
                hint,
            ));
        }
    }

    if let Some(models) = yaml.get("models").and_then(serde_yaml::Value::as_sequence) {
        for item in models {
            let Some(mapping) = item.as_mapping() else {
                continue;
            };
            let Some(id) = yaml_mapping_string(mapping, "id")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let Some(provider) = yaml_mapping_string(mapping, "provider")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let name = yaml_mapping_string(mapping, "name")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| id.clone());
            catalog.models.push(RoleSetupModelChoice::new(
                provider,
                id.clone(),
                name,
                format!("id {id}"),
            ));
        }
    }

    if let Some(roles) = yaml.get("roles").and_then(serde_yaml::Value::as_mapping) {
        for (key, value) in roles {
            let Some(role) = key
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let hint = value
                .as_mapping()
                .map(|mapping| {
                    let model = yaml_mapping_string(mapping, "model")
                        .unwrap_or_else(|| "model".to_string());
                    let runtime = yaml_mapping_string(mapping, "runtime")
                        .unwrap_or_else(|| "runtime".to_string());
                    format!("{model} @ {runtime}")
                })
                .unwrap_or_else(|| "configured role".to_string());
            catalog.roles.push(RoleSetupChoice::new(
                role.to_string(),
                role.to_string(),
                hint,
            ));
        }
    }

    catalog
}

#[derive(Debug, Default)]
struct RawProviderConfig {
    kind: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
}

#[derive(Debug, Default)]
struct RawModelConfig {
    id: Option<String>,
    provider: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Default)]
struct RawRoleConfig {
    model: Option<String>,
    runtime: Option<String>,
    agent_teams: Option<bool>,
}

fn read_provider_config(
    config: &crate::tiffany_orchestrator::TiffanyOrchestratorConfig,
    provider: &str,
) -> Option<RawProviderConfig> {
    let path = orchestrator_config_path(config)?;
    let raw = std::fs::read_to_string(path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).ok()?;
    let providers = yaml.get("providers")?.as_mapping()?;
    let provider_value = serde_yaml::Value::String(provider.to_string());
    let mapping = providers.get(&provider_value)?.as_mapping()?;
    Some(RawProviderConfig {
        kind: yaml_mapping_string(mapping, "type"),
        api_key: yaml_mapping_string(mapping, "api_key"),
        base_url: yaml_mapping_string(mapping, "base_url"),
    })
}

fn read_role_setup(
    config: &crate::tiffany_orchestrator::TiffanyOrchestratorConfig,
    role: &str,
) -> Option<RoleSetupDraft> {
    let path = orchestrator_config_path(config)?;
    let raw = std::fs::read_to_string(path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).ok()?;
    let role_config = read_role_config_from_yaml(&yaml, role)?;
    let model_id = role_config.model?;
    let model = read_model_config_from_yaml(&yaml, &model_id).unwrap_or_else(|| RawModelConfig {
        id: Some(model_id.clone()),
        provider: None,
        name: Some(model_id.clone()),
    });
    let provider = model.provider.unwrap_or_default();
    let model_name = model.name.unwrap_or_else(|| model_id.clone());
    let runtime = role_config.runtime.unwrap_or_default();
    let teams = match role_config.agent_teams {
        Some(true) => "yes",
        Some(false) => "no",
        None => "auto",
    };

    Some(RoleSetupDraft {
        role: role.to_string(),
        provider,
        model_id: model.id.unwrap_or(model_id),
        model_name,
        runtime,
        teams: teams.to_string(),
    })
}

fn read_role_config_from_yaml(yaml: &serde_yaml::Value, role: &str) -> Option<RawRoleConfig> {
    let roles = yaml.get("roles")?.as_mapping()?;
    let role_value = serde_yaml::Value::String(role.to_string());
    let mapping = roles.get(&role_value)?.as_mapping()?;
    Some(RawRoleConfig {
        model: yaml_mapping_string(mapping, "model"),
        runtime: yaml_mapping_string(mapping, "runtime"),
        agent_teams: yaml_mapping_bool(mapping, "agent_teams"),
    })
}

fn read_model_config_from_yaml(yaml: &serde_yaml::Value, model_id: &str) -> Option<RawModelConfig> {
    let models = yaml.get("models")?.as_sequence()?;
    for item in models {
        let mapping = item.as_mapping()?;
        let id = yaml_mapping_string(mapping, "id")?;
        if id == model_id {
            return Some(RawModelConfig {
                id: Some(id),
                provider: yaml_mapping_string(mapping, "provider"),
                name: yaml_mapping_string(mapping, "name"),
            });
        }
    }
    None
}

fn orchestrator_config_path(
    config: &crate::tiffany_orchestrator::TiffanyOrchestratorConfig,
) -> Option<std::path::PathBuf> {
    match config
        .config_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    {
        Some(path) => Some(crate::tiffany_orchestrator::expand_home_path(path)),
        None => dirs::home_dir().map(|home| home.join(".orchestrator/config.yaml")),
    }
}

fn yaml_mapping_string(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string)
}

fn yaml_mapping_bool(mapping: &serde_yaml::Mapping, key: &str) -> Option<bool> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(serde_yaml::Value::as_bool)
}

fn env_var_reference(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(env) = value
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
    {
        return (!env.trim().is_empty()).then(|| env.trim().to_string());
    }
    value
        .strip_prefix('$')
        .filter(|env| !env.trim().is_empty())
        .map(|env| env.trim().to_string())
}

fn provider_default_kind(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => "anthropic",
        "google" | "gemini" => "google",
        "ollama" => "ollama",
        _ => "openai",
    }
}

fn provider_default_env(provider: &str) -> Option<&'static str> {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "minimax" => Some("MINIMAX_API_KEY"),
        "deepseek" => Some("DEEPSEEK_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "moonshot" => Some("MOONSHOT_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        "mistral" => Some("MISTRAL_API_KEY"),
        "cohere" => Some("COHERE_API_KEY"),
        "custom" => Some("CUSTOM_API_KEY"),
        "google" | "gemini" => Some("GOOGLE_API_KEY"),
        "ollama" => None,
        _ => Some("CUSTOM_API_KEY"),
    }
}

fn provider_default_endpoint(provider: &str) -> Option<&'static str> {
    match provider.to_ascii_lowercase().as_str() {
        "ollama" => Some("http://localhost:11434"),
        "minimax" => Some("https://api.minimaxi.com/v1"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "moonshot" => Some("https://api.moonshot.ai/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        _ => None,
    }
}

fn role_profile_setup_initial_draft() -> RoleProfileSetupDraft {
    RoleProfileSetupDraft {
        name: "default".to_string(),
        planner: "minimax/MiniMax-M3@codex".to_string(),
        critic: "minimax/MiniMax-M3@codex".to_string(),
        reviewer: "minimax/MiniMax-M3@codex".to_string(),
        worker_cc: "anthropic/claude-sonnet-4-6@claude-code".to_string(),
        worker_codex: "minimax/MiniMax-M3@codex".to_string(),
        worker_gemini: "google/gemini-1.5-pro@gemini".to_string(),
    }
}

pub(super) fn provider_setup_draft_args(draft: &ProviderSetupDraft) -> Result<String, String> {
    let provider = draft.provider.trim();
    let kind = draft.kind.trim();
    let env = draft.env.trim().trim_start_matches('$');
    let key = draft.key.trim();
    let endpoint = draft.endpoint.trim();

    if provider.is_empty() {
        return Err("provider is required".to_string());
    }
    if !env.is_empty() && !key.is_empty() {
        return Err("fill either env or key, not both".to_string());
    }
    if provider_requires_endpoint(provider, kind) && endpoint.is_empty() {
        return Err(format!(
            "endpoint is required for OpenAI-compatible provider `{provider}`"
        ));
    }

    let mut args = vec!["setup".to_string(), provider.to_string()];
    if !kind.is_empty() {
        args.push("--type".to_string());
        args.push(kind.to_string());
    }
    if !env.is_empty() {
        args.push("--env".to_string());
        args.push(env.to_string());
    }
    if !key.is_empty() && !is_keep_key_marker(key) {
        args.push("--key".to_string());
        args.push(key.to_string());
    }
    if !endpoint.is_empty() {
        args.push("--endpoint".to_string());
        args.push(endpoint.to_string());
    }

    Ok(args.join(" "))
}

fn provider_requires_endpoint(provider: &str, kind: &str) -> bool {
    kind.eq_ignore_ascii_case("openai") && !provider.eq_ignore_ascii_case("openai")
}

#[derive(Clone, Copy)]
struct RoleSetupDefaults {
    provider: &'static str,
    model_id: &'static str,
    model_name: &'static str,
    runtime: &'static str,
    teams: &'static str,
}

const WORKER_CODEX_ROLE_DEFAULTS: RoleSetupDefaults = RoleSetupDefaults {
    provider: "minimax",
    model_id: "minimax-m3-codex",
    model_name: "MiniMax-M3",
    runtime: "codex",
    teams: "no",
};

fn role_default_setup(role: &str) -> Option<RoleSetupDefaults> {
    match role.to_ascii_lowercase().as_str() {
        "worker-codex" => Some(WORKER_CODEX_ROLE_DEFAULTS),
        "worker-cc" => Some(RoleSetupDefaults {
            provider: "anthropic",
            model_id: "sonnet",
            model_name: "claude-sonnet-4-6",
            runtime: "claude-code",
            teams: "yes",
        }),
        "worker-gemini" => Some(RoleSetupDefaults {
            provider: "google",
            model_id: "gemini-pro",
            model_name: "gemini-1.5-pro",
            runtime: "gemini",
            teams: "no",
        }),
        "planner" => Some(RoleSetupDefaults {
            provider: "minimax",
            model_id: "minimax-m3-codex",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        "critic" => Some(RoleSetupDefaults {
            provider: "minimax",
            model_id: "minimax-m3-codex",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        "reviewer" => Some(RoleSetupDefaults {
            provider: "minimax",
            model_id: "minimax-m3-codex",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        _ => None,
    }
}

pub(super) fn role_setup_draft_args(draft: &RoleSetupDraft) -> Result<String, String> {
    let role = draft.role.trim();
    let provider = draft.provider.trim();
    let model_id = draft.model_id.trim();
    let model_name = draft.model_name.trim();
    let runtime = draft.runtime.trim();
    let teams = draft.teams.trim().to_ascii_lowercase();

    if role.is_empty() {
        return Err("role is required".to_string());
    }
    if runtime.is_empty() {
        return Err("runtime is required".to_string());
    }
    let can_derive_model_id = !provider.is_empty() && !model_name.is_empty();
    if model_id.is_empty() && !can_derive_model_id {
        return Err("provider and API model are required".to_string());
    }

    let mut args = vec!["register".to_string(), role.to_string()];
    if !can_derive_model_id {
        args.push("--model".to_string());
        args.push(model_id.to_string());
    }
    args.push("--runtime".to_string());
    args.push(runtime.to_string());
    if !provider.is_empty() {
        args.push("--provider".to_string());
        args.push(provider.to_string());
    }
    if !model_name.is_empty() {
        args.push("--model-name".to_string());
        args.push(model_name.to_string());
    }
    match teams.as_str() {
        "yes" | "true" | "on" | "agent-teams" => args.push("--agent-teams".to_string()),
        "no" | "false" | "off" | "no-agent-teams" => args.push("--no-agent-teams".to_string()),
        "auto" | "" => {}
        _ => return Err("teams must be auto, yes, or no".to_string()),
    }

    Ok(args.join(" "))
}

fn is_keep_key_marker(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "<unchanged>" | "unchanged" | "keep")
}
